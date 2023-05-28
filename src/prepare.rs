use ahash::AHashMap;
use std::borrow::Cow;
use std::collections::hash_map::Entry;

use crate::evaluate::Evaluator;
use crate::object::Object;
use crate::types::{Builtins, Expr, ExprLoc, Function, Identifier, Kwarg, Node};

pub(crate) type PrepareResult<T> = Result<T, Cow<'static, str>>;

/// TODO:
/// * check variables exist before pre-assigning
pub(crate) fn prepare(nodes: Vec<Node>, input_names: &[&str]) -> PrepareResult<(Vec<Object>, Vec<Node>)> {
    let mut p = Prepare::new(nodes.len(), input_names);
    let new_nodes = p.prepare_nodes(nodes)?;
    Ok((p.namespace, new_nodes))
}

struct Prepare {
    name_map: AHashMap<String, usize>,
    namespace: Vec<Object>,
    consts: Vec<bool>,
}

impl Prepare {
    fn new(capacity: usize, input_names: &[&str]) -> Self {
        let mut name_map = AHashMap::with_capacity(capacity);
        for (index, name) in input_names.iter().enumerate() {
            name_map.insert(name.to_string(), index);
        }
        let namespace = vec![Object::Undefined; name_map.len()];
        let consts = vec![false; name_map.len()];
        Self {
            name_map,
            namespace,
            consts,
        }
    }

    fn prepare_nodes(&mut self, nodes: Vec<Node>) -> PrepareResult<Vec<Node>> {
        let mut new_nodes = Vec::with_capacity(nodes.len());
        for node in nodes {
            match node {
                Node::Pass => (),
                Node::Expr(expr) => {
                    let expr = self.prepare_expression(expr)?;
                    new_nodes.push(Node::Expr(expr));
                }
                Node::Assign { target, object } => {
                    let object = self.prepare_expression(object)?;
                    let (target, is_new) = self.get_id(target);
                    let target_id = target.id;
                    if is_new && object.expr.is_const() {
                        self.namespace[target_id] = object.expr.into_object();
                        self.consts[target_id] = true;
                    } else {
                        new_nodes.push(Node::Assign { target, object });
                        self.consts[target_id] = false;
                    }
                }
                Node::OpAssign { target, op, object } => {
                    let target = self.get_id(target).0;
                    self.consts[target.id] = false;
                    let object = self.prepare_expression(object)?;
                    new_nodes.push(Node::OpAssign { target, op, object });
                }
                Node::For {
                    target,
                    iter,
                    body,
                    or_else,
                } => new_nodes.push(Node::For {
                    target: self.prepare_expression(target)?,
                    iter: self.prepare_expression(iter)?,
                    body: self.prepare_nodes(body)?,
                    or_else: self.prepare_nodes(or_else)?,
                }),
                Node::If { test, body, or_else } => {
                    let test = self.prepare_expression(test)?;
                    let body = self.prepare_nodes(body)?;
                    let or_else = self.prepare_nodes(or_else)?;
                    if test.expr.is_const() {
                        if test.expr.into_object().bool()? {
                            new_nodes.extend(body);
                        } else {
                            new_nodes.extend(or_else);
                        }
                    } else {
                        new_nodes.push(Node::If { test, body, or_else })
                    }
                }
            }
        }
        Ok(new_nodes)
    }

    fn prepare_expression(&mut self, loc_expr: ExprLoc) -> PrepareResult<ExprLoc> {
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
                    Function::Builtin(_) => return Err("Cal prepare expected an identifier".into()),
                };
                let func = Function::Builtin(Builtins::find(&ident.name)?);
                Expr::Call {
                    func,
                    args: args
                        .into_iter()
                        .map(|e| self.prepare_expression(e))
                        .collect::<PrepareResult<_>>()?,
                    kwargs: kwargs
                        .into_iter()
                        .map(|kwarg| self.prepare_kwarg(kwarg))
                        .collect::<PrepareResult<_>>()?,
                }
            }
            Expr::List(elements) => {
                let expressions = elements
                    .into_iter()
                    .map(|e| self.prepare_expression(e))
                    .collect::<PrepareResult<_>>()?;
                Expr::List(expressions)
            }
        };

        if can_be_const(&expr, &self.consts) {
            let evaluate = Evaluator::new(&self.namespace);
            let tmp_expr_loc = ExprLoc {
                position: position.clone(),
                expr,
            };
            let object = evaluate.evaluate(&tmp_expr_loc)?;
            Ok(ExprLoc {
                position,
                expr: Expr::Constant(object.into_owned()),
            })
        } else {
            Ok(ExprLoc { position, expr })
        }
    }

    fn prepare_kwarg(&mut self, kwarg: Kwarg) -> PrepareResult<Kwarg> {
        let Kwarg { key, value } = kwarg;
        let value = self.prepare_expression(value)?;
        // WARNING: we're not setting the id on key here, this needs doing when we implement kwargs
        // or maybe key should be a string?
        Ok(Kwarg { key, value })
    }

    /// either return the id for a name, or insert that name and get its ID
    /// returns (id, whether the id is newly added)
    fn get_id(&mut self, ident: Identifier) -> (Identifier, bool) {
        let (id, is_new) = match self.name_map.entry(ident.name.clone()) {
            Entry::Occupied(e) => {
                let id = e.get();
                (*id, false)
            }
            Entry::Vacant(e) => {
                let id = self.namespace.len();
                self.namespace.push(Object::Undefined);
                self.consts.push(false);
                e.insert(id);
                (id, true)
            }
        };
        (Identifier { name: ident.name, id }, is_new)
    }
}

/// whether an expression can be evaluated to a constant
fn can_be_const(expr: &Expr, consts: &[bool]) -> bool {
    match expr {
        Expr::Constant(_) => true,
        Expr::Name(ident) => *consts.get(ident.id).unwrap_or(&false),
        Expr::Call { func, args, kwargs } => {
            !func.side_effects()
                && args.iter().all(|arg| can_be_const(&arg.expr, consts))
                && kwargs.iter().all(|kwarg| can_be_const(&kwarg.value.expr, consts))
        }
        Expr::Op { left, op: _, right } => can_be_const(&left.expr, consts) && can_be_const(&right.expr, consts),
        Expr::CmpOp { left, op: _, right } => can_be_const(&left.expr, consts) && can_be_const(&right.expr, consts),
        Expr::List(elements) => elements.iter().all(|el| can_be_const(&el.expr, consts)),
    }
}
