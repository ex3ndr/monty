use std::{borrow::Cow, fmt};

use crate::heap::HeapData;
use crate::{exceptions::ExceptionRaise, expressions::FrameExit, heap::Heap, object::Object};

#[derive(Debug)]
pub enum Exit<'c, 'h> {
    Return(Value<'h>),
    // Yield(ReturnObject<'h>),
    Raise(ExceptionRaise<'c>),
}

impl fmt::Display for Exit<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Return(v) => write!(f, "{v}"),
            Self::Raise(exc) => write!(f, "{exc}"),
        }
    }
}

impl<'c, 'h> Exit<'c, 'h> {
    pub(crate) fn new(frame_exit: FrameExit<'c>, heap: &'h Heap) -> Self {
        match frame_exit {
            FrameExit::Return(object) => Self::Return(Value { object, heap }),
            FrameExit::Raise(exc) => Self::Raise(exc),
        }
    }

    pub fn value(self) -> Result<Value<'h>, ConversionError> {
        match self {
            Self::Return(value) => Ok(value),
            Self::Raise(_) => Err(ConversionError::new("value", "raise")),
        }
    }
}

#[derive(Debug)]
pub struct Value<'h> {
    object: Object,
    heap: &'h Heap,
}

impl fmt::Display for Value<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.str())
    }
}

impl<'h> Value<'h> {
    /// User facing representation of the object, should match python's `str(object)`
    #[must_use]
    pub fn str(&self) -> Cow<'h, str> {
        self.object.str(self.heap)
    }

    /// Debug representation of the object, should match python's `repr(object)`
    #[must_use]
    pub fn repr(&self) -> Cow<'h, str> {
        self.object.repr(self.heap)
    }

    /// User facing representation of the object type, should roughly match `str(type(object))
    #[must_use]
    pub fn type_str(&self) -> &'static str {
        self.object.type_str(self.heap)
    }

    /// Checks if the return object is None
    #[must_use]
    pub fn is_none(&self) -> bool {
        matches!(self.object, Object::None)
    }

    /// Checks if the return object is Ellipsis
    #[must_use]
    pub fn is_ellipsis(&self) -> bool {
        matches!(self.object, Object::Ellipsis)
    }
}

/// Conversion error type for failed conversions from ReturnObject
#[derive(Debug)]
pub struct ConversionError {
    pub expected: &'static str,
    pub actual: &'static str,
}

impl ConversionError {
    pub fn new(expected: &'static str, actual: &'static str) -> Self {
        Self { expected, actual }
    }
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected {}, got {}", self.expected, self.actual)
    }
}

impl std::error::Error for ConversionError {}

/// Attempts to convert a ReturnObject to an i64 integer.
/// Returns an error if the object is not an Int variant.
impl TryFrom<&Value<'_>> for i64 {
    type Error = ConversionError;

    fn try_from(value: &Value<'_>) -> Result<Self, Self::Error> {
        match value.object {
            Object::Int(i) => Ok(i),
            _ => Err(ConversionError::new("int", value.type_str())),
        }
    }
}

/// Attempts to convert a ReturnObject to an f64 float.
/// Returns an error if the object is not a Float or Int variant.
/// Int values are automatically converted to f64.
impl TryFrom<&Value<'_>> for f64 {
    type Error = ConversionError;

    fn try_from(value: &Value<'_>) -> Result<Self, Self::Error> {
        match value.object {
            Object::Float(f) => Ok(f),
            Object::Int(i) => Ok(i as f64),
            _ => Err(ConversionError::new("float", value.type_str())),
        }
    }
}

/// Attempts to convert a ReturnObject to a String.
/// Returns an error if the object is not a heap-allocated Str variant.
impl TryFrom<&Value<'_>> for String {
    type Error = ConversionError;

    fn try_from(value: &Value<'_>) -> Result<Self, Self::Error> {
        if let Object::Ref(id) = value.object {
            if let HeapData::Str(s) = value.heap.get(id) {
                return Ok(s.clone());
            }
        }
        Err(ConversionError::new("str", value.type_str()))
    }
}

/// Attempts to convert a ReturnObject to a bool.
/// Returns an error if the object is not a True or False variant.
/// Note: This does NOT use Python's truthiness rules (use Object::bool for that).
impl TryFrom<&Value<'_>> for bool {
    type Error = ConversionError;

    fn try_from(value: &Value<'_>) -> Result<Self, Self::Error> {
        match value.object {
            Object::True => Ok(true),
            Object::False => Ok(false),
            _ => Err(ConversionError::new("bool", value.type_str())),
        }
    }
}
