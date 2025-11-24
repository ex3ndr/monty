use std::borrow::Cow;
use std::fmt;

use crate::expressions::ExprLoc;
use crate::object::{string_repr, Object};
use crate::parse::CodeRange;
use crate::run::RunResult;
use crate::Heap;

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExcType {
    ValueError,
    TypeError,
    NameError,
    AttributeError,
}

impl fmt::Display for ExcType {
    // TODO replace with a strum
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.str())
    }
}

impl ExcType {
    // TODO replace with a strum
    fn str(self) -> &'static str {
        match self {
            Self::ValueError => "ValueError",
            Self::TypeError => "TypeError",
            Self::NameError => "NameError",
            Self::AttributeError => "AttributeError",
        }
    }
}

/// Simple lightweight representation of an exception.
#[derive(Debug, Clone, PartialEq)]
pub struct SimpleException {
    exc_type: ExcType,
    arg: Option<Cow<'static, str>>,
}

impl fmt::Display for SimpleException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(", self.exc_type.str())?;

        if let Some(arg) = &self.arg {
            write!(f, "{}", string_repr(arg))?;
        }

        write!(f, ")")
    }
}

impl SimpleException {
    pub(crate) fn new(exc_type: ExcType, arg: Option<Cow<'static, str>>) -> Self {
        SimpleException { exc_type, arg }
    }

    pub(crate) fn type_str(&self) -> &'static str {
        self.exc_type.str()
    }

    pub(crate) fn with_frame(self, frame: StackFrame) -> ExceptionRaise {
        ExceptionRaise {
            exc: self,
            frame: Some(frame),
        }
    }

    pub(crate) fn with_position(self, position: CodeRange) -> ExceptionRaise {
        ExceptionRaise {
            exc: self,
            frame: Some(StackFrame::from_position(position)),
        }
    }

    pub(crate) fn operand_type_error<'c, 'd, T>(
        left: &'d ExprLoc<'c>,
        op: impl fmt::Display,
        right: &'d ExprLoc<'c>,
        left_object: Object,
        right_object: Object,
        heap: &Heap,
    ) -> RunResult<'c, T> {
        let left_type = left_object.type_str(heap);
        let right_type = right_object.type_str(heap);
        let new_position = left.position.extend(&right.position);
        Err(
            exc_fmt!(ExcType::TypeError; "unsupported operand type(s) for {op}: '{left_type}' and '{right_type}'")
                .with_position(new_position)
                .into(),
        )
    }
}

macro_rules! exc_static {
    ($error_type:expr; $msg:expr) => {
        crate::exceptions::SimpleException::new($error_type, Some($msg.into()))
    };
}
pub(crate) use exc_static;

macro_rules! exc_fmt {
    ($error_type:expr; $($fmt_args:tt)*) => {
        crate::exceptions::SimpleException::new($error_type, Some(format!($($fmt_args)*).into()))
    };
}
pub(crate) use exc_fmt;

macro_rules! exc_err_static {
    ($error_type:expr; $msg:expr) => {
        Err(crate::exceptions::exc_static!($error_type; $msg).into())
    };
}
pub(crate) use exc_err_static;

macro_rules! exc_err_fmt {
    ($error_type:expr; $($fmt_args:tt)*) => {
        Err(crate::exceptions::exc_fmt!($error_type; $($fmt_args)*).into())
    };
}
pub(crate) use exc_err_fmt;

#[derive(Debug, Clone)]
pub struct ExceptionRaise<'c> {
    pub exc: SimpleException,
    // first in vec is closes "bottom" frame
    pub(crate) frame: Option<StackFrame<'c>>,
}

impl fmt::Display for ExceptionRaise<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref frame) = self.frame {
            writeln!(f, "Traceback (most recent call last):")?;
            write!(f, "{frame}")?;
        }
        write!(f, "{}", self.exc)
    }
}

impl From<SimpleException> for ExceptionRaise<'_> {
    fn from(exc: SimpleException) -> Self {
        ExceptionRaise { exc, frame: None }
    }
}

impl ExceptionRaise<'_> {
    pub(crate) fn summary(&self) -> String {
        if let Some(ref frame) = self.frame {
            format!("({}) {}", frame.position, self.exc)
        } else {
            format!("(<no-tb>) {}", self.exc)
        }
    }
}

#[derive(Debug, Clone)]
pub struct StackFrame<'c> {
    pub(crate) position: CodeRange<'c>,
    pub(crate) frame_name: Option<&'c str>,
    pub(crate) parent: Option<Box<StackFrame<'c>>>,
}

impl fmt::Display for StackFrame<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref parent) = self.parent {
            write!(f, "{parent}")?;
        }

        self.position.traceback(f, self.frame_name)
    }
}

impl<'c> StackFrame<'c> {
    pub(crate) fn new(position: &CodeRange<'c>, frame_name: &'c str, parent: Option<&StackFrame<'c>>) -> Self {
        Self {
            position: *position,
            frame_name: Some(frame_name),
            parent: parent.map(|parent| Box::new(parent.clone())),
        }
    }

    fn from_position(position: CodeRange<'c>) -> Self {
        Self {
            position,
            frame_name: None,
            parent: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum InternalRunError {
    Error(Cow<'static, str>),
    TodoError(Cow<'static, str>),
    // could be NameError, but we don't always have the name
    Undefined(Cow<'static, str>),
}

macro_rules! internal_error {
    ($error_type:expr; $msg:tt) => {
        $error_type(format!($msg).into())
    };
    ($error_type:expr; $msg:tt, $( $msg_args:expr ),+ ) => {
        $error_type(format!($msg, $( $msg_args ),+).into())
    };
}
pub(crate) use internal_error;

macro_rules! internal_err {
    ($error_type:expr; $msg:tt) => {
        Err(crate::exceptions::internal_error!($error_type; $msg).into())
    };
    ($error_type:expr; $msg:tt, $( $msg_args:expr ),+ ) => {
        Err(crate::exceptions::internal_error!($error_type; $msg, $( $msg_args ),+).into())
    };
}
pub(crate) use internal_err;

impl fmt::Display for InternalRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(s) => write!(f, "Internal Error: {s}"),
            Self::TodoError(s) => write!(f, "Internal Error TODO: {s}"),
            Self::Undefined(s) => {
                if s.is_empty() {
                    write!(f, "Internal Error: accessing undefined object")
                } else {
                    write!(f, "Internal Error: accessing undefined object `{s}`")
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum RunError<'c> {
    Internal(InternalRunError),
    Exc(ExceptionRaise<'c>),
}

impl fmt::Display for RunError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Internal(s) => write!(f, "{s}"),
            Self::Exc(s) => write!(f, "{s}"),
        }
    }
}

impl From<InternalRunError> for RunError<'_> {
    fn from(internal_error: InternalRunError) -> Self {
        Self::Internal(internal_error)
    }
}

impl<'c> From<ExceptionRaise<'c>> for RunError<'c> {
    fn from(exc: ExceptionRaise<'c>) -> Self {
        Self::Exc(exc)
    }
}

impl From<SimpleException> for RunError<'_> {
    fn from(exc: SimpleException) -> Self {
        Self::Exc(exc.into())
    }
}
