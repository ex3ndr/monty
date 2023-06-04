use std::borrow::Cow;
use std::fmt;

use crate::object::Object;
use crate::parse::CodeRange;
use crate::run::RunResult;
use crate::types::ExprLoc;

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq)]
pub enum Exception {
    ValueError(Cow<'static, str>),
    TypeError(Cow<'static, str>),
    NameError(Cow<'static, str>),
}

impl fmt::Display for Exception {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ValueError(s) => write!(f, "{s}"),
            Self::TypeError(s) => write!(f, "{s}"),
            Self::NameError(s) => write!(f, "{s}"),
        }
    }
}

impl Exception {
    pub(crate) fn str_with_type(&self) -> String {
        format!("{}: {self}", self.type_str())
    }

    // TODO should also be replaced by ObjectType enum
    pub(crate) fn type_str(&self) -> &'static str {
        match self {
            Self::ValueError(_) => "ValueError",
            Self::TypeError(_) => "TypeError",
            Self::NameError(_) => "NameError",
        }
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
        left_object: Cow<'d, Object>,
        right_object: Cow<'d, Object>,
    ) -> RunResult<'c, T> {
        let left_type = left_object.type_str();
        let right_type = right_object.type_str();
        let new_position = left.position.extend(&right.position);
        Err(
            exc!(Exception::TypeError; "unsupported operand type(s) for {op}: '{left_type}' and '{right_type}'")
                .with_position(new_position)
                .into(),
        )
    }
}

macro_rules! exc {
    ($error_type:expr; $msg:tt) => {
        $error_type(format!($msg).into())
    };
    ($error_type:expr; $msg:tt, $( $msg_args:expr ),+ ) => {
        $error_type(format!($msg, $( $msg_args ),+).into())
    };
}
pub(crate) use exc;

macro_rules! exc_err {
    ($error_type:expr; $msg:tt) => {
        Err(crate::exceptions::exc!($error_type; $msg).into())
    };
    ($error_type:expr; $msg:tt, $( $msg_args:expr ),+ ) => {
        Err(crate::exceptions::exc!($error_type; $msg, $( $msg_args ),+).into())
    };
}
pub(crate) use exc_err;

#[derive(Debug, Clone)]
pub struct ExceptionRaise<'c> {
    pub(crate) exc: Exception,
    // first in vec is closes "bottom" frame
    pub(crate) frame: Option<StackFrame<'c>>,
}

impl<'c> fmt::Display for ExceptionRaise<'c> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref frame) = self.frame {
            writeln!(f, "Traceback (most recent call last):")?;
            write!(f, "{frame}")?;
        }
        write!(f, "{}", self.exc.str_with_type())
    }
}

impl<'c> From<Exception> for ExceptionRaise<'c> {
    fn from(exc: Exception) -> Self {
        ExceptionRaise { exc, frame: None }
    }
}

impl<'c> ExceptionRaise<'c> {
    pub(crate) fn summary(&self) -> String {
        let exc = self.exc.str_with_type();
        if let Some(ref frame) = self.frame {
            format!("({}) {exc}", frame.position)
        } else {
            format!("(<no-tb>) {exc}")
        }
    }
}

#[derive(Debug, Clone)]
pub struct StackFrame<'c> {
    pub(crate) position: CodeRange<'c>,
    pub(crate) frame_name: Option<&'c str>,
    pub(crate) parent: Option<Box<StackFrame<'c>>>,
}

impl<'c> fmt::Display for StackFrame<'c> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref parent) = self.parent {
            write!(f, "{parent}")?;
        }

        self.position.traceback(f, self.frame_name)
    }
}

impl<'c> StackFrame<'c> {
    pub(crate) fn new(position: &CodeRange<'c>, frame_name: &'c str, parent: &Option<StackFrame<'c>>) -> Self {
        Self {
            position: *position,
            frame_name: Some(frame_name),
            parent: parent.clone().map(Box::new),
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

impl fmt::Display for InternalRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(s) => write!(f, "Internal Error: {s}"),
            Self::TodoError(s) => write!(f, "Internal Error TODO: {s}"),
            Self::Undefined(s) => match s.is_empty() {
                true => write!(f, "Internal Error: accessing undefined object"),
                false => write!(f, "Internal Error: accessing undefined object `{s}`"),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub enum RunError<'c> {
    Internal(InternalRunError),
    Exc(ExceptionRaise<'c>),
}

impl<'c> fmt::Display for RunError<'c> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Internal(s) => write!(f, "{s}"),
            Self::Exc(s) => write!(f, "{s}"),
        }
    }
}

impl<'c> From<InternalRunError> for RunError<'c> {
    fn from(internal_error: InternalRunError) -> Self {
        Self::Internal(internal_error)
    }
}

impl<'c> From<ExceptionRaise<'c>> for RunError<'c> {
    fn from(exc: ExceptionRaise<'c>) -> Self {
        Self::Exc(exc)
    }
}

impl<'c> From<Exception> for RunError<'c> {
    fn from(exc: Exception) -> Self {
        Self::Exc(exc.into())
    }
}
