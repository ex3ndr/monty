use std::cmp::Ordering;

use crate::args::{ArgExprs, ArgValues};
use crate::exceptions::{internal_err, InternalRunError, SimpleException};
use crate::expressions::{Expr, ExprLoc, Identifier, NameScope};
use crate::fstring::{fstring_interpolation, FStringPart};
use crate::heap::{Heap, HeapData};
use crate::namespace::Namespaces;
use crate::operators::{CmpOperator, Operator};
use crate::resource::ResourceTracker;
use crate::run::RunResult;
use crate::value::{Attr, Value};
use crate::values::{Dict, List, PyTrait, Str};

/// Container for evaluation context that holds all state needed during expression evaluation.
///
/// This struct bundles together the namespaces, local namespace index, and heap
/// to avoid passing them as separate parameters to every evaluation function.
/// It simplifies function signatures and makes the evaluation code more readable.
///
/// # Lifetimes
/// * `'c` - Lifetime of the code/AST (compile-time data)
/// * `'e` - Lifetime of expressions
/// * `'h` - Lifetime of the mutable borrows (namespaces and heap)
///
/// # Type Parameters
/// * `T` - The resource tracker type for enforcing execution limits
pub struct EvaluateExpr<'c, 'e, 'h, T: ResourceTracker> {
    /// The namespace stack containing all scopes (global, local, etc.)
    pub namespaces: &'h mut Namespaces<'c, 'e>,
    /// Index of the current local namespace in the namespace stack
    pub local_idx: usize,
    /// The heap for allocating and managing heap-allocated objects
    pub heap: &'h mut Heap<'c, 'e, T>,
}

impl<'c, 'e, 'h, T: ResourceTracker> EvaluateExpr<'c, 'e, 'h, T>
where
    'c: 'e,
{
    /// Creates a new `EvaluateExpr` with the given evaluation context.
    ///
    /// # Arguments
    /// * `namespaces` - The namespace stack containing all scopes
    /// * `local_idx` - Index of the current local namespace
    /// * `heap` - The heap for object allocation
    pub fn new(namespaces: &'h mut Namespaces<'c, 'e>, local_idx: usize, heap: &'h mut Heap<'c, 'e, T>) -> Self {
        Self {
            namespaces,
            local_idx,
            heap,
        }
    }

    /// Evaluates an expression node and returns a value.
    ///
    /// This is the primary evaluation method that recursively evaluates expressions
    /// and returns the resulting value. The returned value may be a heap reference
    /// that the caller is responsible for dropping via `drop_with_heap`.
    pub fn evaluate_use(&mut self, expr_loc: &'e ExprLoc<'c>) -> RunResult<'c, Value<'c, 'e>> {
        match &expr_loc.expr {
            Expr::Literal(literal) => Ok(literal.to_value()),
            Expr::Callable(callable) => Ok(callable.to_value()),
            Expr::Name(ident) => self.namespaces.get_var_value(self.local_idx, self.heap, ident),
            Expr::Call { callable, args } => {
                let args = self.evaluate_args(args)?;
                callable.call(self.namespaces, self.local_idx, self.heap, args)
            }
            Expr::AttrCall { object, attr, args } => Ok(self.attr_call(object, attr, args)?),
            Expr::Op { left, op, right } => match op {
                // Handle boolean operators with short-circuit evaluation.
                // These return the actual operand value, not a boolean.
                Operator::And => self.eval_and(left, right),
                Operator::Or => self.eval_or(left, right),
                _ => self.eval_op(left, op, right),
            },
            Expr::CmpOp { left, op, right } => Ok(self.cmp_op(left, op, right)?.into()),
            Expr::List(elements) => {
                let values = elements
                    .iter()
                    .map(|e| self.evaluate_use(e))
                    .collect::<RunResult<_>>()?;
                let heap_id = self.heap.allocate(HeapData::List(List::new(values)))?;
                Ok(Value::Ref(heap_id))
            }
            Expr::Tuple(elements) => {
                let values = elements
                    .iter()
                    .map(|e| self.evaluate_use(e))
                    .collect::<RunResult<_>>()?;
                let heap_id = self.heap.allocate(HeapData::Tuple(values))?;
                Ok(Value::Ref(heap_id))
            }
            Expr::Subscript { object, index } => {
                let obj = self.evaluate_use(object)?;
                let key = self.evaluate_use(index)?;
                let result = obj.py_getitem(&key, self.heap);
                // Drop temporary references to object and key
                obj.drop_with_heap(self.heap);
                key.drop_with_heap(self.heap);
                result
            }
            Expr::Dict(pairs) => {
                let mut eval_pairs = Vec::new();
                for (key_expr, value_expr) in pairs {
                    let key = self.evaluate_use(key_expr)?;
                    let value = self.evaluate_use(value_expr)?;
                    eval_pairs.push((key, value));
                }
                let dict = Dict::from_pairs(eval_pairs, self.heap)?;
                let dict_id = self.heap.allocate(HeapData::Dict(dict))?;
                Ok(Value::Ref(dict_id))
            }
            Expr::Not(operand) => {
                let val = self.evaluate_use(operand)?;
                let result = !val.py_bool(self.heap);
                val.drop_with_heap(self.heap);
                Ok(Value::Bool(result))
            }
            Expr::UnaryMinus(operand) => {
                let val = self.evaluate_use(operand)?;
                match val {
                    Value::Int(n) => Ok(Value::Int(-n)),
                    Value::Float(f) => Ok(Value::Float(-f)),
                    _ => {
                        use crate::exceptions::{exc_fmt, ExcType};
                        let type_name = val.py_type(Some(self.heap));
                        // Drop the value before returning error to avoid ref counting leak
                        val.drop_with_heap(self.heap);
                        Err(
                            exc_fmt!(ExcType::TypeError; "bad operand type for unary -: '{type_name}'")
                                .with_position(expr_loc.position)
                                .into(),
                        )
                    }
                }
            }
            Expr::FString(parts) => self.evaluate_fstring(parts),
        }
    }

    /// Evaluates an expression node and discards the returned value.
    ///
    /// This is an optimization for statement expressions where the result
    /// is not needed. It avoids unnecessary allocations in some cases
    /// (e.g., pure literals) while still evaluating side effects.
    pub fn evaluate_discard(&mut self, expr_loc: &'e ExprLoc<'c>) -> RunResult<'c, ()> {
        match &expr_loc.expr {
            // TODO, is this right for callable?
            Expr::Literal(_) | Expr::Callable(_) => Ok(()),
            Expr::Name(ident) => {
                // For discard, we just need to verify the variable exists
                match ident.scope {
                    NameScope::Cell => {
                        // Cell variable - look up from namespace and verify it's a cell
                        let namespace = self.namespaces.get(self.local_idx);
                        if let Value::Ref(cell_id) = namespace[ident.heap_id()] {
                            // Just verify we can access it - don't need the value
                            let _ = self.heap.get_cell_value_ref(cell_id);
                            Ok(())
                        } else {
                            panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug");
                        }
                    }
                    _ => self.namespaces.get_var_mut(self.local_idx, ident).map(|_| ()),
                }
            }
            Expr::Call { callable, args } => {
                let args = self.evaluate_args(args)?;
                let result = callable.call(self.namespaces, self.local_idx, self.heap, args)?;
                result.drop_with_heap(self.heap);
                Ok(())
            }
            Expr::AttrCall { object, attr, args } => {
                let result = self.attr_call(object, attr, args)?;
                result.drop_with_heap(self.heap);
                Ok(())
            }
            Expr::Op { left, op, right } => {
                // Handle and/or with short-circuit evaluation
                let result = match op {
                    Operator::And => self.eval_and(left, right)?,
                    Operator::Or => self.eval_or(left, right)?,
                    _ => self.eval_op(left, op, right)?,
                };
                result.drop_with_heap(self.heap);
                Ok(())
            }
            Expr::CmpOp { left, op, right } => self.cmp_op(left, op, right).map(|_| ()),
            Expr::List(elements) => {
                for el in elements {
                    self.evaluate_discard(el)?;
                }
                Ok(())
            }
            Expr::Tuple(elements) => {
                for el in elements {
                    self.evaluate_discard(el)?;
                }
                Ok(())
            }
            Expr::Subscript { object, index } => {
                self.evaluate_discard(object)?;
                self.evaluate_discard(index)?;
                Ok(())
            }
            Expr::Dict(pairs) => {
                for (key_expr, value_expr) in pairs {
                    self.evaluate_discard(key_expr)?;
                    self.evaluate_discard(value_expr)?;
                }
                Ok(())
            }
            Expr::Not(operand) | Expr::UnaryMinus(operand) => {
                self.evaluate_discard(operand)?;
                Ok(())
            }
            Expr::FString(parts) => {
                // Still need to evaluate for side effects, then drop
                let result = self.evaluate_fstring(parts)?;
                result.drop_with_heap(self.heap);
                Ok(())
            }
        }
    }

    /// Evaluates an expression for its truthiness (boolean result).
    ///
    /// This is a specialized helper for conditionals that returns a `bool`
    /// directly rather than a `Value`. It includes optimizations for
    /// comparison operators, `not`, and `and`/`or` to avoid creating
    /// intermediate `Value::Bool` objects.
    pub fn evaluate_bool(&mut self, expr_loc: &'e ExprLoc<'c>) -> RunResult<'c, bool> {
        match &expr_loc.expr {
            Expr::CmpOp { left, op, right } => self.cmp_op(left, op, right),
            // Optimize `not` to avoid creating intermediate Value::Bool
            Expr::Not(operand) => {
                let val = self.evaluate_use(operand)?;
                let result = !val.py_bool(self.heap);
                val.drop_with_heap(self.heap);
                Ok(result)
            }
            // Optimize `and`/`or` with short-circuit and direct boolean conversion
            Expr::Op { left, op, right } if matches!(op, Operator::And | Operator::Or) => {
                let result = match op {
                    Operator::And => self.eval_and(left, right)?,
                    Operator::Or => self.eval_or(left, right)?,
                    _ => unreachable!(),
                };
                let bool_result = result.py_bool(self.heap);
                result.drop_with_heap(self.heap);
                Ok(bool_result)
            }
            _ => {
                let obj = self.evaluate_use(expr_loc)?;
                let result = obj.py_bool(self.heap);
                // Drop temporary reference
                obj.drop_with_heap(self.heap);
                Ok(result)
            }
        }
    }

    pub fn heap(&mut self) -> &mut Heap<'c, 'e, T> {
        self.heap
    }

    /// Evaluates a binary operator expression (`+, -, %`, etc.).
    fn eval_op(
        &mut self,
        left: &'e ExprLoc<'c>,
        op: &Operator,
        right: &'e ExprLoc<'c>,
    ) -> RunResult<'c, Value<'c, 'e>> {
        let lhs = self.evaluate_use(left)?;
        let rhs = self.evaluate_use(right)?;
        let op_result: Option<Value> = match op {
            Operator::Add => lhs.py_add(&rhs, self.heap)?,
            Operator::Sub => lhs.py_sub(&rhs, self.heap)?,
            Operator::Mod => lhs.py_mod(&rhs),
            _ => {
                // Drop temporary references before early return
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                return internal_err!(InternalRunError::TodoError; "Operator {op:?} not yet implemented");
            }
        };
        if let Some(object) = op_result {
            // Drop temporary references to operands now that the operation is complete
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            Ok(object)
        } else {
            let lhs_type = lhs.py_type(Some(self.heap));
            let rhs_type = rhs.py_type(Some(self.heap));
            // Drop temporary references before returning error
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            SimpleException::operand_type_error(left, op, right, lhs_type, rhs_type)
        }
    }

    /// Evaluates the `and` operator with short-circuit evaluation.
    ///
    /// Returns the first falsy value encountered, or the last value if all are truthy.
    fn eval_and(&mut self, left: &'e ExprLoc<'c>, right: &'e ExprLoc<'c>) -> RunResult<'c, Value<'c, 'e>> {
        let lhs = self.evaluate_use(left)?;
        if lhs.py_bool(self.heap) {
            // Drop left operand since we're returning the right one
            lhs.drop_with_heap(self.heap);
            self.evaluate_use(right)
        } else {
            // Short-circuit: return the falsy left operand
            Ok(lhs)
        }
    }

    /// Evaluates the `or` operator with short-circuit semantics.
    ///
    /// Returns the first truthy value encountered, or the last value if all are falsy.
    fn eval_or(&mut self, left: &'e ExprLoc<'c>, right: &'e ExprLoc<'c>) -> RunResult<'c, Value<'c, 'e>> {
        let lhs = self.evaluate_use(left)?;
        if lhs.py_bool(self.heap) {
            // Short-circuit: return the truthy left operand
            Ok(lhs)
        } else {
            // Drop left operand since we're returning the right one
            lhs.drop_with_heap(self.heap);
            self.evaluate_use(right)
        }
    }

    /// Evaluates a comparison expression and returns the boolean result.
    ///
    /// Comparisons always return bool because Python chained comparisons
    /// (e.g., `1 < x < 10`) would need the intermediate value, but we don't
    /// support chaining yet, so we can return bool directly.
    fn cmp_op(&mut self, left: &'e ExprLoc<'c>, op: &CmpOperator, right: &'e ExprLoc<'c>) -> RunResult<'c, bool> {
        let lhs = self.evaluate_use(left)?;
        let rhs = self.evaluate_use(right)?;

        let result = match op {
            CmpOperator::Eq => Some(lhs.py_eq(&rhs, self.heap)),
            CmpOperator::NotEq => Some(!lhs.py_eq(&rhs, self.heap)),
            CmpOperator::Gt => lhs.py_cmp(&rhs, self.heap).map(Ordering::is_gt),
            CmpOperator::GtE => lhs.py_cmp(&rhs, self.heap).map(Ordering::is_ge),
            CmpOperator::Lt => lhs.py_cmp(&rhs, self.heap).map(Ordering::is_lt),
            CmpOperator::LtE => lhs.py_cmp(&rhs, self.heap).map(Ordering::is_le),
            CmpOperator::Is => Some(lhs.is(&rhs)),
            CmpOperator::IsNot => Some(!lhs.is(&rhs)),
            CmpOperator::ModEq(v) => lhs.py_mod_eq(&rhs, *v),
            // In/NotIn are not yet supported
            _ => None,
        };

        if let Some(v) = result {
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            Ok(v)
        } else {
            let left_type = lhs.py_type(Some(self.heap));
            let right_type = rhs.py_type(Some(self.heap));
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            SimpleException::cmp_type_error(left, op, right, left_type, right_type)
        }
    }

    /// Calls a method on an object: `object.attr(args)`.
    ///
    /// This evaluates `object`, looks up `attr`, calls the method with `args`,
    /// and handles proper cleanup of temporary values.
    fn attr_call(
        &mut self,
        object_ident: &Identifier<'c>,
        attr: &Attr,
        args: &'e ArgExprs<'c>,
    ) -> RunResult<'c, Value<'c, 'e>> {
        // Evaluate arguments first to avoid borrow conflicts
        let args = self.evaluate_args(args)?;

        // For Cell scope, look up the cell from the namespace and dereference
        if let NameScope::Cell = object_ident.scope {
            let namespace = self.namespaces.get(self.local_idx);
            let Value::Ref(cell_id) = namespace[object_ident.heap_id()] else {
                panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug")
            };
            // get_cell_value already handles refcount increment
            let mut cell_value = self.heap.get_cell_value(cell_id);
            let result = cell_value.call_attr(self.heap, attr, args);
            cell_value.drop_with_heap(self.heap);
            result
        } else {
            // For normal scopes, use get_var_mut
            let object = self.namespaces.get_var_mut(self.local_idx, object_ident)?;
            object.call_attr(self.heap, attr, args)
        }
    }

    /// Evaluates an f-string by processing its parts sequentially.
    ///
    /// Each part is either:
    /// - Literal: Appended directly to the result
    /// - Interpolation: Evaluate expression, apply conversion, apply format spec
    ///
    /// Reference counting: Intermediate values are properly dropped after formatting.
    /// The final result is a new heap-allocated string.
    fn evaluate_fstring(&mut self, parts: &'e [FStringPart<'c>]) -> RunResult<'c, Value<'c, 'e>> {
        let mut result = String::new();

        for part in parts {
            match part {
                FStringPart::Literal(s) => result.push_str(s),
                FStringPart::Interpolation {
                    expr,
                    conversion,
                    format_spec,
                } => {
                    // Evaluate the expression
                    let value = self.evaluate_use(expr)?;

                    // Process the interpolation (conversion + formatting)
                    let result = fstring_interpolation(self, &mut result, &value, *conversion, format_spec.as_ref());

                    // Always drop the evaluated value (important for reference counting)
                    value.drop_with_heap(self.heap);

                    // Propagate any error from interpolation
                    result?;
                }
            }
        }

        // Allocate result string on heap
        let heap_id = self.heap.allocate(HeapData::Str(Str::new(result)))?;
        Ok(Value::Ref(heap_id))
    }

    /// Evaluates function call arguments from expressions to values.
    fn evaluate_args(&mut self, args_expr: &'e ArgExprs<'c>) -> RunResult<'c, ArgValues<'c, 'e>> {
        match args_expr {
            ArgExprs::Zero => Ok(ArgValues::Zero),
            ArgExprs::One(arg) => self.evaluate_use(arg).map(ArgValues::One),
            ArgExprs::Two(arg1, arg2) => {
                let arg0 = self.evaluate_use(arg1)?;
                let arg1 = self.evaluate_use(arg2)?;
                Ok(ArgValues::Two(arg0, arg1))
            }
            ArgExprs::Args(args) => args
                .iter()
                .map(|a| self.evaluate_use(a))
                .collect::<RunResult<_>>()
                .map(ArgValues::Many),
            _ => todo!("Implement evaluation for kwargs"),
        }
    }
}
