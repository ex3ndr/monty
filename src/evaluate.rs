use std::cmp::Ordering;

use crate::args::{ArgExprs, ArgObjects};
use crate::exceptions::{internal_err, ExcType, InternalRunError, SimpleException};
use crate::expressions::{Expr, ExprLoc, Identifier};
use crate::heap::Heap;
use crate::object::{Attr, Object};
use crate::operators::{CmpOperator, Operator};
use crate::run::RunResult;
use crate::values::{Dict, List, PyValue};
use crate::HeapData;

/// Evaluates an expression node and returns a value.
///
/// `namespace` provides the current frame bindings, while `heap` is threaded so any
/// future heap-backed objects can be created/cloned without re-threading plumbing later.
pub(crate) fn evaluate_use<'c, 'e>(
    namespace: &mut [Object<'c, 'e>],
    heap: &mut Heap<'c, 'e>,
    expr_loc: &'e ExprLoc<'c>,
) -> RunResult<'c, Object<'c, 'e>> {
    match &expr_loc.expr {
        Expr::Literal(literal) => Ok(literal.to_object()),
        Expr::Callable(callable) => Ok(callable.to_object()),
        Expr::Name(ident) => namespace_get_mut(namespace, ident).map(|object| object.clone_with_heap(heap)),
        Expr::Call { callable, args } => {
            let args = evaluate_args(namespace, heap, args)?;
            callable.call(namespace, heap, args)
        }
        Expr::AttrCall { object, attr, args } => Ok(attr_call(namespace, heap, object, attr, args)?),
        Expr::Op { left, op, right } => eval_op(namespace, heap, left, op, right),
        Expr::CmpOp { left, op, right } => Ok(cmp_op(namespace, heap, left, op, right)?.into()),
        Expr::List(elements) => {
            let objects = elements
                .iter()
                .map(|e| evaluate_use(namespace, heap, e))
                .collect::<RunResult<_>>()?;
            let object_id = heap.allocate(HeapData::List(List::new(objects)));
            Ok(Object::Ref(object_id))
        }
        Expr::Tuple(elements) => {
            let objects = elements
                .iter()
                .map(|e| evaluate_use(namespace, heap, e))
                .collect::<RunResult<_>>()?;
            let object_id = heap.allocate(HeapData::Tuple(objects));
            Ok(Object::Ref(object_id))
        }
        Expr::Subscript { object, index } => {
            let obj = evaluate_use(namespace, heap, object)?;
            let key = evaluate_use(namespace, heap, index)?;
            let result = obj.py_getitem(&key, heap);
            // Drop temporary references to object and key
            obj.drop_with_heap(heap);
            key.drop_with_heap(heap);
            result
        }
        Expr::Dict(pairs) => {
            let mut eval_pairs = Vec::new();
            for (key_expr, value_expr) in pairs {
                let key = evaluate_use(namespace, heap, key_expr)?;
                let value = evaluate_use(namespace, heap, value_expr)?;
                eval_pairs.push((key, value));
            }
            let dict = Dict::from_pairs(eval_pairs, heap)?;
            let dict_id = heap.allocate(HeapData::Dict(dict));
            Ok(Object::Ref(dict_id))
        }
    }
}

/// Evaluates an expression node and discard the returned value
///
/// `namespace` provides the current frame bindings, while `heap` is threaded so any
/// future heap-backed objects can be created/cloned without re-threading plumbing later.
pub(crate) fn evaluate_discard<'c, 'e>(
    namespace: &mut [Object<'c, 'e>],
    heap: &mut Heap<'c, 'e>,
    expr_loc: &'e ExprLoc<'c>,
) -> RunResult<'c, ()> {
    match &expr_loc.expr {
        // TODO, is this right for callable?
        Expr::Literal(_) | Expr::Callable(_) => Ok(()),
        Expr::Name(ident) => namespace_get_mut(namespace, ident).map(|_| ()),
        Expr::Call { callable, args } => {
            let args = evaluate_args(namespace, heap, args)?;
            callable.call(namespace, heap, args)?;
            Ok(())
        }
        Expr::AttrCall { object, attr, args } => attr_call(namespace, heap, object, attr, args).map(|_| ()),
        Expr::Op { left, op, right } => eval_op(namespace, heap, left, op, right).map(|_| ()),
        Expr::CmpOp { left, op, right } => cmp_op(namespace, heap, left, op, right).map(|_| ()),
        Expr::List(elements) => {
            for el in elements {
                evaluate_discard(namespace, heap, el)?;
            }
            Ok(())
        }
        Expr::Tuple(elements) => {
            for el in elements {
                evaluate_discard(namespace, heap, el)?;
            }
            Ok(())
        }
        Expr::Subscript { object, index } => {
            evaluate_discard(namespace, heap, object)?;
            evaluate_discard(namespace, heap, index)?;
            Ok(())
        }
        Expr::Dict(pairs) => {
            for (key_expr, value_expr) in pairs {
                evaluate_discard(namespace, heap, key_expr)?;
                evaluate_discard(namespace, heap, value_expr)?;
            }
            Ok(())
        }
    }
}

/// Specialized helper for truthiness checks; shares implementation with `evaluate`.
pub(crate) fn evaluate_bool<'c, 'e>(
    namespace: &mut [Object<'c, 'e>],
    heap: &mut Heap<'c, 'e>,
    expr_loc: &'e ExprLoc<'c>,
) -> RunResult<'c, bool> {
    if let Expr::CmpOp { left, op, right } = &expr_loc.expr {
        cmp_op(namespace, heap, left, op, right)
    } else {
        let obj = evaluate_use(namespace, heap, expr_loc)?;
        let result = obj.py_bool(heap);
        // Drop temporary reference
        obj.drop_with_heap(heap);
        Ok(result)
    }
}

/// Evaluates a binary operator expression (`+, -, %`, etc.).
fn eval_op<'c, 'e>(
    namespace: &mut [Object<'c, 'e>],
    heap: &mut Heap<'c, 'e>,
    left: &'e ExprLoc<'c>,
    op: &Operator,
    right: &'e ExprLoc<'c>,
) -> RunResult<'c, Object<'c, 'e>> {
    let left_object = evaluate_use(namespace, heap, left)?;
    let right_object = evaluate_use(namespace, heap, right)?;
    let op_object: Option<Object> = match op {
        Operator::Add => left_object.py_add(&right_object, heap),
        Operator::Sub => left_object.py_sub(&right_object, heap),
        Operator::Mod => left_object.py_mod(&right_object),
        _ => {
            // Drop temporary references before early return
            left_object.drop_with_heap(heap);
            right_object.drop_with_heap(heap);
            return internal_err!(InternalRunError::TodoError; "Operator {op:?} not yet implemented");
        }
    };
    if let Some(object) = op_object {
        // Drop temporary references to operands now that the operation is complete
        left_object.drop_with_heap(heap);
        right_object.drop_with_heap(heap);
        Ok(object)
    } else {
        let left_type = left_object.py_type(heap);
        let right_type = right_object.py_type(heap);
        left_object.drop_with_heap(heap);
        right_object.drop_with_heap(heap);
        SimpleException::operand_type_error(left, op, right, left_type, right_type)
    }
}

/// Evaluates comparison operators, reusing `evaluate` so heap semantics remain consistent.
fn cmp_op<'c, 'e>(
    namespace: &mut [Object<'c, 'e>],
    heap: &mut Heap<'c, 'e>,
    left: &'e ExprLoc<'c>,
    op: &CmpOperator,
    right: &'e ExprLoc<'c>,
) -> RunResult<'c, bool> {
    let mut left_object = evaluate_use(namespace, heap, left)?;
    let mut right_object = evaluate_use(namespace, heap, right)?;

    let result = match op {
        CmpOperator::Eq => Some(left_object.py_eq(&right_object, heap)),
        CmpOperator::NotEq => Some(!left_object.py_eq(&right_object, heap)),
        CmpOperator::Gt => left_object.py_cmp(&right_object, heap).map(Ordering::is_gt),
        CmpOperator::GtE => left_object.py_cmp(&right_object, heap).map(Ordering::is_ge),
        CmpOperator::Lt => left_object.py_cmp(&right_object, heap).map(Ordering::is_lt),
        CmpOperator::LtE => left_object.py_cmp(&right_object, heap).map(Ordering::is_le),
        CmpOperator::Is => Some(left_object.is(heap, &mut right_object)),
        CmpOperator::IsNot => Some(!left_object.is(heap, &mut right_object)),
        CmpOperator::ModEq(v) => left_object.py_mod_eq(&right_object, *v),
        _ => None,
    };

    if let Some(v) = result {
        left_object.drop_with_heap(heap);
        right_object.drop_with_heap(heap);
        Ok(v)
    } else {
        let left_type = left_object.py_type(heap);
        let right_type = right_object.py_type(heap);
        left_object.drop_with_heap(heap);
        right_object.drop_with_heap(heap);
        SimpleException::cmp_type_error(left, op, right, left_type, right_type)
    }
}

/// Handles attribute method calls like `list.append`, again threading the heap for safety.
fn attr_call<'c, 'e>(
    namespace: &mut [Object<'c, 'e>],
    heap: &mut Heap<'c, 'e>,
    object_ident: &Identifier<'c>,
    attr: &Attr,
    args: &'e ArgExprs<'c>,
) -> RunResult<'c, Object<'c, 'e>> {
    // Evaluate arguments first to avoid borrow conflicts
    let args = evaluate_args(namespace, heap, args)?;

    let object = namespace_get_mut(namespace, object_ident)?;
    object.call_attr(heap, attr, args)
}

/// Evaluates function arguments into an Args, optimized for common argument counts.
fn evaluate_args<'c, 'e>(
    namespace: &mut [Object<'c, 'e>],
    heap: &mut Heap<'c, 'e>,
    args_expr: &'e ArgExprs<'c>,
) -> RunResult<'c, ArgObjects<'c, 'e>> {
    match args_expr {
        ArgExprs::Zero => Ok(ArgObjects::Zero),
        ArgExprs::One(arg) => evaluate_use(namespace, heap, arg).map(ArgObjects::One),
        ArgExprs::Two(arg1, arg2) => {
            let arg0 = evaluate_use(namespace, heap, arg1)?;
            let arg1 = evaluate_use(namespace, heap, arg2)?;
            Ok(ArgObjects::Two(arg0, arg1))
        }
        ArgExprs::Args(args) => args
            .iter()
            .map(|a| evaluate_use(namespace, heap, a))
            .collect::<RunResult<_>>()
            .map(ArgObjects::Many),
        _ => todo!("Implement evaluation for kwargs"),
    }
}

pub fn namespace_get_mut<'c, 'e, 'n>(
    namespace: &'n mut [Object<'c, 'e>],
    ident: &Identifier<'c>,
) -> RunResult<'c, &'n mut Object<'c, 'e>> {
    if let Some(object) = namespace.get_mut(ident.heap_id()) {
        match object {
            Object::Undefined => {}
            _ => return Ok(object),
        }
    }
    Err(SimpleException::new(ExcType::NameError, Some(ident.name.into()))
        .with_position(ident.position)
        .into())
}
