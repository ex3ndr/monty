use crate::evaluate::{evaluate_bool, evaluate_discard, evaluate_use, namespace_get_mut};
use crate::exceptions::{exc_err_static, exc_fmt, internal_err, ExcType, InternalRunError, RunError, StackFrame};
use crate::expressions::{ExprLoc, FrameExit, Identifier, Node};
use crate::function::Function;
use crate::heap::Heap;
use crate::object::Object;
use crate::operators::Operator;
use crate::parse::CodeRange;
use crate::values::PyValue;

pub type RunResult<'c, T> = Result<T, RunError<'c>>;

#[derive(Debug)]
pub(crate) struct RunFrame<'c, 'e> {
    namespace: Vec<Object<'c, 'e>>,
    parent: Option<StackFrame<'c>>,
    /// The name of the current frame (function name or "<module>").
    name: &'c str,
}

impl<'c, 'e> RunFrame<'c, 'e>
where
    'c: 'e,
{
    pub fn new(namespace: Vec<Object<'c, 'e>>) -> Self {
        Self {
            namespace,
            parent: None,
            name: "<module>",
        }
    }

    /// Creates a new frame for function execution with captured closure cells.
    pub fn new_for_function(namespace: Vec<Object<'c, 'e>>, name: &'c str, parent: Option<StackFrame<'c>>) -> Self {
        Self {
            namespace,
            parent,
            name,
        }
    }

    /// Consumes the frame and returns the namespace for inspection (e.g., ref-count testing).
    ///
    /// Only available when the `ref-counting` feature is enabled.
    #[cfg(feature = "ref-counting")]
    pub fn into_namespace(self) -> Vec<Object<'c, 'e>> {
        self.namespace
    }

    pub fn execute(&mut self, heap: &mut Heap<'c, 'e>, nodes: &'e [Node<'c>]) -> RunResult<'c, FrameExit<'c, 'e>> {
        for node in nodes {
            if let Some(leave) = self.execute_node(heap, node)? {
                return Ok(leave);
            }
        }
        Ok(FrameExit::Return(Object::None))
    }

    fn execute_node(
        &mut self,
        heap: &mut Heap<'c, 'e>,
        node: &'e Node<'c>,
    ) -> RunResult<'c, Option<FrameExit<'c, 'e>>> {
        match node {
            Node::Expr(expr) => {
                if let Err(mut e) = evaluate_discard(&mut self.namespace, heap, expr) {
                    set_name(self.name, &mut e);
                    return Err(e);
                }
            }
            Node::Return(expr) => return Ok(Some(FrameExit::Return(self.execute_expr(heap, expr)?))),
            Node::ReturnNone => return Ok(Some(FrameExit::Return(Object::None))),
            Node::Raise(exc) => self.raise(heap, exc.as_ref())?,
            Node::Assign { target, object } => self.assign(heap, target, object)?,
            Node::OpAssign { target, op, object } => self.op_assign(heap, target, op, object)?,
            Node::SubscriptAssign { target, index, value } => {
                self.subscript_assign(heap, target, index, value)?;
            }
            Node::For {
                target,
                iter,
                body,
                or_else,
            } => self.for_loop(heap, target, iter, body, or_else)?,
            Node::If { test, body, or_else } => self.if_(heap, test, body, or_else)?,
            Node::FunctionDef(function) => self.define_function(heap, function),
        }
        Ok(None)
    }

    fn execute_expr(&mut self, heap: &mut Heap<'c, 'e>, expr: &'e ExprLoc<'c>) -> RunResult<'c, Object<'c, 'e>> {
        // it seems the struct creation is optimized away, and has no cost
        match evaluate_use(&mut self.namespace, heap, expr) {
            Ok(object) => Ok(object),
            Err(mut e) => {
                set_name(self.name, &mut e);
                Err(e)
            }
        }
    }

    fn execute_expr_bool(&mut self, heap: &mut Heap<'c, 'e>, expr: &'e ExprLoc<'c>) -> RunResult<'c, bool> {
        match evaluate_bool(&mut self.namespace, heap, expr) {
            Ok(object) => Ok(object),
            Err(mut e) => {
                set_name(self.name, &mut e);
                Err(e)
            }
        }
    }

    fn raise(&mut self, heap: &mut Heap<'c, 'e>, op_exc_expr: Option<&'e ExprLoc<'c>>) -> RunResult<'c, ()> {
        if let Some(exc_expr) = op_exc_expr {
            let object = self.execute_expr(heap, exc_expr)?;
            match object {
                Object::Exc(exc) => Err(exc.with_frame(self.stack_frame(&exc_expr.position)).into()),
                _ => exc_err_static!(ExcType::TypeError; "exceptions must derive from BaseException"),
            }
        } else {
            internal_err!(InternalRunError::TodoError; "plain raise not yet supported")
        }
    }

    fn assign(
        &mut self,
        heap: &mut Heap<'c, 'e>,
        target: &'e Identifier<'c>,
        expr: &'e ExprLoc<'c>,
    ) -> RunResult<'c, ()> {
        let new_value = self.execute_expr(heap, expr)?;
        let old_value = std::mem::replace(&mut self.namespace[target.heap_id()], new_value);
        if let Object::Ref(object_id) = old_value {
            heap.dec_ref(object_id);
        }
        Ok(())
    }

    fn op_assign(
        &mut self,
        heap: &mut Heap<'c, 'e>,
        target: &Identifier<'c>,
        op: &Operator,
        expr: &'e ExprLoc<'c>,
    ) -> RunResult<'c, ()> {
        let right_object = self.execute_expr(heap, expr)?;
        let target_object = namespace_get_mut(&mut self.namespace, target)?;
        let ok = match op {
            Operator::Add => target_object.py_iadd(right_object, heap, None),
            _ => return internal_err!(InternalRunError::TodoError; "Assign operator {op:?} not yet implemented"),
        };
        if ok {
            Ok(())
        } else {
            // TODO this should probably move into exception.rs
            let target_type = target_object.py_type(heap);
            let right_type = target_object.py_type(heap);
            let e = exc_fmt!(ExcType::TypeError; "unsupported operand type(s) for {op}: '{target_type}' and '{right_type}'");
            Err(e.with_frame(self.stack_frame(&expr.position)).into())
        }
    }

    fn subscript_assign(
        &mut self,
        heap: &mut Heap<'c, 'e>,
        target: &Identifier<'c>,
        index: &'e ExprLoc<'c>,
        value: &'e ExprLoc<'c>,
    ) -> RunResult<'c, ()> {
        let key = self.execute_expr(heap, index)?;
        let val = self.execute_expr(heap, value)?;
        let target_object = namespace_get_mut(&mut self.namespace, target)?;
        if let Object::Ref(id) = target_object {
            let id = *id;
            heap.with_entry_mut(id, |heap, data| data.py_setitem(key, val, heap))
        } else {
            let e = exc_fmt!(ExcType::TypeError; "'{}' object does not support item assignment", target_object.py_type(heap));
            Err(e.with_frame(self.stack_frame(&index.position)).into())
        }
    }

    fn for_loop(
        &mut self,
        heap: &mut Heap<'c, 'e>,
        target: &Identifier,
        iter: &'e ExprLoc<'c>,
        body: &'e [Node<'c>],
        _or_else: &'e [Node<'c>],
    ) -> RunResult<'c, ()> {
        let Object::Range(range_size) = self.execute_expr(heap, iter)? else {
            return internal_err!(InternalRunError::TodoError; "`for` iter must be a range");
        };

        for object in 0i64..range_size {
            self.namespace[target.heap_id()] = Object::Int(object);
            self.execute(heap, body)?;
        }
        Ok(())
    }

    fn if_(
        &mut self,
        heap: &mut Heap<'c, 'e>,
        test: &'e ExprLoc<'c>,
        body: &'e [Node<'c>],
        or_else: &'e [Node<'c>],
    ) -> RunResult<'c, ()> {
        if self.execute_expr_bool(heap, test)? {
            self.execute(heap, body)?;
        } else {
            self.execute(heap, or_else)?;
        }
        Ok(())
    }

    fn define_function(&mut self, heap: &mut Heap<'c, 'e>, function: &'e Function<'c>) {
        let old_value = std::mem::replace(&mut self.namespace[function.name.heap_id()], Object::Function(function));
        if let Object::Ref(object_id) = old_value {
            heap.dec_ref(object_id);
        }
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
