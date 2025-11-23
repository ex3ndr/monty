use crate::evaluate::{evaluate_bool, evaluate_discard, evaluate_use};
use crate::exceptions::{
    exc_err_static, exc_fmt, internal_err, ExcType, InternalRunError, RunError, SimpleException, StackFrame,
};
use crate::expressions::{Exit, ExprLoc, Identifier, Node};
use crate::heap::Heap;
use crate::object::Object;
use crate::operators::Operator;
use crate::parse::CodeRange;

pub type RunResult<'c, T> = Result<T, RunError<'c>>;

#[derive(Debug)]
pub(crate) struct RunFrame<'c> {
    namespace: Vec<Object>,
    parent: Option<StackFrame<'c>>,
    name: &'c str,
}

impl<'c> RunFrame<'c> {
    pub fn new(namespace: Vec<Object>) -> Self {
        Self {
            namespace,
            parent: None,
            name: "<module>",
        }
    }

    pub fn execute(&mut self, heap: &mut Heap, nodes: &[Node<'c>]) -> RunResult<'c, Exit<'c>> {
        for node in nodes {
            if let Some(leave) = self.execute_node(heap, node)? {
                return Ok(leave);
            }
        }
        Ok(Exit::ReturnNone)
    }

    fn execute_node(&mut self, heap: &mut Heap, node: &Node<'c>) -> RunResult<'c, Option<Exit<'c>>> {
        match node {
            Node::Pass => return internal_err!(InternalRunError::Error; "Unexpected `pass` in execution"),
            Node::Expr(expr) => {
                if let Err(mut e) = evaluate_discard(&mut self.namespace, heap, expr) {
                    set_name(self.name, &mut e);
                    return Err(e);
                }
            }
            Node::Return(expr) => return Ok(Some(Exit::Return(self.execute_expr(heap, expr)?))),
            Node::ReturnNone => return Ok(Some(Exit::ReturnNone)),
            Node::Raise(exc) => self.raise(heap, exc.as_ref())?,
            Node::Assign { target, object } => {
                self.assign(heap, target, object)?;
            }
            Node::OpAssign { target, op, object } => {
                self.op_assign(heap, target, op, object)?;
            }
            Node::For {
                target,
                iter,
                body,
                or_else,
            } => self.for_loop(heap, target, iter, body, or_else)?,
            Node::If { test, body, or_else } => self.if_(heap, test, body, or_else)?,
        }
        Ok(None)
    }

    fn execute_expr<'d>(&'d mut self, heap: &'d mut Heap, expr: &'d ExprLoc<'c>) -> RunResult<'c, Object> {
        // it seems the struct creation is optimized away, and has no cost
        match evaluate_use(&mut self.namespace, heap, expr) {
            Ok(object) => Ok(object),
            Err(mut e) => {
                set_name(self.name, &mut e);
                Err(e)
            }
        }
    }

    fn execute_expr_bool(&mut self, heap: &mut Heap, expr: &ExprLoc<'c>) -> RunResult<'c, bool> {
        match evaluate_bool(&mut self.namespace, heap, expr) {
            Ok(object) => Ok(object),
            Err(mut e) => {
                set_name(self.name, &mut e);
                Err(e)
            }
        }
    }

    fn raise(&mut self, heap: &mut Heap, op_exc_expr: Option<&ExprLoc<'c>>) -> RunResult<'c, ()> {
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

    fn assign(&mut self, heap: &mut Heap, target: &Identifier<'c>, expr: &ExprLoc<'c>) -> RunResult<'c, ()> {
        let new_value = self.execute_expr(heap, expr)?;
        let old_value = std::mem::replace(&mut self.namespace[target.id], new_value);
        if let Object::Ref(object_id) = old_value {
            heap.dec_ref(object_id);
        }
        Ok(())
    }

    fn op_assign(
        &mut self,
        heap: &mut Heap,
        target: &Identifier<'c>,
        op: &Operator,
        expr: &ExprLoc<'c>,
    ) -> RunResult<'c, ()> {
        let right_object = self.execute_expr(heap, expr)?;
        if let Some(target_object) = self.namespace.get_mut(target.id) {
            let r = match op {
                Operator::Add => target_object.add_mut(right_object, heap),
                _ => return internal_err!(InternalRunError::TodoError; "Assign operator {op:?} not yet implemented"),
            };
            if let Err(right) = r {
                let target_type = target_object.to_string();
                let right_type = right.to_string();
                let e = exc_fmt!(ExcType::TypeError; "unsupported operand type(s) for {op}: '{target_type}' and '{right_type}'");
                Err(e.with_frame(self.stack_frame(&expr.position)).into())
            } else {
                Ok(())
            }
        } else {
            let e = SimpleException::new(ExcType::NameError, Some(target.name.to_string().into()));
            Err(e.with_frame(self.stack_frame(&target.position)).into())
        }
    }

    fn for_loop(
        &mut self,
        heap: &mut Heap,
        target: &Identifier,
        iter: &ExprLoc<'c>,
        body: &[Node<'c>],
        _or_else: &[Node<'c>],
    ) -> RunResult<'c, ()> {
        let Object::Range(range_size) = self.execute_expr(heap, iter)? else {
            return internal_err!(InternalRunError::TodoError; "`for` iter must be a range");
        };

        for object in 0i64..range_size {
            self.namespace[target.id] = Object::Int(object);
            self.execute(heap, body)?;
        }
        Ok(())
    }

    fn if_<'d>(
        &mut self,
        heap: &mut Heap,
        test: &'d ExprLoc<'c>,
        body: &'d [Node<'c>],
        or_else: &'d [Node<'c>],
    ) -> RunResult<'c, ()> {
        if self.execute_expr_bool(heap, test)? {
            self.execute(heap, body)?;
        } else {
            self.execute(heap, or_else)?;
        }
        Ok(())
    }

    fn stack_frame(&self, position: &CodeRange<'c>) -> StackFrame<'c> {
        StackFrame::new(position, self.name, self.parent.as_ref())
    }
}

fn set_name<'c>(name: &'c str, error: &mut RunError<'c>) {
    if let RunError::Exc(ref mut exc) = error {
        if let Some(ref mut stack_frame) = exc.frame {
            stack_frame.frame_name = Some(name);
        }
    }
}
