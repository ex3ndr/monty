use std::borrow::Cow;

use crate::prepare::{RunExpr, RunNode};
use crate::types::{Builtins, CmpOperator, Expr, Node, Operator};
use crate::object::Object;

pub type RunResult<T> = Result<T, Cow<'static, str>>;

#[derive(Debug)]
pub(crate) struct Frame {
    namespace: Vec<Object>,
}

impl Frame {
    pub fn new(namespace: Vec<Object>) -> Self {
        Self {
            namespace,
        }
    }

    pub fn execute(&mut self, nodes: &[RunNode]) -> RunResult<()> {
        for node in nodes {
            self.execute_node(node)?;
        }
        Ok(())
    }

    fn execute_node(&mut self, node: &RunNode) -> RunResult<()> {
        match node {
            Node::Pass => return Err("Unexpected `pass` in execution".into()),
            Node::Expr(expr) => {
                self.execute_expr(expr)?;
            },
            Node::Assign { target, object } => {
                self.assign(*target, object)?;
            },
            Node::OpAssign { target, op, object } => {
                self.op_assign(*target, op, &object)?;
            },
            Node::For {
                target,
                iter,
                body,
                or_else,
            } => self.for_loop(target, iter, body, or_else)?,
            Node::If { test, body, or_else } => self.if_(test, body, or_else)?,
        };
        Ok(())
    }

    fn execute_expr<'a>(&'a self, expr: &'a RunExpr) -> RunResult<Cow<Object>> {
        match expr {
            Expr::Constant(object) => Ok(Cow::Borrowed(object)),
            Expr::Name(id) => {
                if let Some(object) = self.namespace.get(*id) {
                    match object {
                        Object::Undefined => Err(format!("name '{}' is not defined", id).into()),
                        _ => Ok(Cow::Borrowed(object)),
                    }
                } else {
                    Err(format!("name '{}' is not defined", id).into())
                }
            }
            Expr::Call { func, args } => self.call_function(func, args),
            Expr::Op { left, op, right } => self.op(left, op, right),
            Expr::CmpOp { left, op, right } => Ok(Cow::Owned(self.cmp_op(left, op, right)?.into())),
            Expr::List(elements) => {
                let objects = elements
                    .iter()
                    .map(|e| match self.execute_expr(e) {
                        Ok(Cow::Borrowed(object)) => Ok(object.clone()),
                        Ok(Cow::Owned(object)) => Ok(object),
                        Err(e) => Err(e),
                    })
                    .collect::<RunResult<_>>()?;
                Ok(Cow::Owned(Object::List(objects)))
            }
        }
    }

    fn execute_expr_bool(&self, expr: &RunExpr) -> RunResult<bool> {
        match expr {
            Expr::CmpOp { left, op, right } => self.cmp_op(left, op, right),
            _ => {
                let object = self.execute_expr(expr)?;
                object.as_ref().bool().ok_or_else(|| Cow::Owned(format!("Cannot convert {} to bool", object.as_ref())))
            }
        }
    }

    fn assign(&mut self, target: usize, object: &RunExpr) -> RunResult<()> {
        self.namespace[target] = match self.execute_expr(object)? {
            Cow::Borrowed(object) => object.clone(),
            Cow::Owned(object) => object,
        };
        Ok(())
    }

    fn op_assign(&mut self, target: usize, op: &Operator, object: &RunExpr) -> RunResult<()> {
        let right_object = match self.execute_expr(object)? {
            Cow::Borrowed(object) => object.clone(),
            Cow::Owned(object) => object,
        };
        if let Some(target_object) = self.namespace.get_mut(target) {
            let ok = match op {
                Operator::Add => target_object.add_mut(right_object),
                _ => return Err(format!("Assign operator {op:?} not yet implemented").into()),
            };
            match ok {
                true => Ok(()),
                false => Err(format!("Cannot apply assign operator {op:?} {object:?}").into()),
            }
        } else {
            Err(format!("name '{target}' is not defined").into())
        }
    }

    fn call_function(&self, builtin: &Builtins, args: &[RunExpr]) -> RunResult<Cow<Object>> {
        match builtin {
            Builtins::Print => {
                for (i, arg) in args.iter().enumerate() {
                    let object = self.execute_expr(arg)?;
                    if i == 0 {
                        print!("{object}");
                    } else {
                        print!(" {object}");
                    }
                }
                println!();
                Ok(Cow::Owned(Object::None))
            }
            Builtins::Range => {
                if args.len() != 1 {
                    Err("range() takes exactly one argument".into())
                } else {
                    let object = self.execute_expr(&args[0])?;
                    match object.as_ref() {
                        Object::Int(size) => Ok(Cow::Owned(Object::Range(*size))),
                        _ => Err("range() argument must be an integer".into()),
                    }
                }
            },
            Builtins::Len => {
                if args.len() != 1 {
                    Err(format!("len() takes exactly exactly one argument ({} given)", args.len()).into())
                } else {
                    let object = self.execute_expr(&args[0])?;
                    match object.len() {
                        Some(len) => Ok(Cow::Owned(Object::Int(len as i64))),
                        None => Err(format!("Object of type {object} has no len()").into()),
                    }
                }
            }
        }
    }

    fn for_loop(
        &mut self,
        target: &RunExpr,
        iter: &RunExpr,
        body: &[RunNode],
        _or_else: &[RunNode],
    ) -> RunResult<()> {
        let target_id = match target {
            Expr::Name(id) => *id,
            _ => return Err("For target must be a name".into()),
        };
        let range_size = match self.execute_expr(iter)?.as_ref() {
            Object::Range(s) => *s,
            _ => return Err("For iter must be a range".into()),
        };

        for object in 0i64..range_size {
            self.namespace[target_id] = Object::Int(object);
            self.execute(body)?;
        }
        Ok(())
    }

    fn if_(&mut self, test: &RunExpr, body: &[RunNode], or_else: &[RunNode]) -> RunResult<()> {
        if self.execute_expr_bool(test)? {
            self.execute(body)?;
        } else {
            self.execute(or_else)?;
        }
        Ok(())
    }

    fn op(&self, left: &RunExpr, op: &Operator, right: &RunExpr) -> RunResult<Cow<Object>> {
        let left_object = self.execute_expr(left)?;
        let right_object = self.execute_expr(right)?;
        let op_object: Option<Object> = match op {
            Operator::Add => left_object.add(&right_object),
            Operator::Sub => left_object.sub(&right_object),
            Operator::Mod => left_object.modulo(&right_object),
            _ => return Err(format!("Operator {op:?} not yet implemented").into()),
        };
        match op_object {
            Some(object) => Ok(Cow::Owned(object)),
            None => Err(format!("Cannot apply operator {left:?} {op:?} {right:?}").into()),
        }
    }

    fn cmp_op(&self, left: &RunExpr, op: &CmpOperator, right: &RunExpr) -> RunResult<bool> {
        let left_object = self.execute_expr(left)?;
        let right_object = self.execute_expr(right)?;
        let op_object: Option<bool> = match op {
            CmpOperator::Eq => left_object.as_ref().eq(&right_object),
            CmpOperator::NotEq => match left_object.as_ref().eq(&right_object) {
                Some(object) => Some(!object),
                None => None,
            },
            CmpOperator::Gt => Some(left_object.gt(&right_object)),
            CmpOperator::GtE => Some(left_object.ge(&right_object)),
            CmpOperator::Lt => Some(left_object.lt(&right_object)),
            CmpOperator::LtE => Some(left_object.le(&right_object)),
            _ => return Err(format!("CmpOperator {op:?} not yet implemented").into()),
        };
        match op_object {
            Some(object) => Ok(object),
            None => Err(format!("Cannot apply comparison operator {left:?} {op:?} {right:?}").into()),
        }
    }
}
