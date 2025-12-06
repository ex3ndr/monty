use std::{borrow::Cow, fmt};

use ruff_python_ast::{
    self as ast, BoolOp, CmpOp, ElifElseClause, Expr as AstExpr, Keyword, Number, Operator as AstOperator, Stmt,
};
use ruff_python_parser::parse_module;
use ruff_text_size::TextRange;

use crate::args::{ArgExprs, Kwarg};
use crate::builtins::Builtins;
use crate::callable::Callable;
use crate::exceptions::ExcType;
use crate::expressions::{Expr, ExprLoc, Identifier, Literal};
use crate::operators::{CmpOperator, Operator};
use crate::parse_error::ParseError;

#[derive(Debug, Clone)]
pub(crate) enum ParseNode<'c> {
    Pass,
    Expr(ExprLoc<'c>),
    Return(ExprLoc<'c>),
    ReturnNone,
    Raise(Option<ExprLoc<'c>>),
    Assign {
        target: Identifier<'c>,
        object: ExprLoc<'c>,
    },
    OpAssign {
        target: Identifier<'c>,
        op: Operator,
        object: ExprLoc<'c>,
    },
    SubscriptAssign {
        target: Identifier<'c>,
        index: ExprLoc<'c>,
        value: ExprLoc<'c>,
    },
    For {
        target: Identifier<'c>,
        iter: ExprLoc<'c>,
        body: Vec<ParseNode<'c>>,
        or_else: Vec<ParseNode<'c>>,
    },
    If {
        test: ExprLoc<'c>,
        body: Vec<ParseNode<'c>>,
        or_else: Vec<ParseNode<'c>>,
    },
    FunctionDef {
        name: Identifier<'c>,
        params: Vec<&'c str>,
        body: Vec<ParseNode<'c>>,
    },
}

pub(crate) fn parse<'c>(code: &'c str, filename: &'c str) -> Result<Vec<ParseNode<'c>>, ParseError<'c>> {
    match parse_module(code) {
        Ok(parsed) => {
            let module = parsed.into_syntax();
            Parser::new(code, filename).parse_statements(module.body)
        }
        Err(e) => Err(ParseError::Parsing(e.to_string())),
    }
}

pub(crate) struct Parser<'c> {
    line_ends: Vec<usize>,
    code: &'c str,
    filename: &'c str,
}

impl<'c> Parser<'c> {
    fn new(code: &'c str, filename: &'c str) -> Self {
        // position of each line in the source code, to convert indexes to line number and column number
        let mut line_ends = vec![];
        for (i, c) in code.chars().enumerate() {
            if c == '\n' {
                line_ends.push(i);
            }
        }
        Self {
            line_ends,
            code,
            filename,
        }
    }

    fn parse_statements(&self, statements: Vec<Stmt>) -> Result<Vec<ParseNode<'c>>, ParseError<'c>> {
        statements.into_iter().map(|f| self.parse_statement(f)).collect()
    }

    fn parse_elif_else_clauses(&self, clauses: Vec<ElifElseClause>) -> Result<Vec<ParseNode<'c>>, ParseError<'c>> {
        let mut tail: Vec<ParseNode<'c>> = Vec::new();
        for clause in clauses.into_iter().rev() {
            match clause.test {
                Some(test) => {
                    let test = self.parse_expression(test)?;
                    let body = self.parse_statements(clause.body)?;
                    let or_else = tail;
                    let nested = ParseNode::If { test, body, or_else };
                    tail = vec![nested];
                }
                None => {
                    tail = self.parse_statements(clause.body)?;
                }
            }
        }
        Ok(tail)
    }

    fn parse_statement(&self, statement: Stmt) -> Result<ParseNode<'c>, ParseError<'c>> {
        match statement {
            Stmt::FunctionDef(function) => {
                if function.is_async {
                    return Err(ParseError::Todo("AsyncFunctionDef"));
                }

                // Reject unsupported features
                if function.parameters.vararg.is_some() {
                    return Err(ParseError::Todo("*args"));
                } else if function.parameters.kwarg.is_some() {
                    return Err(ParseError::Todo("**kwargs"));
                } else if !function.parameters.kwonlyargs.is_empty() {
                    return Err(ParseError::Todo("keyword-only arguments"));
                } else if !function.parameters.posonlyargs.is_empty() {
                    return Err(ParseError::Todo("positional-only arguments"));
                }

                // Parse parameters - only positional without defaults
                let mut params = Vec::with_capacity(function.parameters.args.len());
                for param in &function.parameters.args {
                    // Reject default argument values
                    if param.default.is_some() {
                        return Err(ParseError::Todo("default argument values"));
                    }
                    params.push(&self.code[param.parameter.name.range]);
                }

                let name = self.identifier_from_range(function.name.range);
                // Parse function body recursively
                let body = self.parse_statements(function.body)?;

                Ok(ParseNode::FunctionDef { name, params, body })
            }
            Stmt::ClassDef(_) => Err(ParseError::Todo("ClassDef")),
            Stmt::Return(ast::StmtReturn { value, .. }) => match value {
                Some(value) => Ok(ParseNode::Return(self.parse_expression(*value)?)),
                None => Ok(ParseNode::ReturnNone),
            },
            Stmt::Delete(_) => Err(ParseError::Todo("Delete")),
            Stmt::TypeAlias(_) => Err(ParseError::Todo("TypeAlias")),
            Stmt::Assign(ast::StmtAssign { targets, value, .. }) => self.parse_assignment(first(targets)?, *value),
            Stmt::AugAssign(ast::StmtAugAssign { target, op, value, .. }) => Ok(ParseNode::OpAssign {
                target: self.parse_identifier(*target)?,
                op: convert_op(op),
                object: self.parse_expression(*value)?,
            }),
            Stmt::AnnAssign(ast::StmtAnnAssign { target, value, .. }) => match value {
                Some(value) => self.parse_assignment(*target, *value),
                None => Ok(ParseNode::Pass),
            },
            Stmt::For(ast::StmtFor {
                is_async,
                target,
                iter,
                body,
                orelse,
                ..
            }) => {
                if is_async {
                    return Err(ParseError::Todo("AsyncFor"));
                }
                Ok(ParseNode::For {
                    target: self.parse_identifier(*target)?,
                    iter: self.parse_expression(*iter)?,
                    body: self.parse_statements(body)?,
                    or_else: self.parse_statements(orelse)?,
                })
            }
            Stmt::While(_) => Err(ParseError::Todo("While")),
            Stmt::If(ast::StmtIf {
                test,
                body,
                elif_else_clauses,
                ..
            }) => {
                let test = self.parse_expression(*test)?;
                let body = self.parse_statements(body)?;
                let or_else = self.parse_elif_else_clauses(elif_else_clauses)?;
                Ok(ParseNode::If { test, body, or_else })
            }
            Stmt::With(ast::StmtWith { is_async, .. }) => {
                if is_async {
                    Err(ParseError::Todo("AsyncWith"))
                } else {
                    Err(ParseError::Todo("With"))
                }
            }
            Stmt::Match(_) => Err(ParseError::Todo("Match")),
            Stmt::Raise(ast::StmtRaise { exc, .. }) => {
                // TODO add cause to ParseNode::Raise
                let expr = match exc {
                    Some(expr) => Some(self.parse_expression(*expr)?),
                    None => None,
                };
                Ok(ParseNode::Raise(expr))
            }
            Stmt::Try(ast::StmtTry { is_star, .. }) => {
                if is_star {
                    Err(ParseError::Todo("TryStar"))
                } else {
                    Err(ParseError::Todo("Try"))
                }
            }
            Stmt::Assert(_) => Err(ParseError::Todo("Assert")),
            Stmt::Import(_) => Err(ParseError::Todo("Import")),
            Stmt::ImportFrom(_) => Err(ParseError::Todo("ImportFrom")),
            Stmt::Global(_) => Err(ParseError::Todo("Global")),
            Stmt::Nonlocal(_) => Err(ParseError::Todo("Nonlocal")),
            Stmt::Expr(ast::StmtExpr { value, .. }) => Ok(ParseNode::Expr(self.parse_expression(*value)?)),
            Stmt::Pass(_) => Ok(ParseNode::Pass),
            Stmt::Break(_) => Err(ParseError::Todo("Break")),
            Stmt::Continue(_) => Err(ParseError::Todo("Continue")),
            Stmt::IpyEscapeCommand(_) => Err(ParseError::Todo("IpyEscapeCommand")),
        }
    }

    /// `lhs = rhs` -> `lhs, rhs`
    /// Handles both simple assignments (x = value) and subscript assignments (dict[key] = value)
    fn parse_assignment(&self, lhs: AstExpr, rhs: AstExpr) -> Result<ParseNode<'c>, ParseError<'c>> {
        // Check if this is a subscript assignment like dict[key] = value
        if let AstExpr::Subscript(ast::ExprSubscript { value, slice, .. }) = lhs {
            Ok(ParseNode::SubscriptAssign {
                target: self.parse_identifier(*value)?,
                index: self.parse_expression(*slice)?,
                value: self.parse_expression(rhs)?,
            })
        } else {
            // Simple identifier assignment like x = value
            Ok(ParseNode::Assign {
                target: self.parse_identifier(lhs)?,
                object: self.parse_expression(rhs)?,
            })
        }
    }

    fn parse_expression(&self, expression: AstExpr) -> Result<ExprLoc<'c>, ParseError<'c>> {
        match expression {
            AstExpr::BoolOp(ast::ExprBoolOp { op, values, range, .. }) => {
                if values.len() != 2 {
                    return Err(ParseError::Todo("BoolOp must have 2 values"));
                }
                let mut values = values.into_iter();
                let left = Box::new(self.parse_expression(values.next().unwrap())?);
                let right = Box::new(self.parse_expression(values.next().unwrap())?);
                Ok(ExprLoc {
                    position: self.convert_range(range),
                    expr: Expr::Op {
                        left,
                        op: convert_bool_op(op),
                        right,
                    },
                })
            }
            AstExpr::Named(_) => Err(ParseError::Todo("NamedExpr")),
            AstExpr::BinOp(ast::ExprBinOp {
                left, op, right, range, ..
            }) => {
                let left = Box::new(self.parse_expression(*left)?);
                let right = Box::new(self.parse_expression(*right)?);
                Ok(ExprLoc {
                    position: self.convert_range(range),
                    expr: Expr::Op {
                        left,
                        op: convert_op(op),
                        right,
                    },
                })
            }
            AstExpr::UnaryOp(_) => Err(ParseError::Todo("UnaryOp")),
            AstExpr::Lambda(_) => Err(ParseError::Todo("Lambda")),
            AstExpr::If(_) => Err(ParseError::Todo("IfExp")),
            AstExpr::Dict(ast::ExprDict { items, range, .. }) => {
                let mut pairs = Vec::new();
                for ast::DictItem { key, value } in items {
                    // key is Option<Expr> - None represents ** unpacking which we don't support yet
                    if let Some(key_expr_ast) = key {
                        let key_expr = self.parse_expression(key_expr_ast)?;
                        let value_expr = self.parse_expression(value)?;
                        pairs.push((key_expr, value_expr));
                    } else {
                        return Err(ParseError::Todo("Dict unpacking (**kwargs)"));
                    }
                }
                Ok(ExprLoc::new(self.convert_range(range), Expr::Dict(pairs)))
            }
            AstExpr::Set(_) => Err(ParseError::Todo("Set")),
            AstExpr::ListComp(_) => Err(ParseError::Todo("ListComp")),
            AstExpr::SetComp(_) => Err(ParseError::Todo("SetComp")),
            AstExpr::DictComp(_) => Err(ParseError::Todo("DictComp")),
            AstExpr::Generator(_) => Err(ParseError::Todo("GeneratorExp")),
            AstExpr::Await(_) => Err(ParseError::Todo("Await")),
            AstExpr::Yield(_) => Err(ParseError::Todo("Yield")),
            AstExpr::YieldFrom(_) => Err(ParseError::Todo("YieldFrom")),
            AstExpr::Compare(ast::ExprCompare {
                left,
                ops,
                comparators,
                range,
                ..
            }) => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::CmpOp {
                    left: Box::new(self.parse_expression(*left)?),
                    op: convert_compare_op(first(ops.into_vec())?),
                    right: Box::new(self.parse_expression(first(comparators.into_vec())?)?),
                },
            )),
            AstExpr::Call(ast::ExprCall {
                func, arguments, range, ..
            }) => {
                let ast::Arguments { args, keywords, .. } = arguments;
                let args = args
                    .into_vec()
                    .into_iter()
                    .map(|f| self.parse_expression(f))
                    .collect::<Result<Vec<_>, ParseError<'c>>>()?;
                let kwargs = keywords
                    .into_vec()
                    .into_iter()
                    .map(|f| self.parse_kwargs(f))
                    .collect::<Result<Vec<_>, ParseError<'c>>>()?;
                let args = ArgExprs::new(args, kwargs);
                let position = self.convert_range(range);
                match *func {
                    AstExpr::Name(ast::ExprName { id, range, .. }) => {
                        let name = id.to_string();
                        // Try to resolve the name as a builtin function or exception type.
                        // If neither, treat it as a name to be looked up at runtime.
                        let callable = if let Ok(builtin) = name.parse::<Builtins>() {
                            Callable::Builtin(builtin)
                        } else if let Ok(exc_type) = name.parse::<ExcType>() {
                            Callable::ExcType(exc_type)
                        } else {
                            // Name will be looked up in the namespace at runtime
                            let ident = self.identifier_from_range(range);
                            Callable::Name(ident)
                        };
                        Ok(ExprLoc::new(position, Expr::Call { callable, args }))
                    }
                    AstExpr::Attribute(ast::ExprAttribute { value, attr, .. }) => {
                        let object = self.parse_identifier(*value)?;
                        Ok(ExprLoc::new(
                            position,
                            Expr::AttrCall {
                                object,
                                attr: attr.id().to_string().into(),
                                args,
                            },
                        ))
                    }
                    other => Err(ParseError::Internal(
                        format!("Expected name or attribute, got {other:?}").into(),
                    )),
                }
            }
            AstExpr::FString(_) => Err(ParseError::Todo("FormattedValue")),
            AstExpr::TString(_) => Err(ParseError::Todo("FormattedValue")),
            AstExpr::StringLiteral(ast::ExprStringLiteral { value, range, .. }) => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::Literal(Literal::Str(value.to_string())),
            )),
            AstExpr::BytesLiteral(ast::ExprBytesLiteral { value, range, .. }) => {
                let bytes: Cow<'_, [u8]> = Cow::from(&value);
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::Literal(Literal::Bytes(bytes.into_owned())),
                ))
            }
            AstExpr::NumberLiteral(ast::ExprNumberLiteral { value, range, .. }) => {
                let const_value = match value {
                    Number::Int(i) => match i.as_i64() {
                        Some(i) => Literal::Int(i),
                        None => return Err(ParseError::Todo("BigInt Support")),
                    },
                    Number::Float(f) => Literal::Float(f),
                    Number::Complex { .. } => return Err(ParseError::Todo("complex constants")),
                };
                Ok(ExprLoc::new(self.convert_range(range), Expr::Literal(const_value)))
            }
            AstExpr::BooleanLiteral(ast::ExprBooleanLiteral { value, range, .. }) => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::Literal(Literal::Bool(value)),
            )),
            AstExpr::NoneLiteral(ast::ExprNoneLiteral { range, .. }) => {
                Ok(ExprLoc::new(self.convert_range(range), Expr::Literal(Literal::None)))
            }
            AstExpr::EllipsisLiteral(ast::ExprEllipsisLiteral { range, .. }) => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::Literal(Literal::Ellipsis),
            )),
            AstExpr::Attribute(_) => Err(ParseError::Todo("Attribute")),
            AstExpr::Subscript(ast::ExprSubscript {
                value, slice, range, ..
            }) => {
                let object = Box::new(self.parse_expression(*value)?);
                let index = Box::new(self.parse_expression(*slice)?);
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::Subscript { object, index },
                ))
            }
            AstExpr::Starred(_) => Err(ParseError::Todo("Starred")),
            AstExpr::Name(ast::ExprName { id, range, .. }) => {
                let name = id.to_string();
                let position = self.convert_range(range);
                // Check if the name is a builtin function or exception type
                let expr = if let Ok(builtin) = name.parse::<Builtins>() {
                    Expr::Callable(Callable::Builtin(builtin))
                } else if let Ok(exc_type) = name.parse::<ExcType>() {
                    Expr::Callable(Callable::ExcType(exc_type))
                } else {
                    Expr::Name(self.identifier_from_range(range))
                };
                Ok(ExprLoc::new(position, expr))
            }
            AstExpr::List(ast::ExprList { elts, range, .. }) => {
                let items = elts
                    .into_iter()
                    .map(|f| self.parse_expression(f))
                    .collect::<Result<_, ParseError<'c>>>()?;

                Ok(ExprLoc::new(self.convert_range(range), Expr::List(items)))
            }
            AstExpr::Tuple(ast::ExprTuple { elts, range, .. }) => {
                let items = elts
                    .into_iter()
                    .map(|f| self.parse_expression(f))
                    .collect::<Result<_, ParseError<'c>>>()?;

                Ok(ExprLoc::new(self.convert_range(range), Expr::Tuple(items)))
            }
            AstExpr::Slice(_) => Err(ParseError::Todo("Slice")),
            AstExpr::IpyEscapeCommand(_) => Err(ParseError::Todo("IpyEscapeCommand")),
        }
    }

    fn parse_kwargs(&self, kwarg: Keyword) -> Result<Kwarg<'c>, ParseError<'c>> {
        let key = match kwarg.arg {
            Some(key) => self.identifier_from_range(key.range),
            None => return Err(ParseError::Todo("kwargs with no key")),
        };
        let value = self.parse_expression(kwarg.value)?;
        Ok(Kwarg { key, value })
    }

    fn parse_identifier(&self, ast: AstExpr) -> Result<Identifier<'c>, ParseError<'c>> {
        match ast {
            AstExpr::Name(ast::ExprName { range, .. }) => Ok(self.identifier_from_range(range)),
            other => Err(ParseError::Internal(format!("Expected name, got {other:?}").into())),
        }
    }

    fn identifier_from_range(&self, range: TextRange) -> Identifier<'c> {
        Identifier::new(&self.code[range], self.convert_range(range))
    }

    fn convert_range(&self, range: TextRange) -> CodeRange<'c> {
        let start = range.start().into();
        let (start_line_no, start_line_start, start_line_end) = self.index_to_position(start);
        let start = CodeLoc::new(start_line_no, start - start_line_start + 1);

        let end = range.end().into();
        let (end_line_no, end_line_start, _) = self.index_to_position(end);
        let end = CodeLoc::new(end_line_no, end - end_line_start + 1);

        let preview_line = if start_line_no == end_line_no {
            if let Some(start_line_end) = start_line_end {
                Some(&self.code[start_line_start..start_line_end])
            } else {
                Some(&self.code[start_line_start..])
            }
        } else {
            None
        };

        CodeRange::new(self.filename, start, end, preview_line)
    }

    fn index_to_position(&self, index: usize) -> (usize, usize, Option<usize>) {
        let mut line_start = 0;
        for (line_no, line_end) in self.line_ends.iter().enumerate() {
            if index <= *line_end {
                return (line_no, line_start, Some(*line_end));
            }
            line_start = *line_end + 1;
        }
        (self.line_ends.len() + 1, line_start, None)
    }
}

fn first<T: fmt::Debug>(v: Vec<T>) -> Result<T, ParseError<'static>> {
    if v.len() == 1 {
        v.into_iter()
            .next()
            .ok_or_else(|| ParseError::Internal("Expected 1 element, got 0".into()))
    } else {
        Err(ParseError::Internal(
            format!("Expected 1 element, got {} (raw: {v:?})", v.len()).into(),
        ))
    }
}

fn convert_op(op: AstOperator) -> Operator {
    match op {
        AstOperator::Add => Operator::Add,
        AstOperator::Sub => Operator::Sub,
        AstOperator::Mult => Operator::Mult,
        AstOperator::MatMult => Operator::MatMult,
        AstOperator::Div => Operator::Div,
        AstOperator::Mod => Operator::Mod,
        AstOperator::Pow => Operator::Pow,
        AstOperator::LShift => Operator::LShift,
        AstOperator::RShift => Operator::RShift,
        AstOperator::BitOr => Operator::BitOr,
        AstOperator::BitXor => Operator::BitXor,
        AstOperator::BitAnd => Operator::BitAnd,
        AstOperator::FloorDiv => Operator::FloorDiv,
    }
}

fn convert_bool_op(op: BoolOp) -> Operator {
    match op {
        BoolOp::And => Operator::And,
        BoolOp::Or => Operator::Or,
    }
}

fn convert_compare_op(op: CmpOp) -> CmpOperator {
    match op {
        CmpOp::Eq => CmpOperator::Eq,
        CmpOp::NotEq => CmpOperator::NotEq,
        CmpOp::Lt => CmpOperator::Lt,
        CmpOp::LtE => CmpOperator::LtE,
        CmpOp::Gt => CmpOperator::Gt,
        CmpOp::GtE => CmpOperator::GtE,
        CmpOp::Is => CmpOperator::Is,
        CmpOp::IsNot => CmpOperator::IsNot,
        CmpOp::In => CmpOperator::In,
        CmpOp::NotIn => CmpOperator::NotIn,
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CodeRange<'c> {
    filename: &'c str,
    preview_line: Option<&'c str>,
    start: CodeLoc,
    end: CodeLoc,
}

impl fmt::Display for CodeRange<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} to {}", self.start, self.end)
    }
}

impl<'c> CodeRange<'c> {
    fn new(filename: &'c str, start: CodeLoc, end: CodeLoc, preview_line: Option<&'c str>) -> Self {
        Self {
            filename,
            preview_line,
            start,
            end,
        }
    }

    pub fn extend(&self, end: &CodeRange<'c>) -> Self {
        Self {
            filename: self.filename,
            preview_line: if self.start.line == end.end.line {
                self.preview_line
            } else {
                None
            },
            start: self.start,
            end: end.end,
        }
    }

    pub fn traceback(&self, f: &mut fmt::Formatter<'_>, frame_name: Option<&str>) -> fmt::Result {
        if let Some(frame_name) = frame_name {
            writeln!(
                f,
                r#"  File "{}", line {}, in {frame_name}"#,
                self.filename, self.start.line
            )?;
        } else {
            writeln!(
                f,
                r#"  File "{}", line {}, in <unknown frame>"#,
                self.filename, self.start.line
            )?;
        }

        if let Some(line) = &self.preview_line {
            writeln!(f, "    {line}")?;
            f.write_str(&" ".repeat(4 - 1 + self.start.column as usize))?;
            writeln!(f, "{}", "~".repeat((self.end.column - self.start.column) as usize))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CodeLoc {
    line: u32,
    column: u32,
}

impl fmt::Display for CodeLoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.line, self.column)
    }
}

impl CodeLoc {
    fn new(line: usize, column: usize) -> Self {
        Self {
            line: line as u32,
            column: column as u32,
        }
    }
}
