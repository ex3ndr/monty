use crate::args::ArgValues;
use crate::evaluate::EvaluateExpr;
use crate::exceptions::{
    exc_err_static, exc_fmt, internal_err, ExcType, InternalRunError, RunError, SimpleException, StackFrame,
};
use crate::expressions::{ExprLoc, FrameExit, Identifier, NameScope, Node};
use crate::function::Function;
use crate::heap::Heap;
use crate::namespace::{Namespaces, GLOBAL_NS_IDX};
use crate::operators::Operator;
use crate::parse::CodeRange;
use crate::resource::ResourceTracker;
use crate::value::Value;
use crate::values::PyTrait;

pub type RunResult<'c, T> = Result<T, RunError<'c>>;

/// Represents an execution frame with an index into Namespaces.
///
/// At module level, `local_idx == GLOBAL_NS_IDX` (same namespace).
/// In functions, `local_idx` points to the function's local namespace.
/// Global variables always use `GLOBAL_NS_IDX` (0) directly.
///
/// # Closure Support
///
/// Cell variables (for closures) are stored directly in the namespace as
/// `Value::Ref(cell_id)` pointing to a `HeapData::Cell`. Both captured cells
/// (from enclosing scopes) and owned cells (for variables captured by nested
/// functions) are injected into the namespace at function call time.
///
/// When accessing a variable with `NameScope::Cell`, we look up the namespace
/// slot to get the `Value::Ref(cell_id)`, then read/write through that cell.
#[derive(Debug)]
pub(crate) struct RunFrame<'c> {
    /// Index of this frame's local namespace in Namespaces.
    local_idx: usize,
    /// Parent stack frame for error reporting.
    parent: Option<StackFrame<'c>>,
    /// The name of the current frame (function name or "<module>").
    name: &'c str,
}

impl<'c> RunFrame<'c> {
    /// Creates a new frame for module-level execution.
    ///
    /// At module level, `local_idx` is `GLOBAL_NS_IDX` (0).
    pub fn new() -> Self {
        Self {
            local_idx: GLOBAL_NS_IDX,
            parent: None,
            name: "<module>",
        }
    }

    /// Creates a new frame for function execution.
    ///
    /// The function's local namespace is at `local_idx`. Global variables
    /// always use `GLOBAL_NS_IDX` directly.
    ///
    /// Cell variables (for closures) are already injected into the namespace
    /// by Function::call or Function::call_with_cells before this frame is created.
    ///
    /// # Arguments
    /// * `local_idx` - Index of the function's local namespace in Namespaces
    /// * `name` - The function name (for error messages)
    /// * `parent` - Parent stack frame for error traceback
    pub fn new_for_function(local_idx: usize, name: &'c str, parent: Option<StackFrame<'c>>) -> Self {
        Self {
            local_idx,
            parent,
            name,
        }
    }

    pub fn execute<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        nodes: &'e [Node<'c>],
    ) -> RunResult<'c, FrameExit<'c, 'e>>
    where
        'c: 'e,
    {
        for node in nodes {
            // Check time limit at statement boundaries
            heap.tracker().check_time()?;

            // Trigger garbage collection if scheduler says it's time.
            // GC runs at statement boundaries because:
            // 1. This is a natural pause point where we have access to GC roots
            // 2. The namespace state is stable (not mid-expression evaluation)
            // Note: GC won't run during long-running single expressions (e.g., large list
            // comprehensions). This is acceptable because most Python code is structured
            // as multiple statements, and resource limits (time, memory) still apply.
            if heap.tracker().should_gc() {
                heap.collect_garbage(|| namespaces.iter_heap_ids());
            }

            if let Some(leave) = self.execute_node(namespaces, heap, node)? {
                return Ok(leave);
            }
        }
        Ok(FrameExit::Return(Value::None))
    }

    fn execute_node<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        node: &'e Node<'c>,
    ) -> RunResult<'c, Option<FrameExit<'c, 'e>>>
    where
        'c: 'e,
    {
        match node {
            Node::Expr(expr) => {
                if let Err(mut e) = EvaluateExpr::new(namespaces, self.local_idx, heap).evaluate_discard(expr) {
                    set_name(self.name, &mut e);
                    return Err(e);
                }
            }
            Node::Return(expr) => return Ok(Some(FrameExit::Return(self.execute_expr(namespaces, heap, expr)?))),
            Node::ReturnNone => return Ok(Some(FrameExit::Return(Value::None))),
            Node::Raise(exc) => self.raise(namespaces, heap, exc.as_ref())?,
            Node::Assert { test, msg } => self.assert_(namespaces, heap, test, msg.as_ref())?,
            Node::Assign { target, object } => self.assign(namespaces, heap, target, object)?,
            Node::OpAssign { target, op, object } => self.op_assign(namespaces, heap, target, op, object)?,
            Node::SubscriptAssign { target, index, value } => {
                self.subscript_assign(namespaces, heap, target, index, value)?;
            }
            Node::For {
                target,
                iter,
                body,
                or_else,
            } => self.for_loop(namespaces, heap, target, iter, body, or_else)?,
            Node::If { test, body, or_else } => self.if_(namespaces, heap, test, body, or_else)?,
            Node::FunctionDef(function) => self.define_function(namespaces, heap, function),
        }
        Ok(None)
    }

    fn execute_expr<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        expr: &'e ExprLoc<'c>,
    ) -> RunResult<'c, Value<'c, 'e>>
    where
        'c: 'e,
    {
        match EvaluateExpr::new(namespaces, self.local_idx, heap).evaluate_use(expr) {
            Ok(value) => Ok(value),
            Err(mut e) => {
                set_name(self.name, &mut e);
                Err(e)
            }
        }
    }

    fn execute_expr_bool<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        expr: &'e ExprLoc<'c>,
    ) -> RunResult<'c, bool>
    where
        'c: 'e,
    {
        match EvaluateExpr::new(namespaces, self.local_idx, heap).evaluate_bool(expr) {
            Ok(value) => Ok(value),
            Err(mut e) => {
                set_name(self.name, &mut e);
                Err(e)
            }
        }
    }

    /// Executes a raise statement.
    ///
    /// Handles:
    /// * Exception instance (Value::Exc) - raise directly
    /// * Exception type (Value::Callable with ExcType) - instantiate then raise
    /// * Anything else - TypeError
    fn raise<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        op_exc_expr: Option<&'e ExprLoc<'c>>,
    ) -> RunResult<'c, ()>
    where
        'c: 'e,
    {
        if let Some(exc_expr) = op_exc_expr {
            let value = self.execute_expr(namespaces, heap, exc_expr)?;
            match &value {
                Value::Exc(_) => {
                    // Match on the reference then use into_exc() due to issues with destructuring Value
                    let exc = value.into_exc();
                    return Err(exc.with_frame(self.stack_frame(&exc_expr.position)).into());
                }
                Value::Callable(callable) => {
                    let result = callable.call(namespaces, self.local_idx, heap, ArgValues::Zero)?;
                    // Drop the original callable value
                    if matches!(&result, Value::Exc(_)) {
                        value.drop_with_heap(heap);
                        let exc = result.into_exc();
                        return Err(exc.with_frame(self.stack_frame(&exc_expr.position)).into());
                    }
                }
                _ => {}
            }
            value.drop_with_heap(heap);
            exc_err_static!(ExcType::TypeError; "exceptions must derive from BaseException")
        } else {
            internal_err!(InternalRunError::TodoError; "plain raise not yet supported")
        }
    }

    /// Executes an assert statement by evaluating the test expression and raising
    /// `AssertionError` if the test is falsy.
    ///
    /// If a message expression is provided, it is evaluated and used as the exception message.
    fn assert_<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        test: &'e ExprLoc<'c>,
        msg: Option<&'e ExprLoc<'c>>,
    ) -> RunResult<'c, ()>
    where
        'c: 'e,
    {
        if !self.execute_expr_bool(namespaces, heap, test)? {
            let msg = if let Some(msg_expr) = msg {
                Some(
                    self.execute_expr(namespaces, heap, msg_expr)?
                        .py_str(heap)
                        .to_string()
                        .into(),
                )
            } else {
                None
            };
            return Err(SimpleException::new(ExcType::AssertionError, msg)
                .with_frame(self.stack_frame(&test.position))
                .into());
        }
        Ok(())
    }

    fn assign<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        target: &'e Identifier<'c>,
        expr: &'e ExprLoc<'c>,
    ) -> RunResult<'c, ()>
    where
        'c: 'e,
    {
        let new_value = self.execute_expr(namespaces, heap, expr)?;

        // Determine which namespace to use
        let ns_idx = match target.scope {
            NameScope::Global => GLOBAL_NS_IDX,
            _ => self.local_idx, // Local and Cell both use local namespace
        };

        if target.scope == NameScope::Cell {
            // Cell assignment - look up cell HeapId from namespace slot, then write through it
            let namespace = namespaces.get_mut(ns_idx);
            let Value::Ref(cell_id) = namespace[target.heap_id()] else {
                panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug")
            };
            heap.set_cell_value(cell_id, new_value);
        } else {
            // Direct assignment to namespace slot (Local or Global)
            let namespace = namespaces.get_mut(ns_idx);
            let old_value = std::mem::replace(&mut namespace[target.heap_id()], new_value);
            old_value.drop_with_heap(heap);
        }
        Ok(())
    }

    fn op_assign<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        target: &Identifier<'c>,
        op: &Operator,
        expr: &'e ExprLoc<'c>,
    ) -> RunResult<'c, ()>
    where
        'c: 'e,
    {
        let rhs = self.execute_expr(namespaces, heap, expr)?;
        // Capture rhs type before it's consumed by py_iadd
        let rhs_type = rhs.py_type(Some(heap));

        // Cell variables need special handling - read through cell, modify, write back
        let err_target_type = if target.scope == NameScope::Cell {
            let namespace = namespaces.get_mut(self.local_idx);
            let Value::Ref(cell_id) = namespace[target.heap_id()] else {
                panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug")
            };
            let mut cell_value = heap.get_cell_value(cell_id);
            let ok = match op {
                Operator::Add => cell_value.py_iadd(rhs, heap, None)?,
                _ => return internal_err!(InternalRunError::TodoError; "Assign operator {op:?} not yet implemented"),
            };
            if ok {
                heap.set_cell_value(cell_id, cell_value);
                None
            } else {
                Some(cell_value.py_type(Some(heap)))
            }
        } else {
            // Direct access for Local/Global scopes
            let target_val = namespaces.get_var_mut(self.local_idx, target)?;
            let ok = match op {
                Operator::Add => target_val.py_iadd(rhs, heap, None)?,
                _ => return internal_err!(InternalRunError::TodoError; "Assign operator {op:?} not yet implemented"),
            };
            if ok {
                None
            } else {
                Some(target_val.py_type(Some(heap)))
            }
        };

        if let Some(target_type) = err_target_type {
            let e = SimpleException::augmented_assign_type_error(op, target_type, rhs_type);
            Err(e.with_frame(self.stack_frame(&expr.position)).into())
        } else {
            Ok(())
        }
    }

    fn subscript_assign<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        target: &Identifier<'c>,
        index: &'e ExprLoc<'c>,
        value: &'e ExprLoc<'c>,
    ) -> RunResult<'c, ()>
    where
        'c: 'e,
    {
        let key = self.execute_expr(namespaces, heap, index)?;
        let val = self.execute_expr(namespaces, heap, value)?;
        let target_val = namespaces.get_var_mut(self.local_idx, target)?;
        if let Value::Ref(id) = target_val {
            let id = *id;
            heap.with_entry_mut(id, |heap, data| data.py_setitem(key, val, heap))
        } else {
            let e = exc_fmt!(ExcType::TypeError; "'{}' object does not support item assignment", target_val.py_type(Some(heap)));
            Err(e.with_frame(self.stack_frame(&index.position)).into())
        }
    }

    fn for_loop<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        target: &Identifier,
        iter: &'e ExprLoc<'c>,
        body: &'e [Node<'c>],
        _or_else: &'e [Node<'c>],
    ) -> RunResult<'c, ()>
    where
        'c: 'e,
    {
        let Value::Range(range_size) = self.execute_expr(namespaces, heap, iter)? else {
            return internal_err!(InternalRunError::TodoError; "`for` iter must be a range");
        };

        for value in 0i64..range_size {
            // For loop target is always local scope
            let namespace = namespaces.get_mut(self.local_idx);
            namespace[target.heap_id()] = Value::Int(value);
            self.execute(namespaces, heap, body)?;
        }
        Ok(())
    }

    fn if_<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        test: &'e ExprLoc<'c>,
        body: &'e [Node<'c>],
        or_else: &'e [Node<'c>],
    ) -> RunResult<'c, ()>
    where
        'c: 'e,
    {
        if self.execute_expr_bool(namespaces, heap, test)? {
            self.execute(namespaces, heap, body)?;
        } else {
            self.execute(namespaces, heap, or_else)?;
        }
        Ok(())
    }

    /// Defines a function (or closure) by storing it in the namespace.
    ///
    /// If the function has free_var_enclosing_slots (captures variables from enclosing scope),
    /// this captures the cells from the enclosing namespace and stores a Closure.
    /// Otherwise, it stores a simple Function reference.
    ///
    /// # Cell Sharing
    ///
    /// Closures share cells with their enclosing scope. The cell HeapIds are
    /// looked up from the enclosing namespace slots specified in free_var_enclosing_slots.
    /// This ensures modifications through `nonlocal` are visible to both scopes.
    fn define_function<'e, T: ResourceTracker>(
        &self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e, T>,
        function: &'e Function<'c>,
    ) where
        'c: 'e,
    {
        let new_value = if function.is_closure() {
            // This function captures variables from enclosing scopes.
            // Look up the cell HeapIds from the enclosing namespace.
            let enclosing_namespace = namespaces.get(self.local_idx);
            let mut captured_cells = Vec::with_capacity(function.free_var_enclosing_slots.len());

            for &enclosing_slot in &function.free_var_enclosing_slots {
                // The enclosing namespace slot contains Value::Ref(cell_id)
                let Value::Ref(cell_id) = enclosing_namespace[enclosing_slot] else {
                    panic!("Expected cell in enclosing namespace slot {enclosing_slot} - prepare-time bug")
                };

                // Increment the cell's refcount since this closure now holds a reference
                heap.inc_ref(cell_id);
                captured_cells.push(cell_id);
            }

            Value::Closure(function, captured_cells)
        } else {
            // Simple function without captures
            Value::Function(function)
        };

        let namespace = namespaces.get_mut(self.local_idx);
        let old_value = std::mem::replace(&mut namespace[function.name.heap_id()], new_value);
        // Drop the old value properly (dec_ref for Refs, no-op for others)
        old_value.drop_with_heap(heap);
    }

    fn stack_frame(&self, position: &CodeRange<'c>) -> StackFrame<'c> {
        StackFrame::new(position, self.name, self.parent.as_ref())
    }
}

fn set_name<'e>(name: &'e str, error: &mut RunError<'e>) {
    if let RunError::Exc(ref mut exc) = error {
        if let Some(ref mut stack_frame) = exc.frame {
            stack_frame.frame_name = Some(name);
        }
    }
}
