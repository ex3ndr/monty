use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use crate::exceptions::{internal_err, ExcType, Exception, InternalRunError};
use crate::expressions::{Expr, ExprLoc, Function, Identifier, Kwarg};
use crate::heap::Heap;
use crate::object::{Attr, Object};
use crate::operators::{CmpOperator, Operator};
use crate::run::RunResult;

/// Evaluates an expression node and returns a value backed by the shared heap.
///
/// `namespace` provides the current frame bindings, while `heap` is threaded so any
/// future heap-backed objects can be created/cloned without re-threading plumbing later.
pub(crate) fn evaluate<'c, 'd>(
    namespace: &'d mut [Object],
    heap: &'d mut Heap,
    expr_loc: &'d ExprLoc<'c>,
) -> RunResult<'c, Cow<'d, Object>> {
    match &expr_loc.expr {
        Expr::Constant(literal) => Ok(Cow::Owned(literal.to_object())),
        Expr::Name(ident) => {
            if let Some(object) = namespace.get(ident.id) {
                match object {
                    Object::Undefined => Err(InternalRunError::Undefined(ident.name.clone().into()).into()),
                    _ => Ok(Cow::Borrowed(object)),
                }
            } else {
                let name = ident.name.clone();

                Err(Exception::new(name, ExcType::NameError)
                    .with_position(expr_loc.position)
                    .into())
            }
        }
        Expr::Call { func, args, kwargs } => Ok(call_function(namespace, heap, func, args, kwargs)?),
        Expr::AttrCall {
            object,
            attr,
            args,
            kwargs,
        } => Ok(attr_call(namespace, heap, expr_loc, object, attr, args, kwargs)?),
        // Expr::AttrCall { .. } => todo!(),
        Expr::Op { left, op, right } => eval_op(namespace, heap, left, op, right),
        Expr::CmpOp { left, op, right } => Ok(Cow::Owned(cmp_op(namespace, heap, left, op, right)?.into())),
        Expr::List(elements) => {
            let objects = elements
                .iter()
                .map(|e| evaluate(namespace, heap, e).map(std::borrow::Cow::into_owned))
                .collect::<RunResult<_>>()?;
            Ok(Cow::Owned(Object::List(Rc::new(RefCell::new(objects)))))
        }
    }
}

/// Specialized helper for truthiness checks; shares implementation with `evaluate`.
pub(crate) fn evaluate_bool<'c, 'd>(
    namespace: &'d mut [Object],
    heap: &'d mut Heap,
    expr_loc: &'d ExprLoc<'c>,
) -> RunResult<'c, bool> {
    match &expr_loc.expr {
        Expr::CmpOp { left, op, right } => cmp_op(namespace, heap, left, op, right),
        _ => Ok(evaluate(namespace, heap, expr_loc)?.as_ref().bool()),
    }
}

/// Evaluates a binary operator expression (`+, -, %`, etc.).
fn eval_op<'c, 'd>(
    namespace: &'d mut [Object],
    heap: &'d mut Heap,
    left: &'d ExprLoc<'c>,
    op: &'d Operator,
    right: &'d ExprLoc<'c>,
) -> RunResult<'c, Cow<'d, Object>> {
    let left_object = evaluate(namespace, heap, left)?.into_owned();
    let right_object = evaluate(namespace, heap, right)?;
    let op_object: Option<Object> = match op {
        Operator::Add => left_object.add(&right_object),
        Operator::Sub => left_object.sub(&right_object),
        Operator::Mod => left_object.modulus(&right_object),
        _ => return internal_err!(InternalRunError::TodoError; "Operator {op:?} not yet implemented"),
    };
    match op_object {
        Some(object) => Ok(Cow::Owned(object)),
        None => Exception::operand_type_error(left, op, right, Cow::Owned(left_object), right_object),
    }
}

/// Evaluates comparison operators, reusing `evaluate` so heap semantics remain consistent.
fn cmp_op<'c, 'd>(
    namespace: &'d mut [Object],
    heap: &'d mut Heap,
    left: &'d ExprLoc<'c>,
    op: &'d CmpOperator,
    right: &'d ExprLoc<'c>,
) -> RunResult<'c, bool> {
    let left_object = evaluate(namespace, heap, left)?.into_owned();
    let right_object = evaluate(namespace, heap, right)?;
    let left_cow: Cow<Object> = Cow::Owned(left_object);
    match op {
        CmpOperator::Eq => Ok(left_cow.as_ref().py_eq(&right_object)),
        CmpOperator::NotEq => Ok(!left_cow.as_ref().py_eq(&right_object)),
        CmpOperator::Gt => Ok(left_cow.gt(&right_object)),
        CmpOperator::GtE => Ok(left_cow.ge(&right_object)),
        CmpOperator::Lt => Ok(left_cow.lt(&right_object)),
        CmpOperator::LtE => Ok(left_cow.le(&right_object)),
        CmpOperator::ModEq(v) => match left_cow.as_ref().modulus_eq(&right_object, *v) {
            Some(b) => Ok(b),
            None => Exception::operand_type_error(left, Operator::Mod, right, left_cow, right_object),
        },
        _ => internal_err!(InternalRunError::TodoError; "Operator {op:?} not yet implemented"),
    }
}

/// Evaluates builtin function calls, collecting argument values via the shared heap.
fn call_function<'c, 'd>(
    namespace: &'d mut [Object],
    heap: &'d mut Heap,
    function: &'d Function,
    args: &'d [ExprLoc<'c>],
    _kwargs: &'d [Kwarg],
) -> RunResult<'c, Cow<'d, Object>> {
    let builtin = match function {
        Function::Builtin(builtin) => builtin,
        Function::Ident(_) => {
            return internal_err!(InternalRunError::TodoError; "User defined functions not yet implemented")
        }
    };
    let args: Vec<Cow<Object>> = args
        .iter()
        .map(|a| evaluate(namespace, heap, a).map(|o| Cow::Owned(o.into_owned())))
        .collect::<RunResult<_>>()?;
    builtin.call_function(args)
}

/// Handles attribute method calls like `list.append`, again threading the heap for safety.
fn attr_call<'c, 'd>(
    namespace: &'d mut [Object],
    heap: &'d mut Heap,
    expr_loc: &'d ExprLoc<'c>,
    object_ident: &Identifier<'c>,
    attr: &Attr,
    args: &'d [ExprLoc<'c>],
    _kwargs: &'d [Kwarg],
) -> RunResult<'c, Cow<'d, Object>> {
    // Evaluate arguments first to avoid borrow conflicts
    let args: Vec<Cow<Object>> = args
        .iter()
        .map(|a| evaluate(namespace, heap, a).map(|o| Cow::Owned(o.into_owned())))
        .collect::<RunResult<_>>()?;

    let object = if let Some(object) = namespace.get_mut(object_ident.id) {
        match object {
            Object::Undefined => return Err(InternalRunError::Undefined(object_ident.name.clone().into()).into()),
            _ => object,
        }
    } else {
        let name = object_ident.name.clone();

        return Err(Exception::new(name, ExcType::NameError)
            .with_position(expr_loc.position)
            .into());
    };
    object.attr_call(attr, args)
}
