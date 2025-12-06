use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::fmt::{self, Write};
use std::hash::{Hash, Hasher};

use strum::{Display, EnumString, IntoStaticStr};

use crate::args::ArgObjects;
use crate::expressions::ExprLoc;
use crate::heap::HeapData;
use crate::object::{Attr, Object};
use crate::operators::{CmpOperator, Operator};
use crate::parse::CodeRange;
use crate::run::RunResult;
use crate::values::str::string_repr;
use crate::values::PyValue;
use crate::Heap;

/// Python exception types supported by the interpreter.
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `Into<&'static str>` implementations.
/// The string representation matches the variant name exactly (e.g., `ValueError` -> "ValueError").
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Hash, Display, EnumString, IntoStaticStr)]
pub enum ExcType {
    ValueError,
    TypeError,
    NameError,
    AttributeError,
    KeyError,
    IndexError,
}

impl ExcType {
    /// Creates an exception instance from an exception type and arguments.
    ///
    /// Handles exception constructors like `ValueError('message')`.
    /// Currently supports zero or one string argument.
    pub(crate) fn call<'c, 'e>(
        self,
        heap: &mut Heap<'c, 'e>,
        args: ArgObjects<'c, 'e>,
    ) -> RunResult<'c, Object<'c, 'e>> {
        match args {
            ArgObjects::Zero => Ok(Object::Exc(SimpleException::new(self, None))),
            ArgObjects::One(Object::InternString(s)) => {
                Ok(Object::Exc(SimpleException::new(self, Some(s.to_owned().into()))))
            }
            ArgObjects::One(Object::Ref(object_id)) => {
                if let HeapData::Str(s) = heap.get(object_id) {
                    Ok(Object::Exc(SimpleException::new(
                        self,
                        Some(s.as_str().to_owned().into()),
                    )))
                } else {
                    internal_err!(InternalRunError::TodoError; "Exceptions can only be called with zero or one string argument")
                }
            }
            _ => {
                internal_err!(InternalRunError::TodoError; "Exceptions can only be called with zero or one string argument")
            }
        }
    }

    #[must_use]
    pub fn attribute_error(type_str: &str, attr: &Attr) -> RunError<'static> {
        exc_fmt!(Self::AttributeError; "'{type_str}' object has no attribute '{attr}'").into()
    }

    #[must_use]
    pub fn type_error_not_sub(type_str: &str) -> RunError<'static> {
        exc_fmt!(Self::TypeError; "'{type_str}' object is not subscriptable").into()
    }

    /// Creates a TypeError for unhashable types (e.g., list, dict used as dict keys).
    ///
    /// This matches Python's error message: `TypeError: unhashable type: 'list'`
    #[must_use]
    pub fn type_error_unhashable(type_str: &str) -> RunError<'static> {
        exc_fmt!(Self::TypeError; "unhashable type: '{type_str}'").into()
    }

    /// Creates a KeyError for a missing dict key.
    ///
    /// For string keys, uses the raw string value without extra quoting.
    /// For other types, uses repr.
    #[must_use]
    pub fn key_error(key: &Object<'_, '_>, heap: &Heap<'_, '_>) -> RunError<'static> {
        let key_str = match key {
            Object::InternString(s) => (*s).to_owned(),
            Object::Ref(id) => {
                if let HeapData::Str(s) = heap.get(*id) {
                    s.as_str().to_owned()
                } else {
                    key.py_repr(heap).into_owned()
                }
            }
            _ => key.py_repr(heap).into_owned(),
        };
        SimpleException::new(Self::KeyError, Some(key_str.into())).into()
    }

    /// Creates a TypeError for when a function receives the wrong number of arguments.
    ///
    /// Matches CPython's error format exactly:
    /// - For 1 expected arg: `{name}() takes exactly one argument ({actual} given)`
    /// - For N expected args: `{name} expected {expected} arguments, got {actual}`
    ///
    /// # Arguments
    /// * `name` - The function name (e.g., "len" for builtins, "list.append" for methods)
    /// * `expected` - Number of expected arguments
    /// * `actual` - Number of arguments actually provided
    #[must_use]
    pub fn type_error_arg_count(name: &str, expected: usize, actual: usize) -> RunError<'static> {
        if expected == 1 {
            // CPython: "len() takes exactly one argument (2 given)"
            exc_fmt!(Self::TypeError; "{}() takes exactly one argument ({} given)", name, actual).into()
        } else {
            // CPython: "insert expected 2 arguments, got 1"
            exc_fmt!(Self::TypeError; "{} expected {} arguments, got {}", name, expected, actual).into()
        }
    }

    /// Creates a TypeError for when a method that takes no arguments receives some.
    ///
    /// Matches CPython's format: `{name}() takes no arguments ({actual} given)`
    ///
    /// # Arguments
    /// * `name` - The method name (e.g., "dict.keys")
    /// * `actual` - Number of arguments actually provided
    #[must_use]
    pub fn type_error_no_args(name: &str, actual: usize) -> RunError<'static> {
        // CPython: "dict.keys() takes no arguments (1 given)"
        exc_fmt!(Self::TypeError; "{}() takes no arguments ({} given)", name, actual).into()
    }

    /// Creates a TypeError for when a function receives fewer arguments than required.
    ///
    /// Matches CPython's format: `{name} expected at least {min} argument, got {actual}`
    ///
    /// # Arguments
    /// * `name` - The function name (e.g., "get", "pop")
    /// * `min` - Minimum number of required arguments
    /// * `actual` - Number of arguments actually provided
    #[must_use]
    pub fn type_error_at_least(name: &str, min: usize, actual: usize) -> RunError<'static> {
        // CPython: "get expected at least 1 argument, got 0"
        exc_fmt!(Self::TypeError; "{} expected at least {} argument, got {}", name, min, actual).into()
    }

    /// Creates a TypeError for when a function receives more arguments than allowed.
    ///
    /// Matches CPython's format: `{name} expected at most {max} arguments, got {actual}`
    ///
    /// # Arguments
    /// * `name` - The function name (e.g., "get", "pop")
    /// * `max` - Maximum number of allowed arguments
    /// * `actual` - Number of arguments actually provided
    #[must_use]
    pub fn type_error_at_most<'c>(name: &str, max: usize, actual: usize) -> RunError<'c> {
        // CPython: "get expected at most 2 arguments, got 3"
        exc_fmt!(Self::TypeError; "{} expected at most {} arguments, got {}", name, max, actual).into()
    }

    /// Creates an IndexError for list index out of range.
    ///
    /// Matches CPython's format: `IndexError('list index out of range')`
    #[must_use]
    pub fn list_index_error<'c>() -> RunError<'c> {
        exc_static!(Self::IndexError; "list index out of range").into()
    }

    /// Creates an IndexError for tuple index out of range.
    ///
    /// Matches CPython's format: `IndexError('tuple index out of range')`
    #[must_use]
    pub fn tuple_index_error<'c>() -> RunError<'c> {
        exc_static!(Self::IndexError; "tuple index out of range").into()
    }

    /// Creates a TypeError for non-integer sequence indices.
    ///
    /// Matches CPython's format: `TypeError('{type}' indices must be integers, not '{index_type}')`
    #[must_use]
    pub fn type_error_indices<'c>(type_str: &str, index_type: &str) -> RunError<'c> {
        exc_fmt!(Self::TypeError; "{} indices must be integers, not '{}'", type_str, index_type).into()
    }
}

/// Simple lightweight representation of an exception.
///
/// This is used for performance reasons for common exception patterns.
#[derive(Debug, Clone, PartialEq)]
pub struct SimpleException<'c> {
    exc_type: ExcType,
    arg: Option<Cow<'c, str>>,
}

impl fmt::Display for SimpleException<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let type_str: &'static str = self.exc_type.into();
        write!(f, "{type_str}(")?;

        if let Some(arg) = &self.arg {
            f.write_str(&string_repr(arg))?;
        }

        f.write_char(')')
    }
}

impl<'c> SimpleException<'c> {
    /// Creates a new exception with the given type and optional argument message.
    #[must_use]
    pub fn new(exc_type: ExcType, arg: Option<Cow<'c, str>>) -> Self {
        SimpleException { exc_type, arg }
    }

    pub(crate) fn type_str(&self) -> &'static str {
        self.exc_type.into()
    }

    /// Computes a hash for this exception based on its type and argument.
    ///
    /// Used when exceptions are used as dict keys (rare but supported).
    #[must_use]
    pub fn py_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.exc_type.hash(&mut hasher);
        self.arg.hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn with_frame(self, frame: StackFrame<'c>) -> ExceptionRaise<'c> {
        ExceptionRaise {
            exc: self,
            frame: Some(frame),
        }
    }

    pub(crate) fn with_position(self, position: CodeRange<'c>) -> ExceptionRaise<'c> {
        ExceptionRaise {
            exc: self,
            frame: Some(StackFrame::from_position(position)),
        }
    }

    /// Creates a TypeError for binary operator type mismatches.
    ///
    /// For `+` with str/list on the left side, uses CPython's special format:
    /// `can only concatenate {type} (not "{other}") to {type}`
    ///
    /// For other cases, uses the generic format:
    /// `unsupported operand type(s) for {op}: '{left}' and '{right}'`
    pub(crate) fn operand_type_error<T>(
        left: &ExprLoc<'c>,
        op: &Operator,
        right: &ExprLoc<'c>,
        left_type: &str,
        right_type: &str,
    ) -> RunResult<'c, T> {
        let new_position = left.position.extend(&right.position);

        // CPython uses a special message for str/list + operations
        let message = if *op == Operator::Add && (left_type == "str" || left_type == "list") {
            format!("can only concatenate {left_type} (not \"{right_type}\") to {left_type}")
        } else {
            format!("unsupported operand type(s) for {op}: '{left_type}' and '{right_type}'")
        };

        Err(SimpleException::new(ExcType::TypeError, Some(message.into()))
            .with_position(new_position)
            .into())
    }

    pub(crate) fn cmp_type_error<T>(
        left: &ExprLoc<'c>,
        op: &CmpOperator,
        right: &ExprLoc<'c>,
        left_type: &str,
        right_type: &str,
    ) -> RunResult<'c, T> {
        let new_position = left.position.extend(&right.position);

        let e =
            exc_fmt!(ExcType::TypeError; "'{op}' not supported between instances of '{left_type}' and '{right_type}'");

        Err(e.with_position(new_position).into())
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

// TODO remove this, we should always set position before creating the Err
macro_rules! exc_err_fmt {
    ($error_type:expr; $($fmt_args:tt)*) => {
        Err(crate::exceptions::exc_fmt!($error_type; $($fmt_args)*).into())
    };
}
pub(crate) use exc_err_fmt;

#[derive(Debug, Clone)]
pub struct ExceptionRaise<'c> {
    pub exc: SimpleException<'c>,
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

impl<'c> From<SimpleException<'c>> for ExceptionRaise<'c> {
    fn from(exc: SimpleException<'c>) -> Self {
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
    /// The name of the frame (function name, or None for module-level code).
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
                    f.write_str("Internal Error: accessing undefined object")
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

impl<'c> From<SimpleException<'c>> for RunError<'c> {
    fn from(exc: SimpleException<'c>) -> Self {
        Self::Exc(exc.into())
    }
}
