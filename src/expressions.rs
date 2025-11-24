use std::fmt;

use crate::exceptions::ExceptionRaise;
use crate::literal::Literal;
use crate::object::{Attr, Object};
use crate::object_types::Types;
use crate::operators::{CmpOperator, Operator};
use crate::parse::CodeRange;

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
    Builtin(Types),
    // TODO can we remove Ident here and thereby Function?
    Ident(Identifier<'c>),
}

impl fmt::Display for Function<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin(b) => write!(f, "{b}"),
            Self::Ident(i) => write!(f, "{}", i.name),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Expr<'c> {
    Constant(Literal),
    Name(Identifier<'c>),
    Call {
        func: Function<'c>,
        args: Vec<ExprLoc<'c>>,
        kwargs: Vec<Kwarg<'c>>,
    },
    AttrCall {
        object: Identifier<'c>,
        attr: Attr,
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
    List(Vec<ExprLoc<'c>>),
    Tuple(Vec<ExprLoc<'c>>),
}

impl fmt::Display for Expr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Constant(object) => write!(f, "{}", object.repr()),
            Self::Name(identifier) => write!(f, "{}", identifier.name),
            Self::Call { func, args, kwargs } => self.print_args(f, func, args, kwargs),
            Self::AttrCall {
                object,
                attr,
                args,
                kwargs,
            } => {
                write!(f, "{}.", object.name)?;
                self.print_args(f, attr, args, kwargs)
            }
            Self::Op { left, op, right } => write!(f, "{left} {op} {right}"),
            Self::CmpOp { left, op, right } => write!(f, "{left} {op} {right}"),
            Self::List(list) => {
                write!(
                    f,
                    "[{}]",
                    list.iter().map(ToString::to_string).collect::<Vec<String>>().join(", ")
                )
            }
            Self::Tuple(tuple) => {
                write!(
                    f,
                    "({})",
                    tuple
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<String>>()
                        .join(", ")
                )
            }
        }
    }
}

impl<'c> Expr<'c> {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::Constant(Literal::None))
    }

    fn print_args(
        &self,
        f: &mut fmt::Formatter<'_>,
        func: impl fmt::Display,
        args: &[ExprLoc<'c>],
        kwargs: &[Kwarg<'c>],
    ) -> fmt::Result {
        write!(f, "{func}(")?;
        let mut pos_args = false;
        for (index, arg) in args.iter().enumerate() {
            if index == 0 {
                write!(f, "{arg}")?;
            } else {
                write!(f, ", {arg}")?;
            }
            pos_args = true;
        }
        if pos_args {
            for kwarg in kwargs {
                write!(f, ", {}={}", kwarg.key.name, kwarg.value)?;
            }
        } else {
            for (index, kwarg) in kwargs.iter().enumerate() {
                if index == 0 {
                    write!(f, "{}={}", kwarg.key.name, kwarg.value)?;
                } else {
                    write!(f, ", {}={}", kwarg.key.name, kwarg.value)?;
                }
            }
        }
        write!(f, ")")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExprLoc<'c> {
    pub position: CodeRange<'c>,
    pub expr: Expr<'c>,
}

impl fmt::Display for ExprLoc<'_> {
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

// TODO need a new AssignTo (enum of identifier, tuple) type used for "Assign" and "For"

#[derive(Debug, Clone)]
pub(crate) enum Node<'c> {
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

#[derive(Debug)]
pub enum FrameExit<'c> {
    Return(Object),
    // Yield(Object),
    #[allow(dead_code)]
    Raise(ExceptionRaise<'c>),
}
