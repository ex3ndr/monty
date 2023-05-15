use crate::object::Object;
use crate::prepare::PrepareResult;

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

/// Defined separately since these operators always return a bool
#[derive(Clone, Debug, PartialEq)]
pub enum CmpOperator {
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
}

#[derive(Debug, Clone)]
pub(crate) enum Expr<T, Funcs> {
    Constant(Object),
    Name(T),
    Call {
        func: Funcs,
        args: Vec<Expr<T, Funcs>>,
        // kwargs: Vec<(T, Expr<T, Funcs>)>,
    },
    Op {
        left: Box<Expr<T, Funcs>>,
        op: Operator,
        right: Box<Expr<T, Funcs>>,
    },
    CmpOp {
        left: Box<Expr<T, Funcs>>,
        op: CmpOperator,
        right: Box<Expr<T, Funcs>>,
    },
    #[allow(dead_code)]
    List(Vec<Expr<T, Funcs>>),
}

#[derive(Debug, Clone)]
pub(crate) enum Node<Vars, Funcs> {
    Pass,
    Expr(Expr<Vars, Funcs>),
    Assign {
        target: Vars,
        object: Box<Expr<Vars, Funcs>>,
    },
    OpAssign {
        target: Vars,
        op: Operator,
        object: Box<Expr<Vars, Funcs>>,
    },
    For {
        target: Expr<Vars, Funcs>,
        iter: Expr<Vars, Funcs>,
        body: Vec<Node<Vars, Funcs>>,
        or_else: Vec<Node<Vars, Funcs>>,
    },
    If {
        test: Expr<Vars, Funcs>,
        body: Vec<Node<Vars, Funcs>>,
        or_else: Vec<Node<Vars, Funcs>>,
    },
}

// this is a temporary hack
#[derive(Debug, Clone)]
pub(crate) enum Builtins {
    Print,
    Range,
    Len,
}

impl Builtins {
    pub fn find(name: &str) -> PrepareResult<Self> {
        match name {
            "print" => Ok(Self::Print),
            "range" => Ok(Self::Range),
            "len" => Ok(Self::Len),
            _ => Err(format!("unknown builtin: {}", name).into()),
        }
    }
}
