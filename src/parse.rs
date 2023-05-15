use std::borrow::Cow;

use num::ToPrimitive;
use rustpython_parser::ast::{
    Boolop, Cmpop, Constant, Expr as AstExpr, ExprKind, Keyword, Operator as AstOperator, Stmt, StmtKind,
};
use rustpython_parser::parse_program;

use crate::types::{Expr, Node, Operator, CmpOperator};
use crate::object::Object;

pub type ParseResult<T> = Result<T, Cow<'static, str>>;

type ParseNode = Node<String, String>;
type ParseExpr = Expr<String, String>;

pub(crate) fn parse(code: &str, filename: Option<&str>) -> ParseResult<Vec<ParseNode>> {
    match parse_program(code, filename.unwrap_or("code.py")) {
        Ok(ast) => {
            // dbg!(&ast);
            parse_statements(ast)
        }
        Err(e) => Err(format!("Parse error: {e}").into()),
    }
}

fn parse_statements(statements: Vec<Stmt>) -> ParseResult<Vec<ParseNode>> {
    statements.into_iter().map(|e| parse_statement(e)).collect()
}

fn parse_statement(statement: Stmt) -> ParseResult<ParseNode> {
    match statement.node {
        StmtKind::FunctionDef {
            name: _,
            args: _,
            body: _,
            decorator_list: _,
            returns: _,
            type_comment: _,
        } => todo!("FunctionDef"),
        StmtKind::AsyncFunctionDef {
            name: _,
            args: _,
            body: _,
            decorator_list: _,
            returns: _,
            type_comment: _,
        } => todo!("AsyncFunctionDef"),
        StmtKind::ClassDef {
            name: _,
            bases: _,
            keywords: _,
            body: _,
            decorator_list: _,
        } => todo!("ClassDef"),
        StmtKind::Return { value: _ } => todo!("Return"),
        StmtKind::Delete { targets: _ } => todo!("Delete"),
        StmtKind::Assign { targets, value, .. } => parse_assignment(first(targets)?, *value),
        StmtKind::AugAssign { target, op, value } => {
            Ok(Node::OpAssign {
                target: get_name(*target)?,
                op: convert_op(op),
                object: Box::new(parse_expression(*value)?),
            })
        }
        StmtKind::AnnAssign { target, value, .. } => match value {
            Some(value) => parse_assignment(*target, *value),
            None => Ok(Node::Pass),
        },
        StmtKind::For {
            target,
            iter,
            body,
            orelse,
            ..
        } => {
            let target = parse_expression(*target)?;
            let iter = parse_expression(*iter)?;
            let body = parse_statements(body)?;
            let or_else = parse_statements(orelse)?;
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
        } => todo!("AsyncFor"),
        StmtKind::While {
            test: _,
            body: _,
            orelse: _,
        } => todo!("While"),
        StmtKind::If { test, body, orelse } => {
            let test = parse_expression(*test)?;
            let body = parse_statements(body)?;
            let or_else = parse_statements(orelse)?;
            Ok(Node::If { test, body, or_else })
        }
        StmtKind::With {
            items: _,
            body: _,
            type_comment: _,
        } => todo!("With"),
        StmtKind::AsyncWith {
            items: _,
            body: _,
            type_comment: _,
        } => todo!("AsyncWith"),
        StmtKind::Match { subject: _, cases: _ } => todo!("Match"),
        StmtKind::Raise { exc: _, cause: _ } => todo!("Raise"),
        StmtKind::Try {
            body: _,
            handlers: _,
            orelse: _,
            finalbody: _,
        } => todo!("Try"),
        StmtKind::TryStar {
            body: _,
            handlers: _,
            orelse: _,
            finalbody: _,
        } => todo!("TryStar"),
        StmtKind::Assert { test: _, msg: _ } => todo!("Assert"),
        StmtKind::Import { names: _ } => todo!("Import"),
        StmtKind::ImportFrom {
            module: _,
            names: _,
            level: _,
        } => todo!("ImportFrom"),
        StmtKind::Global { names: _ } => todo!("Global"),
        StmtKind::Nonlocal { names: _ } => todo!("Nonlocal"),
        StmtKind::Expr { value } => Ok(Node::Expr(parse_expression(*value)?)),
        StmtKind::Pass => Ok(Node::Pass),
        StmtKind::Break => todo!("Break"),
        StmtKind::Continue => todo!("Continue"),
    }
}

/// `lhs = rhs` -> `lhs, rhs`
fn parse_assignment(lhs: AstExpr, rhs: AstExpr) -> ParseResult<ParseNode> {
    let target = get_name(lhs)?;
    let value = Box::new(parse_expression(rhs)?);
    Ok(Node::Assign { target, object: value })
}

fn parse_expression(expression: AstExpr) -> ParseResult<ParseExpr> {
    match expression.node {
        ExprKind::BoolOp { op, values } => {
            if values.len() != 2 {
                return Err("BoolOp must have 2 values".into());
            }
            let mut values = values.into_iter();
            let left = Box::new(parse_expression(values.next().unwrap())?);
            let right = Box::new(parse_expression(values.next().unwrap())?);
            Ok(Expr::Op {
                left,
                op: convert_bool_op(op),
                right,
            })
        }
        ExprKind::NamedExpr { target: _, value: _ } => todo!("NamedExpr"),
        ExprKind::BinOp { left, op, right } => {
            let left = Box::new(parse_expression(*left)?);
            let right = Box::new(parse_expression(*right)?);
            Ok(Expr::Op {
                left,
                op: convert_op(op),
                right,
            })
        }
        ExprKind::UnaryOp { op: _, operand: _ } => todo!("UnaryOp"),
        ExprKind::Lambda { args: _, body: _ } => todo!("Lambda"),
        ExprKind::IfExp {
            test: _,
            body: _,
            orelse: _,
        } => todo!("IfExp"),
        ExprKind::Dict { keys: _, values: _ } => todo!("Dict"),
        ExprKind::Set { elts: _ } => todo!("Set"),
        ExprKind::ListComp { elt: _, generators: _ } => todo!("ListComp"),
        ExprKind::SetComp { elt: _, generators: _ } => todo!("SetComp"),
        ExprKind::DictComp {
            key: _,
            value: _,
            generators: _,
        } => todo!("DictComp"),
        ExprKind::GeneratorExp { elt: _, generators: _ } => todo!("GeneratorExp"),
        ExprKind::Await { value: _ } => todo!("Await"),
        ExprKind::Yield { value: _ } => todo!("Yield"),
        ExprKind::YieldFrom { value: _ } => todo!("YieldFrom"),
        ExprKind::Compare { left, ops, comparators } => Ok(Expr::CmpOp {
            left: Box::new(parse_expression(*left)?),
            op: convert_compare_op(first(ops)?),
            right: Box::new(parse_expression(first(comparators)?)?),
        }),
        ExprKind::Call { func, args, keywords: _ } => {
            let func = get_name(*func)?;
            let args = args.into_iter().map(parse_expression).collect::<ParseResult<_>>()?;
            // let kwargs = keywords
            //     .into_iter()
            //     .map(parse_kwargs)
            //     .collect::<ParseResult<Vec<_>>>()?;
            // Ok(Expr::Call { func, args, kwargs })
            Ok(Expr::Call { func, args })
        }
        ExprKind::FormattedValue {
            value: _,
            conversion: _,
            format_spec: _,
        } => todo!("FormattedValue"),
        ExprKind::JoinedStr { values: _ } => todo!("JoinedStr"),
        ExprKind::Constant { value, .. } => Ok(Expr::Constant(convert_const(value)?)),
        ExprKind::Attribute {
            value: _,
            attr: _,
            ctx: _,
        } => todo!("Attribute"),
        ExprKind::Subscript {
            value: _,
            slice: _,
            ctx: _,
        } => todo!("Subscript"),
        ExprKind::Starred { value: _, ctx: _ } => todo!("Starred"),
        ExprKind::Name { id, .. } => Ok(Expr::Name(id)),
        ExprKind::List { elts: _, ctx: _ } => todo!("List"),
        ExprKind::Tuple { elts: _, ctx: _ } => todo!("Tuple"),
        ExprKind::Slice {
            lower: _,
            upper: _,
            step: _,
        } => todo!("Slice"),
    }
}

#[allow(dead_code)]
fn parse_kwargs(kwarg: Keyword) -> ParseResult<(String, ParseExpr)> {
    let key = match kwarg.node.arg {
        Some(key) => key,
        None => return Err("kwargs with no key".into()),
    };
    let value = parse_expression(kwarg.node.value)?;
    Ok((key, value))
}

fn get_name(lhs: AstExpr) -> ParseResult<String> {
    match lhs.node {
        ExprKind::Name { id, .. } => Ok(id),
        _ => Err(format!("Expected name, got {:?}", lhs.node).into()),
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
