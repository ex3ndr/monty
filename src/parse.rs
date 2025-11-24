use std::fmt;

use num::ToPrimitive;
use rustpython_parser::ast::{
    Boolop, Cmpop, Constant, Expr as AstExpr, ExprKind, Keyword, Operator as AstOperator, Stmt, StmtKind, TextRange,
};
use rustpython_parser::parse_program;

use crate::expressions::{Expr, ExprLoc, Function, Identifier, Kwarg, Node};
use crate::literal::Literal;
use crate::object::Attr;
use crate::operators::{CmpOperator, Operator};
use crate::parse_error::{ParseError, ParseResult};

pub(crate) fn parse<'c>(code: &'c str, filename: &'c str) -> ParseResult<'c, Vec<Node<'c>>> {
    match parse_program(code, filename) {
        Ok(ast) => Parser::new(code, filename).parse_statements(ast),
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

    fn parse_statements(&self, statements: Vec<Stmt>) -> ParseResult<'c, Vec<Node<'c>>> {
        statements.into_iter().map(|f| self.parse_statement(f)).collect()
    }

    fn parse_statement(&self, statement: Stmt) -> ParseResult<'c, Node<'c>> {
        match statement.node {
            StmtKind::FunctionDef {
                name: _,
                args: _,
                body: _,
                decorator_list: _,
                returns: _,
                type_comment: _,
            } => Err(ParseError::Todo("FunctionDef")),
            StmtKind::AsyncFunctionDef {
                name: _,
                args: _,
                body: _,
                decorator_list: _,
                returns: _,
                type_comment: _,
            } => Err(ParseError::Todo("AsyncFunctionDef")),
            StmtKind::ClassDef {
                name: _,
                bases: _,
                keywords: _,
                body: _,
                decorator_list: _,
            } => Err(ParseError::Todo("ClassDef")),
            StmtKind::Return { value } => match value {
                Some(value) => Ok(Node::Return(self.parse_expression(*value)?)),
                None => Ok(Node::ReturnNone),
            },
            StmtKind::Delete { targets: _ } => Err(ParseError::Todo("Delete")),
            StmtKind::Assign { targets, value, .. } => self.parse_assignment(first(targets)?, *value),
            StmtKind::AugAssign { target, op, value } => Ok(Node::OpAssign {
                target: self.parse_identifier(*target)?,
                op: convert_op(op),
                object: self.parse_expression(*value)?,
            }),
            StmtKind::AnnAssign { target, value, .. } => match value {
                Some(value) => self.parse_assignment(*target, *value),
                None => Ok(Node::Pass),
            },
            StmtKind::For {
                target,
                iter,
                body,
                orelse,
                ..
            } => Ok(Node::For {
                target: self.parse_identifier(*target)?,
                iter: self.parse_expression(*iter)?,
                body: self.parse_statements(body)?,
                or_else: self.parse_statements(orelse)?,
            }),
            StmtKind::AsyncFor {
                target: _,
                iter: _,
                body: _,
                orelse: _,
                type_comment: _,
            } => Err(ParseError::Todo("AsyncFor")),
            StmtKind::While {
                test: _,
                body: _,
                orelse: _,
            } => Err(ParseError::Todo("While")),
            StmtKind::If { test, body, orelse } => {
                let test = self.parse_expression(*test)?;
                let body = self.parse_statements(body)?;
                let or_else = self.parse_statements(orelse)?;
                Ok(Node::If { test, body, or_else })
            }
            StmtKind::With {
                items: _,
                body: _,
                type_comment: _,
            } => Err(ParseError::Todo("With")),
            StmtKind::AsyncWith {
                items: _,
                body: _,
                type_comment: _,
            } => Err(ParseError::Todo("AsyncWith")),
            StmtKind::Match { subject: _, cases: _ } => Err(ParseError::Todo("Match")),
            StmtKind::Raise { exc, cause: _ } => {
                // TODO add cause to Node::Raise
                let expr = match exc {
                    Some(expr) => Some(self.parse_expression(*expr)?),
                    None => None,
                };
                Ok(Node::Raise(expr))
            }
            StmtKind::Try {
                body: _,
                handlers: _,
                orelse: _,
                finalbody: _,
            } => Err(ParseError::Todo("Try")),
            StmtKind::TryStar {
                body: _,
                handlers: _,
                orelse: _,
                finalbody: _,
            } => Err(ParseError::Todo("TryStar")),
            StmtKind::Assert { test: _, msg: _ } => Err(ParseError::Todo("Assert")),
            StmtKind::Import { names: _ } => Err(ParseError::Todo("Import")),
            StmtKind::ImportFrom {
                module: _,
                names: _,
                level: _,
            } => Err(ParseError::Todo("ImportFrom")),
            StmtKind::Global { names: _ } => Err(ParseError::Todo("Global")),
            StmtKind::Nonlocal { names: _ } => Err(ParseError::Todo("Nonlocal")),
            StmtKind::Expr { value } => Ok(Node::Expr(self.parse_expression(*value)?)),
            StmtKind::Pass => Ok(Node::Pass),
            StmtKind::Break => Err(ParseError::Todo("Break")),
            StmtKind::Continue => Err(ParseError::Todo("Continue")),
        }
    }

    /// `lhs = rhs` -> `lhs, rhs`
    fn parse_assignment(&self, lhs: AstExpr, rhs: AstExpr) -> ParseResult<'c, Node<'c>> {
        Ok(Node::Assign {
            target: self.parse_identifier(lhs)?,
            object: self.parse_expression(rhs)?,
        })
    }

    fn parse_expression(&self, expression: AstExpr) -> ParseResult<'c, ExprLoc<'c>> {
        let AstExpr {
            node,
            range,
            custom: (),
        } = expression;
        match node {
            ExprKind::BoolOp { op, values } => {
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
            ExprKind::NamedExpr { target: _, value: _ } => Err(ParseError::Todo("NamedExpr")),
            ExprKind::BinOp { left, op, right } => {
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
            ExprKind::UnaryOp { op: _, operand: _ } => Err(ParseError::Todo("UnaryOp")),
            ExprKind::Lambda { args: _, body: _ } => Err(ParseError::Todo("Lambda")),
            ExprKind::IfExp {
                test: _,
                body: _,
                orelse: _,
            } => Err(ParseError::Todo("IfExp")),
            ExprKind::Dict { keys: _, values: _ } => Err(ParseError::Todo("Dict")),
            ExprKind::Set { elts: _ } => Err(ParseError::Todo("Set")),
            ExprKind::ListComp { elt: _, generators: _ } => Err(ParseError::Todo("ListComp")),
            ExprKind::SetComp { elt: _, generators: _ } => Err(ParseError::Todo("SetComp")),
            ExprKind::DictComp {
                key: _,
                value: _,
                generators: _,
            } => Err(ParseError::Todo("DictComp")),
            ExprKind::GeneratorExp { elt: _, generators: _ } => Err(ParseError::Todo("GeneratorExp")),
            ExprKind::Await { value: _ } => Err(ParseError::Todo("Await")),
            ExprKind::Yield { value: _ } => Err(ParseError::Todo("Yield")),
            ExprKind::YieldFrom { value: _ } => Err(ParseError::Todo("YieldFrom")),
            ExprKind::Compare { left, ops, comparators } => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::CmpOp {
                    left: Box::new(self.parse_expression(*left)?),
                    op: convert_compare_op(first(ops)?),
                    right: Box::new(self.parse_expression(first(comparators)?)?),
                },
            )),
            ExprKind::Call { func, args, keywords } => {
                let args = args
                    .into_iter()
                    .map(|f| self.parse_expression(f))
                    .collect::<ParseResult<_>>()?;
                let kwargs = keywords
                    .into_iter()
                    .map(|f| self.parse_kwargs(f))
                    .collect::<ParseResult<_>>()?;
                let position = self.convert_range(range);
                match func.node {
                    ExprKind::Name { id, .. } => {
                        let func = Identifier::from_name(id, self.convert_range(func.range));
                        let func = Function::Ident(func);
                        Ok(ExprLoc::new(position, Expr::Call { func, args, kwargs }))
                    }
                    ExprKind::Attribute { value, attr, ctx: _ } => {
                        let object = self.parse_identifier(*value)?;
                        let attr = Attr::find(&attr)?;
                        Ok(ExprLoc::new(
                            position,
                            Expr::AttrCall {
                                object,
                                attr,
                                args,
                                kwargs,
                            },
                        ))
                    }
                    _ => Err(ParseError::Internal(
                        format!("Expected name or attribute, got {:?}", func.node).into(),
                    )),
                }
            }
            ExprKind::FormattedValue {
                value: _,
                conversion: _,
                format_spec: _,
            } => Err(ParseError::Todo("FormattedValue")),
            ExprKind::JoinedStr { values: _ } => Err(ParseError::Todo("JoinedStr")),
            ExprKind::Constant { value, .. } => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::Constant(convert_const(value)?),
            )),
            ExprKind::Attribute {
                value: _,
                attr: _,
                ctx: _,
            } => Err(ParseError::Todo("Attribute")),
            ExprKind::Subscript {
                value: _,
                slice: _,
                ctx: _,
            } => Err(ParseError::Todo("Subscript")),
            ExprKind::Starred { value: _, ctx: _ } => Err(ParseError::Todo("Starred")),
            ExprKind::Name { id, .. } => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::Name(Identifier::from_name(id, self.convert_range(expression.range))),
            )),
            // TODO what is ctx here?
            ExprKind::List { elts, ctx: _ } => {
                let items = elts
                    .into_iter()
                    .map(|f| self.parse_expression(f))
                    .collect::<ParseResult<_>>()?;

                Ok(ExprLoc::new(self.convert_range(range), Expr::List(items)))
            }
            ExprKind::Tuple { elts, ctx: _ } => {
                let items = elts
                    .into_iter()
                    .map(|f| self.parse_expression(f))
                    .collect::<ParseResult<_>>()?;

                Ok(ExprLoc::new(self.convert_range(range), Expr::Tuple(items)))
            }
            ExprKind::Slice {
                lower: _,
                upper: _,
                step: _,
            } => Err(ParseError::Todo("Slice")),
        }
    }

    fn parse_kwargs(&self, kwarg: Keyword) -> ParseResult<'c, Kwarg<'c>> {
        let key = match kwarg.node.arg {
            Some(key) => Identifier::from_name(key, self.convert_range(kwarg.range)),
            None => return Err(ParseError::Todo("kwargs with no key")),
        };
        let value = self.parse_expression(kwarg.node.value)?;
        Ok(Kwarg { key, value })
    }

    fn parse_identifier(&self, ast: AstExpr) -> ParseResult<'c, Identifier<'c>> {
        match ast.node {
            ExprKind::Name { id, .. } => Ok(Identifier::from_name(id, self.convert_range(ast.range))),
            _ => Err(ParseError::Internal(
                format!("Expected name, got {:?}", ast.node).into(),
            )),
        }
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

fn first<T: fmt::Debug>(v: Vec<T>) -> ParseResult<'static, T> {
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

fn convert_bool_op(op: Boolop) -> Operator {
    match op {
        Boolop::And => Operator::And,
        Boolop::Or => Operator::Or,
    }
}

fn convert_compare_op(op: Cmpop) -> CmpOperator {
    match op {
        Cmpop::Eq => CmpOperator::Eq,
        Cmpop::NotEq => CmpOperator::NotEq,
        Cmpop::Lt => CmpOperator::Lt,
        Cmpop::LtE => CmpOperator::LtE,
        Cmpop::Gt => CmpOperator::Gt,
        Cmpop::GtE => CmpOperator::GtE,
        Cmpop::Is => CmpOperator::Is,
        Cmpop::IsNot => CmpOperator::IsNot,
        Cmpop::In => CmpOperator::In,
        Cmpop::NotIn => CmpOperator::NotIn,
    }
}

fn convert_const(c: Constant) -> ParseResult<'static, Literal> {
    let v = match c {
        Constant::None => Literal::None,
        Constant::Bool(b) => Literal::Bool(b),
        Constant::Str(s) => Literal::Str(s),
        Constant::Bytes(b) => Literal::Bytes(b),
        Constant::Int(big_int) => match big_int.to_i64() {
            Some(i) => Literal::Int(i),
            None => return Err(ParseError::Todo("BigInt Support")),
        },
        Constant::Tuple(tuple) => {
            let t = tuple.into_iter().map(convert_const).collect::<ParseResult<_>>()?;
            Literal::Tuple(t)
        }
        Constant::Float(f) => Literal::Float(f),
        Constant::Complex { .. } => return Err(ParseError::Todo("complex constants")),
        Constant::Ellipsis => Literal::Ellipsis,
    };
    Ok(v)
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
            write!(f, "{}", " ".repeat(4 - 1 + self.start.column as usize))?;
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
