use std::collections::hash_map::Entry;

use ahash::AHashMap;

use crate::args::ArgExprs;
use crate::callable::Callable;
use crate::exceptions::{ExcType, ExceptionRaise, SimpleException};
use crate::expressions::{Expr, ExprLoc, Identifier, Literal, Node};
use crate::function::Function;
use crate::operators::{CmpOperator, Operator};
use crate::parse::ParseNode;
use crate::parse_error::ParseError;

/// Result of the prepare phase, containing everything needed to execute code.
///
/// This struct holds the outputs of name resolution and AST transformation:
/// - The initial namespace with placeholder values for each variable
/// - A mapping from variable names to their namespace indices (for ref-count testing)
/// - The transformed AST nodes ready for execution
pub(crate) struct PrepareResult<'c> {
    /// Number of items in the namespace
    pub namespace_size: usize,
    /// Maps variable names to their indices in the namespace.
    /// Used for ref-count testing to look up variables by name.
    /// Only available when the `ref-counting` feature is enabled.
    #[cfg(feature = "ref-counting")]
    pub name_map: AHashMap<String, usize>,
    /// The prepared AST nodes with all names resolved to namespace indices.
    pub nodes: Vec<Node<'c>>,
}

/// Prepares parsed nodes for execution by resolving names and building the initial namespace.
///
/// The namespace will be converted to runtime Objects when execution begins and the heap is available.
pub(crate) fn prepare<'c>(
    nodes: Vec<ParseNode<'c>>,
    input_names: &[&str],
) -> Result<PrepareResult<'c>, ParseError<'c>> {
    let mut p = Prepare::new(nodes.len(), input_names, true);
    let prepared_nodes = p.prepare_nodes(nodes)?;
    Ok(PrepareResult {
        namespace_size: p.namespace_size,
        #[cfg(feature = "ref-counting")]
        name_map: p.name_map,
        nodes: prepared_nodes,
    })
}

/// State machine for the preparation phase that transforms parsed AST nodes into an executable form.
///
/// This struct maintains the mapping between variable names and their namespace indices,
/// builds the initial namespace with Literals (pre-runtime), and handles scope resolution.
/// The preparation phase is crucial for converting string-based name lookups into efficient
/// integer-indexed namespace access during execution.
struct Prepare {
    /// Maps variable names to their indices in the namespace vector
    name_map: AHashMap<String, usize>,
    /// Number of items in the namespace
    pub namespace_size: usize,
    /// Root frame is the outer frame of the script, e.g. the "global" scope.
    /// When true, the last expression in a block is implicitly returned.
    root_frame: bool,
}

impl Prepare {
    /// Creates a new Prepare instance with pre-populated input names.
    ///
    /// Input names are typically function parameters or external variables that are
    /// already defined when execution begins. They are given sequential namespace indices
    /// starting from 0.
    ///
    /// # Arguments
    /// * `capacity` - Expected number of nodes, used to preallocate the name map
    /// * `input_names` - Names that should be pre-registered in the namespace (e.g., function parameters)
    /// * `root_frame` - Whether this is the root/global scope (affects return behavior)
    fn new(capacity: usize, input_names: &[&str], root_frame: bool) -> Self {
        let mut name_map = AHashMap::with_capacity(capacity);
        for (index, name) in input_names.iter().enumerate() {
            name_map.insert((*name).to_string(), index);
        }
        let namespace_size = name_map.len();
        Self {
            name_map,
            namespace_size,
            root_frame,
        }
    }

    /// Recursively prepares a sequence of AST nodes by resolving names and transforming expressions.
    ///
    /// This method processes each node type differently:
    /// - Resolves variable names to namespace indices
    /// - Transforms function calls from identifier-based to builtin type-based
    /// - Handles special cases like implicit returns in root frames
    /// - Validates that names used in attribute calls are already defined
    ///
    /// # Returns
    /// A vector of prepared nodes ready for execution
    fn prepare_nodes<'c>(&mut self, nodes: Vec<ParseNode<'c>>) -> Result<Vec<Node<'c>>, ParseError<'c>> {
        let nodes_len = nodes.len();
        let mut new_nodes = Vec::with_capacity(nodes_len);
        for (index, node) in nodes.into_iter().enumerate() {
            match node {
                ParseNode::Pass => (),
                ParseNode::Expr(expr) => {
                    let expr = self.prepare_expression(expr)?;
                    // In the root frame (global scope), the last expression is implicitly returned
                    // if it's not None. This matches Python REPL behavior where the last expression
                    // value is displayed/returned.
                    if self.root_frame && index == nodes_len - 1 && !expr.expr.is_none() {
                        new_nodes.push(Node::Return(expr));
                    } else {
                        new_nodes.push(Node::Expr(expr));
                    }
                }
                ParseNode::Return(expr) => {
                    let expr = self.prepare_expression(expr)?;
                    new_nodes.push(Node::Return(expr));
                }
                ParseNode::ReturnNone => new_nodes.push(Node::ReturnNone),
                ParseNode::Raise(exc) => {
                    let expr = match exc {
                        Some(expr) => {
                            match expr.expr {
                                // Handle raising an exception type constant without instantiation,
                                // e.g. `raise TypeError`. This is transformed into a call: `raise TypeError()`
                                // so the exception is properly instantiated before being raised.
                                Expr::Callable(Callable::ExcType(exc_type)) => {
                                    let call_expr = Expr::Call {
                                        callable: Callable::ExcType(exc_type),
                                        args: ArgExprs::Zero,
                                    };
                                    Some(ExprLoc::new(expr.position, call_expr))
                                }
                                // Handle raising a builtin constant (unlikely but consistent)
                                Expr::Callable(Callable::Builtin(builtin)) => {
                                    let call_expr = Expr::Call {
                                        callable: Callable::Builtin(builtin),
                                        args: ArgExprs::Zero,
                                    };
                                    Some(ExprLoc::new(expr.position, call_expr))
                                }
                                Expr::Name(id) => {
                                    // Handle raising a variable that holds an exception type.
                                    // This is transformed into a call so the exception is properly instantiated.
                                    let position = id.position;
                                    // Name will be looked up in the namespace at runtime
                                    let (resolved_id, is_new) = self.get_id(id);
                                    if is_new {
                                        let exc: ExceptionRaise = SimpleException::new(
                                            ExcType::NameError,
                                            Some(resolved_id.name.to_owned().into()),
                                        )
                                        .into();
                                        return Err(exc.into());
                                    }
                                    let call_expr = Expr::Call {
                                        callable: Callable::Name(resolved_id),
                                        args: ArgExprs::Zero,
                                    };
                                    Some(ExprLoc::new(position, call_expr))
                                }
                                _ => Some(self.prepare_expression(expr)?),
                            }
                        }
                        None => None,
                    };
                    new_nodes.push(Node::Raise(expr));
                }
                ParseNode::Assign { target, object } => {
                    let object = self.prepare_expression(object)?;
                    let (target, _) = self.get_id(target);
                    new_nodes.push(Node::Assign { target, object });
                }
                ParseNode::OpAssign { target, op, object } => {
                    let target = self.get_id(target).0;
                    let object = self.prepare_expression(object)?;
                    new_nodes.push(Node::OpAssign { target, op, object });
                }
                ParseNode::SubscriptAssign { target, index, value } => {
                    let target = self.get_id(target).0;
                    let index = self.prepare_expression(index)?;
                    let value = self.prepare_expression(value)?;
                    new_nodes.push(Node::SubscriptAssign { target, index, value });
                }
                ParseNode::For {
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
                ParseNode::If { test, body, or_else } => {
                    let test = self.prepare_expression(test)?;
                    let body = self.prepare_nodes(body)?;
                    let or_else = self.prepare_nodes(or_else)?;
                    new_nodes.push(Node::If { test, body, or_else });
                }
                ParseNode::FunctionDef { name, params, body } => {
                    let func_node = self.prepare_function_def(name, params, body)?;
                    new_nodes.push(func_node);
                }
            }
        }
        Ok(new_nodes)
    }

    /// Prepares an expression by resolving names, transforming calls, and applying optimizations.
    ///
    /// Key transformations performed:
    /// - Name lookups are resolved to namespace indices via `get_id`
    /// - Function calls are resolved from identifiers to builtin types
    /// - Attribute calls validate that the object is already defined (not a new name)
    /// - Lists and tuples are recursively prepared
    /// - Modulo equality patterns like `x % n == k` (constant right-hand side) are optimized to
    ///   `CmpOperator::ModEq`
    ///
    /// # Errors
    /// Returns a NameError if an attribute call references an undefined variable
    fn prepare_expression<'c>(&mut self, loc_expr: ExprLoc<'c>) -> Result<ExprLoc<'c>, ParseError<'c>> {
        let ExprLoc { position, expr } = loc_expr;
        let expr = match expr {
            Expr::Literal(object) => Expr::Literal(object),
            Expr::Callable(callable) => Expr::Callable(callable),
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
            Expr::Call { callable, mut args } => {
                // Prepare the arguments
                args.prepare_args(|expr| self.prepare_expression(expr))?;
                // For Name callables, resolve the identifier in the namespace
                let callable = match callable {
                    Callable::Name(ident) => {
                        let (resolved_ident, is_new) = self.get_id(ident);
                        // Unlike regular name lookups, calling requires the name to already exist.
                        // Calling an undefined variable should fail at prepare-time, not runtime.
                        if is_new {
                            let exc: ExceptionRaise =
                                SimpleException::new(ExcType::NameError, Some(resolved_ident.name.to_owned().into()))
                                    .into();
                            return Err(exc.into());
                        }
                        Callable::Name(resolved_ident)
                    }
                    // Builtins and ExcTypes are already resolved at parse time
                    other => other,
                };
                Expr::Call { callable, args }
            }
            Expr::AttrCall { object, attr, mut args } => {
                let (object, is_new) = self.get_id(object);
                // Unlike regular name lookups, attribute calls require the object to already exist.
                // Calling a method on an undefined variable should fail at prepare-time, not runtime.
                // Example: `undefined_var.method()` should raise NameError here.
                if is_new {
                    let exc: ExceptionRaise =
                        SimpleException::new(ExcType::NameError, Some(object.name.to_owned().into())).into();
                    return Err(exc.into());
                }
                args.prepare_args(|expr| self.prepare_expression(expr))?;
                Expr::AttrCall { object, attr, args }
            }
            Expr::List(elements) => {
                let expressions = elements
                    .into_iter()
                    .map(|e| self.prepare_expression(e))
                    .collect::<Result<_, ParseError<'c>>>()?;
                Expr::List(expressions)
            }
            Expr::Tuple(elements) => {
                let expressions = elements
                    .into_iter()
                    .map(|e| self.prepare_expression(e))
                    .collect::<Result<_, ParseError<'c>>>()?;
                Expr::Tuple(expressions)
            }
            Expr::Subscript { object, index } => Expr::Subscript {
                object: Box::new(self.prepare_expression(*object)?),
                index: Box::new(self.prepare_expression(*index)?),
            },
            Expr::Dict(pairs) => {
                let prepared_pairs = pairs
                    .into_iter()
                    .map(|(k, v)| Ok((self.prepare_expression(k)?, self.prepare_expression(v)?)))
                    .collect::<Result<_, ParseError<'c>>>()?;
                Expr::Dict(prepared_pairs)
            }
        };

        // Optimization: Transform `(x % n) == value` with any constant right-hand side into a
        // specialized ModEq operator.
        // This is a common pattern in competitive programming (e.g., FizzBuzz checks like `i % 3 == 0`)
        // and can be executed more efficiently with a single modulo operation + comparison
        // instead of separate modulo, then equality check.
        if let Expr::CmpOp { left, op, right } = &expr {
            if op == &CmpOperator::Eq {
                if let Expr::Literal(Literal::Int(value)) = right.expr {
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

    /// Prepares a function definition by analyzing closures and building a FunctionDef.
    fn prepare_function_def<'c>(
        &mut self,
        name: Identifier<'c>,
        params: Vec<&'c str>,
        body: Vec<ParseNode<'c>>,
    ) -> Result<Node<'c>, ParseError<'c>> {
        let (name, _) = self.get_id(name);

        // Create child preparer for function body
        let mut prepare = Prepare::new(body.len(), &params, false);
        // Prepare the function body
        let prepared_body = prepare.prepare_nodes(body)?;

        // Return the final FunctionDef node
        Ok(Node::FunctionDef(Function::new(
            name,
            params,
            prepared_body,
            prepare.namespace_size,
        )))
    }

    /// Resolves an identifier to its namespace index, creating a new entry if needed.
    ///
    /// This is the core name resolution mechanism. If the name already exists in the name_map,
    /// its existing index is returned. If it's a new name, it's added to both the name_map
    /// and namespace (as Literal::Undefined), and assigned the next available index.
    ///
    /// # Returns
    /// A tuple of (resolved Identifier with id set, whether this is a new name).
    /// The boolean is true if this name was just added to the namespace.
    fn get_id<'c>(&mut self, ident: Identifier<'c>) -> (Identifier<'c>, bool) {
        let (id, is_new) = match self.name_map.entry(ident.name.to_owned()) {
            Entry::Occupied(e) => {
                let id = e.get();
                (*id, false)
            }
            Entry::Vacant(e) => {
                // New name: allocate next index and add Undefined placeholder
                let id = self.namespace_size;
                self.namespace_size += 1;
                e.insert(id);
                (id, true)
            }
        };
        (Identifier::new_with_heap(ident.name, ident.position, id), is_new)
    }
}
