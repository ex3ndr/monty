use std::borrow::Cow;

use num::ToPrimitive;
use rustpython_parser::ast::{
    Boolop, Cmpop, Constant, Expr as AstExpr, ExprKind, Keyword, Operator as AstOperator, Stmt, StmtKind, TextRange,
};
use rustpython_parser::parse_program;

use crate::object::Object;
use crate::types::{CmpOperator, CodePosition, CodeRange, Expr, ExprLoc, Function, Identifier, Kwarg, Node, Operator};

pub type ParseResult<T> = Result<T, Cow<'static, str>>;

pub(crate) fn parse(code: &str, filename: &str) -> ParseResult<Vec<Node>> {
    match parse_program(code, filename) {
        Ok(ast) => Parser::new(code).parse_statements(ast),
        Err(e) => Err(format!("Parse error: {e}").into()),
    }
}

pub struct Parser {
    line_ends: Vec<usize>,
}

impl Parser {
    fn new(code: &str) -> Self {
        // position of each line in the source code, to convert indexes to line number and column number
        let mut line_ends = vec![0];
        for (i, c) in code.chars().enumerate() {
            if c == '\n' {
                line_ends.push(i);
            }
        }
        Self { line_ends }
    }

    fn parse_statements(&self, statements: Vec<Stmt>) -> ParseResult<Vec<Node>> {
        statements.into_iter().map(|f| self.parse_statement(f)).collect()
    }

    fn parse_statement(&self, statement: Stmt) -> ParseResult<Node> {
        match statement.node {
            StmtKind::FunctionDef {
                name: _,
                args: _,
                body: _,
                decorator_list: _,
                returns: _,
                type_comment: _,
            } => Err("TODO FunctionDef".into()),
            StmtKind::AsyncFunctionDef {
                name: _,
                args: _,
                body: _,
                decorator_list: _,
                returns: _,
                type_comment: _,
            } => Err("TODO AsyncFunctionDef".into()),
            StmtKind::ClassDef {
                name: _,
                bases: _,
                keywords: _,
                body: _,
                decorator_list: _,
            } => Err("TODO ClassDef".into()),
            StmtKind::Return { value: _ } => Err("TODO Return".into()),
            StmtKind::Delete { targets: _ } => Err("TODO Delete".into()),
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
            } => {
                let target = self.parse_expression(*target)?;
                let iter = self.parse_expression(*iter)?;
                let body = self.parse_statements(body)?;
                let or_else = self.parse_statements(orelse)?;
                Ok(Node::For {
                    target,
                    iter,
                    body,
                    or_else,
                })
            }
            StmtKind::AsyncFor {
                target: _,
                iter: _,
                body: _,
                orelse: _,
                type_comment: _,
            } => Err("TODO AsyncFor".into()),
            StmtKind::While {
                test: _,
                body: _,
                orelse: _,
            } => Err("TODO While".into()),
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
            } => Err("TODO With".into()),
            StmtKind::AsyncWith {
                items: _,
                body: _,
                type_comment: _,
            } => Err("TODO AsyncWith".into()),
            StmtKind::Match { subject: _, cases: _ } => Err("TODO Match".into()),
            StmtKind::Raise { exc: _, cause: _ } => Err("TODO Raise".into()),
            StmtKind::Try {
                body: _,
                handlers: _,
                orelse: _,
                finalbody: _,
            } => Err("TODO Try".into()),
            StmtKind::TryStar {
                body: _,
                handlers: _,
                orelse: _,
                finalbody: _,
            } => Err("TODO TryStar".into()),
            StmtKind::Assert { test: _, msg: _ } => Err("TODO Assert".into()),
            StmtKind::Import { names: _ } => Err("TODO Import".into()),
            StmtKind::ImportFrom {
                module: _,
                names: _,
                level: _,
            } => Err("TODO ImportFrom".into()),
            StmtKind::Global { names: _ } => Err("TODO Global".into()),
            StmtKind::Nonlocal { names: _ } => Err("TODO Nonlocal".into()),
            StmtKind::Expr { value } => Ok(Node::Expr(self.parse_expression(*value)?)),
            StmtKind::Pass => Ok(Node::Pass),
            StmtKind::Break => Err("TODO Break".into()),
            StmtKind::Continue => Err("TODO Continue".into()),
        }
    }

    /// `lhs = rhs` -> `lhs, rhs`
    fn parse_assignment(&self, lhs: AstExpr, rhs: AstExpr) -> ParseResult<Node> {
        Ok(Node::Assign {
            target: self.parse_identifier(lhs)?,
            object: self.parse_expression(rhs)?,
        })
    }

    fn parse_expression(&self, expression: AstExpr) -> ParseResult<ExprLoc> {
        let AstExpr { node, range, custom: _ } = expression;
        match node {
            ExprKind::BoolOp { op, values } => {
                if values.len() != 2 {
                    return Err("BoolOp must have 2 values".into());
                }
                let mut values = values.into_iter();
                let left = Box::new(self.parse_expression(values.next().unwrap())?);
                let right = Box::new(self.parse_expression(values.next().unwrap())?);
                Ok(ExprLoc {
                    position: self.convert_range(&range),
                    expr: Expr::Op {
                        left,
                        op: convert_bool_op(op),
                        right,
                    },
                })
            }
            ExprKind::NamedExpr { target: _, value: _ } => Err("TODO NamedExpr".into()),
            ExprKind::BinOp { left, op, right } => {
                let left = Box::new(self.parse_expression(*left)?);
                let right = Box::new(self.parse_expression(*right)?);
                Ok(ExprLoc {
                    position: self.convert_range(&range),
                    expr: Expr::Op {
                        left,
                        op: convert_op(op),
                        right,
                    },
                })
            }
            ExprKind::UnaryOp { op: _, operand: _ } => Err("TODO UnaryOp".into()),
            ExprKind::Lambda { args: _, body: _ } => Err("TODO Lambda".into()),
            ExprKind::IfExp {
                test: _,
                body: _,
                orelse: _,
            } => Err("TODO IfExp".into()),
            ExprKind::Dict { keys: _, values: _ } => Err("TODO Dict".into()),
            ExprKind::Set { elts: _ } => Err("TODO Set".into()),
            ExprKind::ListComp { elt: _, generators: _ } => Err("TODO ListComp".into()),
            ExprKind::SetComp { elt: _, generators: _ } => Err("TODO SetComp".into()),
            ExprKind::DictComp {
                key: _,
                value: _,
                generators: _,
            } => Err("TODO DictComp".into()),
            ExprKind::GeneratorExp { elt: _, generators: _ } => Err("TODO GeneratorExp".into()),
            ExprKind::Await { value: _ } => Err("TODO Await".into()),
            ExprKind::Yield { value: _ } => Err("TODO Yield".into()),
            ExprKind::YieldFrom { value: _ } => Err("TODO YieldFrom".into()),
            ExprKind::Compare { left, ops, comparators } => Ok(ExprLoc::new(
                self.convert_range(&range),
                Expr::CmpOp {
                    left: Box::new(self.parse_expression(*left)?),
                    op: convert_compare_op(first(ops)?),
                    right: Box::new(self.parse_expression(first(comparators)?)?),
                },
            )),
            ExprKind::Call { func, args, keywords } => {
                let func = Function::Ident(self.parse_identifier(*func)?);
                let args = args
                    .into_iter()
                    .map(|f| self.parse_expression(f))
                    .collect::<ParseResult<_>>()?;
                let kwargs = keywords
                    .into_iter()
                    .map(|f| self.parse_kwargs(f))
                    .collect::<ParseResult<_>>()?;
                Ok(ExprLoc::new(self.convert_range(&range), Expr::Call { func, args, kwargs }))
            }
            ExprKind::FormattedValue {
                value: _,
                conversion: _,
                format_spec: _,
            } => Err("TODO FormattedValue".into()),
            ExprKind::JoinedStr { values: _ } => Err("TODO JoinedStr".into()),
            ExprKind::Constant { value, .. } => Ok(ExprLoc::new(self.convert_range(&range), Expr::Constant(convert_const(value)?))),
            ExprKind::Attribute {
                value: _,
                attr: _,
                ctx: _,
            } => Err("TODO Attribute".into()),
            ExprKind::Subscript {
                value: _,
                slice: _,
                ctx: _,
            } => Err("TODO Subscript".into()),
            ExprKind::Starred { value: _, ctx: _ } => Err("TODO Starred".into()),
            ExprKind::Name { id, .. } => Ok(ExprLoc::new(self.convert_range(&range), Expr::Name(Identifier::from_name(id)))),
            ExprKind::List { elts: _, ctx: _ } => Err("TODO List".into()),
            ExprKind::Tuple { elts: _, ctx: _ } => Err("TODO Tuple".into()),
            ExprKind::Slice {
                lower: _,
                upper: _,
                step: _,
            } => Err("TODO Slice".into()),
        }
    }

    fn parse_kwargs(&self, kwarg: Keyword) -> ParseResult<Kwarg> {
        let key = match kwarg.node.arg {
            Some(key) => Identifier::from_name(key),
            None => return Err("kwargs with no key".into()),
        };
        let value = self.parse_expression(kwarg.node.value)?;
        Ok(Kwarg { key, value })
    }

    fn parse_identifier(&self, ast: AstExpr) -> ParseResult<Identifier> {
        match ast.node {
            ExprKind::Name { id, .. } => Ok(Identifier::from_name(id)),
            _ => Err(format!("Expected name, got {:?}", ast.node).into()),
        }
    }

    fn convert_range(&self, range: &TextRange) -> CodeRange {
        CodeRange::new(
            self.index_to_position(range.start().into()),
            self.index_to_position(range.end().into()),
        )
    }

    fn index_to_position(&self, index: usize) -> CodePosition {
        for (line, line_end) in self.line_ends.iter().enumerate() {
            if index <= *line_end {
                return CodePosition::new(line, index - self.line_ends[line - 1]);
            }
        }
        let len = self.line_ends.len();
        CodePosition::new(len, index - self.line_ends[len - 1])
    }
}

fn first<T: std::fmt::Debug>(v: Vec<T>) -> ParseResult<T> {
    if v.len() != 1 {
        Err(format!("Expected 1 element, got {} (raw: {v:?})", v.len()).into())
    } else {
        v.into_iter().next().ok_or_else(|| "Expected 1 element, got 0".into())
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

fn convert_const(c: Constant) -> ParseResult<Object> {
    let v = match c {
        Constant::None => Object::None,
        Constant::Bool(b) => match b {
            true => Object::True,
            false => Object::False,
        },
        Constant::Str(s) => Object::Str(s),
        Constant::Bytes(b) => Object::Bytes(b),
        Constant::Int(big_int) => match big_int.to_i64() {
            Some(i) => Object::Int(i),
            None => return Err(format!("int {big_int} too big").into()),
        },
        Constant::Tuple(tuple) => {
            let t = tuple.into_iter().map(convert_const).collect::<ParseResult<_>>()?;
            Object::Tuple(t)
        }
        Constant::Float(f) => Object::Float(f),
        Constant::Complex { .. } => return Err("complex constants not supported".into()),
        Constant::Ellipsis => Object::Ellipsis,
    };
    Ok(v)
}
