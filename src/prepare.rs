use ahash::AHashMap;
use std::borrow::Cow;

use crate::types::{Builtins, Expr, Node};
use crate::object::Object;

pub(crate) type PrepareResult<T> = Result<T, Cow<'static, str>>;

pub(crate) type RunNode = Node<usize, Builtins>;
pub(crate) type RunExpr = Expr<usize, Builtins>;

/// TODO:
/// * pre-calculate const expressions
/// * const assignment add directly to namespace
pub(crate) fn prepare(nodes: Vec<Node<String, String>>) -> PrepareResult<(Vec<Object>, Vec<RunNode>)> {
    let mut namespace = Namespace::new(nodes.len());
    let new_nodes = prepare_nodes(nodes, &mut namespace)?;
    let initial_namespace = vec![Object::Undefined; namespace.names_count];
    Ok((initial_namespace, new_nodes))
}

fn prepare_nodes(nodes: Vec<Node<String, String>>, namespace: &mut Namespace) -> PrepareResult<Vec<RunNode>> {
    let mut new_nodes = Vec::with_capacity(nodes.len());
    for node in nodes {
        match node {
            Node::Pass => (),
            Node::Expr(expr) => {
                let expr = prepare_expression(expr, namespace)?;
                new_nodes.push(Node::Expr(expr));
            }
            Node::Assign { target, object: value } => {
                let target = namespace.get_id(target);
                let value = Box::new(prepare_expression(*value, namespace)?);
                new_nodes.push(Node::Assign { target, object: value });
            }
            Node::OpAssign { target, op, object: value } => {
                let target = namespace.get_id(target);
                let value = Box::new(prepare_expression(*value, namespace)?);
                new_nodes.push(Node::OpAssign { target, op, object: value });
            }
            Node::For {
                target,
                iter,
                body,
                or_else,
            } => new_nodes.push(Node::For {
                target: prepare_expression(target, namespace)?,
                iter: prepare_expression(iter, namespace)?,
                body: prepare_nodes(body, namespace)?,
                or_else: prepare_nodes(or_else, namespace)?,
            }),
            Node::If { test, body, or_else } => new_nodes.push(Node::If {
                test: prepare_expression(test, namespace)?,
                body: prepare_nodes(body, namespace)?,
                or_else: prepare_nodes(or_else, namespace)?,
            }),
        }
    }
    Ok(new_nodes)
}

fn prepare_expression(expr: Expr<String, String>, namespace: &mut Namespace) -> PrepareResult<RunExpr> {
    match expr {
        Expr::Constant(value) => Ok(Expr::Constant(value)),
        Expr::Name(name) => Ok(Expr::Name(namespace.get_id(name))),
        Expr::Op { left, op, right } => Ok(Expr::Op {
            left: Box::new(prepare_expression(*left, namespace)?),
            op,
            right: Box::new(prepare_expression(*right, namespace)?),
        }),
        Expr::CmpOp { left, op, right } => Ok(Expr::CmpOp {
            left: Box::new(prepare_expression(*left, namespace)?),
            op,
            right: Box::new(prepare_expression(*right, namespace)?),
        }),
        Expr::Call { func, args } => {
            let func = Builtins::find(&func)?;
            Ok(Expr::Call {
                func,
                args: args
                    .into_iter()
                    .map(|e| prepare_expression(e, namespace))
                    .collect::<PrepareResult<Vec<_>>>()?,
            })
        }
        Expr::List(elements) => {
            let expressions = elements
                .into_iter()
                .map(|e| prepare_expression(e, namespace))
                .collect::<PrepareResult<Vec<_>>>()?;
            Ok(Expr::List(expressions))
        }
    }
}

struct Namespace {
    name_map: AHashMap<String, usize>,
    names_count: usize,
}

impl Namespace {
    fn new(capacity: usize) -> Self {
        Self {
            name_map: AHashMap::with_capacity(capacity),
            names_count: 0,
        }
    }

    fn get_id(&mut self, name: String) -> usize {
        *self.name_map.entry(name).or_insert_with(|| {
            let name = self.names_count;
            self.names_count += 1;
            name
        })
    }
}
