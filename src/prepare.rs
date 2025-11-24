use std::collections::hash_map::Entry;

use ahash::AHashMap;

use crate::exceptions::{internal_err, ExcType, ExceptionRaise, SimpleException};
use crate::expressions::{Expr, ExprLoc, Function, Identifier, Kwarg, Node};
use crate::literal::Literal;
use crate::object_types::Types;
use crate::operators::{CmpOperator, Operator};
use crate::parse_error::{ParseError, ParseResult};

/// Prepares parsed nodes for execution by resolving names and building the initial namespace.
///
/// Returns the initial namespace as Literals (not runtime Objects) along with the prepared nodes.
/// The namespace will be converted to runtime Objects when execution begins and the heap is available.
pub(crate) fn prepare<'c>(
    nodes: Vec<Node<'c>>,
    input_names: &[&str],
) -> ParseResult<'c, (Vec<Literal>, Vec<Node<'c>>)> {
    let mut p = Prepare::new(nodes.len(), input_names, true);
    let new_nodes = p.prepare_nodes(nodes)?;
    Ok((p.namespace, new_nodes))
}

struct Prepare {
    name_map: AHashMap<String, usize>,
    namespace: Vec<Literal>,
    /// Root frame is the outer frame of the script, e.g. the "global" scope
    root_frame: bool,
}

impl Prepare {
    fn new(capacity: usize, input_names: &[&str], root_frame: bool) -> Self {
        let mut name_map = AHashMap::with_capacity(capacity);
        for (index, name) in input_names.iter().enumerate() {
            name_map.insert((*name).to_string(), index);
        }
        let namespace = vec![Literal::Undefined; name_map.len()];
        Self {
            name_map,
            namespace,
            root_frame,
        }
    }

    fn prepare_nodes<'c>(&mut self, nodes: Vec<Node<'c>>) -> ParseResult<'c, Vec<Node<'c>>> {
        let nodes_len = nodes.len();
        let mut new_nodes = Vec::with_capacity(nodes_len);
        for (index, node) in nodes.into_iter().enumerate() {
            match node {
                Node::Pass => (),
                Node::Expr(expr) => {
                    let expr = self.prepare_expression(expr)?;
                    // if we're in the root frame, and the expr is the last node, and it's not None, return it
                    if self.root_frame && index == nodes_len - 1 && !expr.expr.is_none() {
                        new_nodes.push(Node::Return(expr));
                    } else {
                        new_nodes.push(Node::Expr(expr));
                    }
                }
                Node::Return(expr) => {
                    let expr = self.prepare_expression(expr)?;
                    new_nodes.push(Node::Return(expr));
                }
                Node::ReturnNone => new_nodes.push(Node::ReturnNone),
                Node::Raise(exc) => {
                    let expr = match exc {
                        Some(expr) => {
                            match expr.expr {
                                Expr::Name(id) => {
                                    // this is raising an exception type, e.g. `raise TypeError`
                                    let expr = Expr::Call {
                                        func: Function::Builtin(Types::find(&id.name)?),
                                        args: vec![],
                                        kwargs: vec![],
                                    };
                                    Some(ExprLoc::new(id.position, expr))
                                }
                                _ => Some(self.prepare_expression(expr)?),
                            }
                        }
                        None => None,
                    };
                    new_nodes.push(Node::Raise(expr));
                }
                Node::Assign { target, object } => {
                    let object = self.prepare_expression(object)?;
                    let (target, _) = self.get_id(target);
                    new_nodes.push(Node::Assign { target, object });
                }
                Node::OpAssign { target, op, object } => {
                    let target = self.get_id(target).0;
                    let object = self.prepare_expression(object)?;
                    new_nodes.push(Node::OpAssign { target, op, object });
                }
                Node::For {
                    target,
                    iter,
                    body,
                    or_else,
                } => {
                    new_nodes.push(Node::For {
                        target: self.get_id(target).0,
                        iter: self.prepare_expression(iter)?,
                        body: self.prepare_nodes(body)?,
                        or_else: self.prepare_nodes(or_else)?,
                    });
                }
                Node::If { test, body, or_else } => {
                    let test = self.prepare_expression(test)?;
                    let body = self.prepare_nodes(body)?;
                    let or_else = self.prepare_nodes(or_else)?;
                    new_nodes.push(Node::If { test, body, or_else });
                }
            }
        }
        Ok(new_nodes)
    }

    fn prepare_expression<'c>(&mut self, loc_expr: ExprLoc<'c>) -> ParseResult<'c, ExprLoc<'c>> {
        let ExprLoc { position, expr } = loc_expr;
        let expr = match expr {
            Expr::Constant(object) => Expr::Constant(object),
            Expr::Name(name) => Expr::Name(self.get_id(name).0),
            Expr::Op { left, op, right } => Expr::Op {
                left: Box::new(self.prepare_expression(*left)?),
                op,
                right: Box::new(self.prepare_expression(*right)?),
            },
            Expr::CmpOp { left, op, right } => Expr::CmpOp {
                left: Box::new(self.prepare_expression(*left)?),
                op,
                right: Box::new(self.prepare_expression(*right)?),
            },
            Expr::Call { func, args, kwargs } => {
                let ident = match func {
                    Function::Ident(ident) => ident,
                    Function::Builtin(_) => {
                        return internal_err!(ParseError::Internal; "Call prepare expected an identifier")
                    }
                };
                let func = Function::Builtin(Types::find(&ident.name)?);
                let (args, kwargs) = self.get_args_kwargs(args, kwargs)?;
                Expr::Call { func, args, kwargs }
            }
            Expr::AttrCall {
                object,
                attr,
                args,
                kwargs,
            } => {
                let (object, is_new) = self.get_id(object);
                if is_new {
                    let exc: ExceptionRaise = SimpleException::new(ExcType::NameError, Some(object.name.into())).into();
                    return Err(exc.into());
                }
                let (args, kwargs) = self.get_args_kwargs(args, kwargs)?;
                Expr::AttrCall {
                    object,
                    attr,
                    args,
                    kwargs,
                }
            }
            Expr::List(elements) => {
                let expressions = elements
                    .into_iter()
                    .map(|e| self.prepare_expression(e))
                    .collect::<ParseResult<_>>()?;
                Expr::List(expressions)
            }
            Expr::Tuple(elements) => {
                let expressions = elements
                    .into_iter()
                    .map(|e| self.prepare_expression(e))
                    .collect::<ParseResult<_>>()?;
                Expr::Tuple(expressions)
            }
        };

        if let Expr::CmpOp { left, op, right } = &expr {
            if op == &CmpOperator::Eq {
                if let Expr::Constant(Literal::Int(value)) = right.expr {
                    if let Expr::Op {
                        left: left2,
                        op,
                        right: right2,
                    } = &left.expr
                    {
                        if op == &Operator::Mod {
                            let new_expr = Expr::CmpOp {
                                left: left2.clone(),
                                op: CmpOperator::ModEq(value),
                                right: right2.clone(),
                            };
                            return Ok(ExprLoc {
                                position: left.position,
                                expr: new_expr,
                            });
                        }
                    }
                }
            }
        }

        Ok(ExprLoc { position, expr })
    }

    fn prepare_kwarg<'c>(&mut self, kwarg: Kwarg<'c>) -> ParseResult<'c, Kwarg<'c>> {
        let Kwarg { key, value } = kwarg;
        let value = self.prepare_expression(value)?;
        // WARNING: we're not setting the id on key here, this needs doing when we implement kwargs
        // or maybe key should be a string?
        Ok(Kwarg { key, value })
    }

    /// either return the id for a name, or insert that name and get its ID
    /// returns (id, whether the id is newly added)
    fn get_id<'c>(&mut self, ident: Identifier<'c>) -> (Identifier<'c>, bool) {
        let (id, is_new) = match self.name_map.entry(ident.name.clone()) {
            Entry::Occupied(e) => {
                let id = e.get();
                (*id, false)
            }
            Entry::Vacant(e) => {
                let id = self.namespace.len();
                self.namespace.push(Literal::Undefined);
                e.insert(id);
                (id, true)
            }
        };
        (
            Identifier {
                name: ident.name,
                id,
                position: ident.position,
            },
            is_new,
        )
    }

    fn get_args_kwargs<'c>(
        &mut self,
        args: Vec<ExprLoc<'c>>,
        kwargs: Vec<Kwarg<'c>>,
    ) -> ParseResult<'c, (Vec<ExprLoc<'c>>, Vec<Kwarg<'c>>)> {
        let args = args
            .into_iter()
            .map(|e| self.prepare_expression(e))
            .collect::<ParseResult<_>>()?;
        let kwargs = kwargs
            .into_iter()
            .map(|kwarg| self.prepare_kwarg(kwarg))
            .collect::<ParseResult<_>>()?;
        Ok((args, kwargs))
    }
}
