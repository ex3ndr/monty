use std::fmt;

use crate::exceptions::{exc_err, ExceptionRaise};

use crate::object::Object;
use crate::parse::CodeRange;
use crate::parse_error::{ParseError, ParseResult};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Operator {
    Add,
    Sub,
    Mult,
    MatMult,
    Div,
    Mod,
    Pow,
    LShift,
    RShift,
    BitOr,
    BitXor,
    BitAnd,
    FloorDiv,
    // bool operators
    And,
    Or,
}

impl fmt::Display for Operator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Add => write!(f, "+"),
            Self::Sub => write!(f, "-"),
            Self::Mult => write!(f, "*"),
            Self::MatMult => write!(f, "@"),
            Self::Div => write!(f, "/"),
            Self::Mod => write!(f, "%"),
            Self::Pow => write!(f, "**"),
            Self::LShift => write!(f, "<<"),
            Self::RShift => write!(f, ">>"),
            Self::BitOr => write!(f, "|"),
            Self::BitXor => write!(f, "^"),
            Self::BitAnd => write!(f, "&"),
            Self::FloorDiv => write!(f, "//"),
            Self::And => write!(f, "and"),
            Self::Or => write!(f, "or"),
        }
    }
}

/// Defined separately since these operators always return a bool
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CmpOperator {
    Eq,
    NotEq,
    Lt,
    LtE,
    Gt,
    GtE,
    Is,
    IsNot,
    In,
    NotIn,
    // we should support floats too, either via a Number type, or ModEqInt and ModEqFloat
    ModEq(i64),
}

impl fmt::Display for CmpOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eq => write!(f, "=="),
            Self::NotEq => write!(f, "!="),
            Self::Lt => write!(f, "<"),
            Self::LtE => write!(f, "<="),
            Self::Gt => write!(f, ">"),
            Self::GtE => write!(f, ">="),
            Self::Is => write!(f, "is"),
            Self::IsNot => write!(f, "is not"),
            Self::In => write!(f, "in"),
            Self::NotIn => write!(f, "not in"),
            Self::ModEq(v) => write!(f, "% X == {v}"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExprLoc<'c> {
    pub position: CodeRange<'c>,
    pub expr: Expr<'c>,
}

impl<'c> fmt::Display for ExprLoc<'c> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // don't show position as that should be displayed separately
        write!(f, "{}", self.expr)
    }
}

impl<'c> ExprLoc<'c> {
    pub fn new(position: CodeRange<'c>, expr: Expr<'c>) -> Self {
        Self { position, expr }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Identifier<'c> {
    pub position: CodeRange<'c>,
    pub name: String, // TODO could this a `&'c str` or cow?
    pub id: usize,
}

impl<'c> Identifier<'c> {
    pub fn from_name(name: String, position: CodeRange<'c>) -> Self {
        Self { name, position, id: 0 }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Kwarg<'c> {
    pub key: Identifier<'c>,
    pub value: ExprLoc<'c>,
}

#[derive(Debug, Clone)]
pub(crate) enum Function<'c> {
    Builtin(Builtins),
    Ident(Identifier<'c>),
}

impl<'c> fmt::Display for Function<'c> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin(b) => write!(f, "{b}"),
            Self::Ident(i) => write!(f, "{}", i.name),
        }
    }
}

impl<'c> Function<'c> {
    /// whether the function has side effects
    pub fn side_effects(&self) -> bool {
        match self {
            Self::Builtin(b) => b.side_effects(),
            _ => true,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Expr<'c> {
    Constant(Object),
    Name(Identifier<'c>),
    Call {
        func: Function<'c>,
        args: Vec<ExprLoc<'c>>,
        kwargs: Vec<Kwarg<'c>>,
    },
    Op {
        left: Box<ExprLoc<'c>>,
        op: Operator,
        right: Box<ExprLoc<'c>>,
    },
    CmpOp {
        left: Box<ExprLoc<'c>>,
        op: CmpOperator,
        right: Box<ExprLoc<'c>>,
    },
    #[allow(dead_code)]
    List(Vec<ExprLoc<'c>>),
}

impl<'c> fmt::Display for Expr<'c> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Constant(object) => write!(f, "{}", object.repr()),
            Self::Name(identifier) => write!(f, "{}", identifier.name),
            Self::Call { func, args, kwargs } => {
                write!(f, "{func}(")?;
                for arg in args.iter() {
                    write!(f, "{arg}, ")?;
                }
                for kwarg in kwargs.iter() {
                    write!(f, "{}={}, ", kwarg.key.name, kwarg.value)?;
                }
                write!(f, ")")
            }
            Self::Op { left, op, right } => write!(f, "{left} {op} {right}"),
            Self::CmpOp { left, op, right } => write!(f, "{left} {op} {right}"),
            Self::List(list) => {
                write!(f, "[")?;
                for item in list.iter() {
                    write!(f, "{item}, ")?;
                }
                write!(f, "]")
            }
        }
    }
}

impl<'c> Expr<'c> {
    pub fn is_const(&self) -> bool {
        matches!(self, Self::Constant(_))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::Constant(Object::None))
    }

    pub fn into_object(self) -> Object {
        match self {
            Self::Constant(object) => object,
            _ => panic!("into_const can only be called on Constant expression."),
        }
    }
}

// TODO need a new AssignTo (enum of identifier, tuple) type used for "Assign" and "For"

#[derive(Debug, Clone)]
pub(crate) enum Node<'c> {
    Pass,
    Expr(ExprLoc<'c>),
    Return(ExprLoc<'c>),
    ReturnNone,
    Assign {
        target: Identifier<'c>,
        object: ExprLoc<'c>,
    },
    OpAssign {
        target: Identifier<'c>,
        op: Operator,
        object: ExprLoc<'c>,
    },
    For {
        target: Identifier<'c>,
        iter: ExprLoc<'c>,
        body: Vec<Node<'c>>,
        or_else: Vec<Node<'c>>,
    },
    If {
        test: ExprLoc<'c>,
        body: Vec<Node<'c>>,
        or_else: Vec<Node<'c>>,
    },
}

// this is a temporary hack
#[derive(Debug, Clone)]
pub(crate) enum Builtins {
    Print,
    Range,
    Len,
}

impl fmt::Display for Builtins {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Print => write!(f, "print"),
            Self::Range => write!(f, "range"),
            Self::Len => write!(f, "len"),
        }
    }
}

impl Builtins {
    pub fn find(name: &str) -> ParseResult<'static, Self> {
        match name {
            "print" => Ok(Self::Print),
            "range" => Ok(Self::Range),
            "len" => Ok(Self::Len),
            _ => exc_err!(ParseError::Internal; "unknown builtin: {name}"),
        }
    }

    /// whether the function has side effects
    pub fn side_effects(&self) -> bool {
        #[allow(clippy::match_like_matches_macro)]
        match self {
            Self::Print => true,
            _ => false,
        }
    }
}

#[derive(Debug)]
pub enum Exit<'c> {
    ReturnNone,
    Return(Object),
    // Yield(Object),
    Raise(ExceptionRaise<'c>),
}

impl<'c> fmt::Display for Exit<'c> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReturnNone => write!(f, "None"),
            Self::Return(v) => write!(f, "{v}"),
            Self::Raise(exc) => write!(f, "{exc}"),
        }
    }
}
