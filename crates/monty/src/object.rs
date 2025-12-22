use std::{
    fmt::{self, Write},
    hash::{Hash, Hasher},
};

use ahash::AHashSet;
use indexmap::IndexMap;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    builtins::Builtins,
    exceptions::{ExcType, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    resource::ResourceTracker,
    types::{
        bytes::{bytes_repr, Bytes},
        dict::Dict,
        list::List,
        str::{string_repr, Str},
        tuple::Tuple,
        PyTrait, Type,
    },
    value::Value,
};

/// A Python value that can be passed to or returned from the interpreter.
///
/// This is the public-facing type for Python values. It owns all its data and can be
/// freely cloned, serialized, or stored. Unlike the internal `Value` type, `PyObject`
/// does not require a heap for operations.
///
/// # Input vs Output Variants
///
/// Most variants can be used both as inputs (passed to `Executor::run()`) and outputs
/// (returned from execution). However:
/// - `Repr` is output-only: represents values that have no direct `PyObject` mapping
/// - `Exception` can be used as input (to raise) or output (when code raises)
///
/// # Hashability
///
/// Only immutable variants (`None`, `Ellipsis`, `Bool`, `Int`, `Float`, `String`, `Bytes`)
/// implement `Hash`. Attempting to hash mutable variants (`List`, `Dict`) will panic.
///
/// # JSON Serialization
///
/// `PyObject` supports JSON serialization with natural mappings:
///
/// **Bidirectional (can serialize and deserialize):**
/// - `None` ↔ JSON `null`
/// - `Bool` ↔ JSON `true`/`false`
/// - `Int` ↔ JSON integer
/// - `Float` ↔ JSON float
/// - `String` ↔ JSON string
/// - `List` ↔ JSON array
/// - `Dict` ↔ JSON object (keys must be interns)
///
/// **Output-only (serialize only, cannot deserialize from JSON):**
/// - `Ellipsis` → `{"$ellipsis": true}`
/// - `Tuple` → `{"$tuple": [...]}`
/// - `Bytes` → `{"$bytes": [...]}`
/// - `Exception` → `{"$exception": {"type": "...", "arg": "..."}}`
/// - `Repr` → `{"$repr": "..."}`
#[derive(Debug, Clone)]
pub enum PyObject {
    /// Python's `Ellipsis` singleton (`...`).
    Ellipsis,
    /// Python's `None` singleton.
    None,
    /// Python boolean (`True` or `False`).
    Bool(bool),
    /// Python integer (64-bit signed).
    Int(i64),
    /// Python float (64-bit IEEE 754).
    Float(f64),
    /// Python string (UTF-8).
    String(String),
    /// Python bytes object.
    Bytes(Vec<u8>),
    /// Python list (mutable sequence).
    List(Vec<PyObject>),
    /// Python tuple (immutable sequence).
    Tuple(Vec<PyObject>),
    /// Python dictionary (insertion-ordered mapping).
    Dict(IndexMap<PyObject, PyObject>),
    /// Python exception with type and optional message argument.
    Exception {
        /// The exception type (e.g., `ValueError`, `TypeError`).
        exc_type: ExcType,
        /// Optional string argument passed to the exception constructor.
        arg: Option<String>,
    },
    /// A Python type object (e.g., `int`, `str`, `list`).
    ///
    /// Returned by the `type()` builtin and can be compared with other types.
    Type(Type),
    /// Fallback for values that cannot be represented as other variants.
    ///
    /// Contains the `repr()` string of the original value.
    ///
    /// This is output-only and cannot be used as an input to `Executor::run()`.
    Repr(String),
    /// Represents a cycle detected during Value-to-PyObject conversion.
    ///
    /// When converting cyclic structures (e.g., `a = []; a.append(a)`), this variant
    /// is used to break the infinite recursion. Contains the type-specific placeholder
    /// string (e.g., `"[...]"` for lists, `"{...}"` for dicts).
    ///
    /// This is output-only and cannot be used as an input to `Executor::run()`.
    Cycle(HeapId, &'static str),
}

impl fmt::Display for PyObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => f.write_str(s),
            Self::Cycle(_, placeholder) => f.write_str(placeholder),
            Self::Type(t) => write!(f, "<class '{t}'>"),
            _ => self.repr_fmt(f),
        }
    }
}

impl PyObject {
    /// Converts a `Value` into a `PyObject`, properly handling reference counting.
    ///
    /// Takes ownership of the `Value`, extracts its content to create a PyObject,
    /// then properly drops the Value via `drop_with_heap` to maintain reference counting.
    ///
    /// The `interns` parameter is used to look up interned string/bytes content.
    pub(crate) fn new(value: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Self {
        let py_obj = Self::from_value(&value, heap, interns);
        value.drop_with_heap(heap);
        py_obj
    }

    /// Converts this `PyObject` into an `Value`, allocating on the heap if needed.
    ///
    /// Immediate values (None, Bool, Int, Float, Ellipsis, Exception) are created directly.
    /// Heap-allocated values (String, Bytes, List, Tuple, Dict) are allocated
    /// via the heap and wrapped in `Value::Ref`.
    ///
    /// # Errors
    /// Returns `InvalidInputError` if called on the `Repr` variant,
    /// as it is only valid as an output from code execution, not as an input.
    pub(crate) fn to_value(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Result<Value, InvalidInputError> {
        match self {
            Self::Ellipsis => Ok(Value::Ellipsis),
            Self::None => Ok(Value::None),
            Self::Bool(b) => Ok(Value::Bool(b)),
            Self::Int(i) => Ok(Value::Int(i)),
            Self::Float(f) => Ok(Value::Float(f)),
            Self::String(s) => Ok(Value::Ref(heap.allocate(HeapData::Str(Str::new(s)))?)),
            Self::Bytes(b) => Ok(Value::Ref(heap.allocate(HeapData::Bytes(Bytes::new(b)))?)),
            Self::List(items) => {
                let values: Vec<Value> = items
                    .into_iter()
                    .map(|item| item.to_value(heap, interns))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Ref(heap.allocate(HeapData::List(List::new(values)))?))
            }
            Self::Tuple(items) => {
                let values: Vec<Value> = items
                    .into_iter()
                    .map(|item| item.to_value(heap, interns))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Ref(heap.allocate(HeapData::Tuple(Tuple::new(values)))?))
            }
            Self::Dict(map) => {
                let pairs: Result<Vec<(Value, Value)>, InvalidInputError> = map
                    .into_iter()
                    .map(|(k, v)| Ok((k.to_value(heap, interns)?, v.to_value(heap, interns)?)))
                    .collect();
                // PyObject Dict keys are already validated as hashable, so unwrap is safe
                let dict =
                    Dict::from_pairs(pairs?, heap, interns).expect("PyObject Dict should only contain hashable keys");
                Ok(Value::Ref(heap.allocate(HeapData::Dict(dict))?))
            }
            Self::Exception { exc_type, arg } => {
                let exc = SimpleException::new(exc_type, arg);
                Ok(Value::Exc(exc))
            }
            Self::Repr(_) => Err(InvalidInputError::new("Repr")),
            Self::Cycle(_, _) => Err(InvalidInputError::new("Cycle")),
            Self::Type(t) => Ok(Value::Builtin(Builtins::Type(t))),
        }
    }

    fn from_value(object: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Self {
        let mut visited = AHashSet::new();
        Self::from_value_inner(object, heap, &mut visited, interns)
    }

    /// Internal helper for converting Value to PyObject with cycle detection.
    ///
    /// The `visited` set tracks HeapIds we're currently processing. When we encounter
    /// a HeapId already in the set, we've found a cycle and return `PyObject::Cycle`
    /// with an appropriate placeholder string.
    fn from_value_inner(
        object: &Value,
        heap: &Heap<impl ResourceTracker>,
        visited: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> Self {
        match object {
            Value::Undefined => panic!("Undefined found while converting to PyObject"),
            Value::Ellipsis => Self::Ellipsis,
            Value::None => Self::None,
            Value::Bool(b) => Self::Bool(*b),
            Value::Int(i) => Self::Int(*i),
            Value::Float(f) => Self::Float(*f),
            Value::InternString(string_id) => Self::String(interns.get_str(*string_id).to_owned()),
            Value::InternBytes(bytes_id) => Self::Bytes(interns.get_bytes(*bytes_id).to_owned()),
            Value::Exc(exc) => Self::Exception {
                exc_type: exc.exc_type(),
                arg: exc.arg().map(ToString::to_string),
            },
            Value::Ref(id) => {
                // Check for cycle
                if visited.contains(id) {
                    // Cycle detected - return appropriate placeholder
                    return match heap.get(*id) {
                        HeapData::List(_) => Self::Cycle(*id, "[...]"),
                        HeapData::Tuple(_) => Self::Cycle(*id, "(...)"),
                        HeapData::Dict(_) => Self::Cycle(*id, "{...}"),
                        _ => Self::Cycle(*id, "..."),
                    };
                }

                // Mark this id as being visited
                visited.insert(*id);

                let result = match heap.get(*id) {
                    HeapData::Str(s) => Self::String(s.as_str().to_owned()),
                    HeapData::Bytes(b) => Self::Bytes(b.as_slice().to_owned()),
                    HeapData::List(list) => Self::List(
                        list.as_vec()
                            .iter()
                            .map(|obj| PyObject::from_value_inner(obj, heap, visited, interns))
                            .collect(),
                    ),
                    HeapData::Tuple(tuple) => Self::Tuple(
                        tuple
                            .as_vec()
                            .iter()
                            .map(|obj| PyObject::from_value_inner(obj, heap, visited, interns))
                            .collect(),
                    ),
                    HeapData::Dict(dict) => {
                        let mut new_dict = IndexMap::with_capacity(dict.as_index_map().len());
                        for bucket in dict.as_index_map().values() {
                            for (k, v) in bucket {
                                new_dict.insert(
                                    PyObject::from_value_inner(k, heap, visited, interns),
                                    PyObject::from_value_inner(v, heap, visited, interns),
                                );
                            }
                        }
                        Self::Dict(new_dict)
                    }
                    // Cells are internal closure implementation details
                    HeapData::Cell(inner) => {
                        // Show the cell's contents
                        PyObject::from_value_inner(inner, heap, visited, interns)
                    }
                    HeapData::Closure(..) | HeapData::FunctionDefaults(..) => {
                        Self::Repr(object.py_repr(heap, interns).into_owned())
                    }
                };

                // Remove from visited set after processing
                visited.remove(id);
                result
            }
            Value::Builtin(Builtins::Type(t)) => Self::Type(*t),
            #[cfg(feature = "dec-ref-check")]
            Value::Dereferenced => panic!("Dereferenced found while converting to PyObject"),
            _ => Self::Repr(object.py_repr(heap, interns).into_owned()),
        }
    }

    /// Returns the Python `repr()` string for this value.
    ///
    /// # Panics
    /// Could panic if out of memory.
    #[must_use]
    pub fn py_repr(&self) -> String {
        let mut s = String::new();
        self.repr_fmt(&mut s).expect("Unable to format repr display value");
        s
    }

    fn repr_fmt(&self, f: &mut impl Write) -> fmt::Result {
        match self {
            Self::Ellipsis => f.write_str("Ellipsis"),
            Self::None => f.write_str("None"),
            Self::Bool(true) => f.write_str("True"),
            Self::Bool(false) => f.write_str("False"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => {
                let s = v.to_string();
                f.write_str(&s)?;
                if !s.contains('.') {
                    f.write_str(".0")?;
                }
                Ok(())
            }
            Self::String(s) => f.write_str(&string_repr(s)),
            Self::Bytes(b) => f.write_str(&bytes_repr(b)),
            Self::List(l) => {
                f.write_char('[')?;
                let mut iter = l.iter();
                if let Some(first) = iter.next() {
                    first.repr_fmt(f)?;
                    for item in iter {
                        f.write_str(", ")?;
                        item.repr_fmt(f)?;
                    }
                }
                f.write_char(']')
            }
            Self::Tuple(t) => {
                f.write_char('(')?;
                let mut iter = t.iter();
                if let Some(first) = iter.next() {
                    first.repr_fmt(f)?;
                    for item in iter {
                        f.write_str(", ")?;
                        item.repr_fmt(f)?;
                    }
                }
                f.write_char(')')
            }
            Self::Dict(d) => {
                f.write_char('{')?;
                let mut iter = d.iter();
                if let Some((k, v)) = iter.next() {
                    k.repr_fmt(f)?;
                    f.write_str(": ")?;
                    v.repr_fmt(f)?;
                    for (k, v) in iter {
                        f.write_str(", ")?;
                        k.repr_fmt(f)?;
                        f.write_str(": ")?;
                        v.repr_fmt(f)?;
                    }
                }
                f.write_char('}')
            }
            Self::Exception { exc_type, arg } => {
                let type_str: &'static str = exc_type.into();
                write!(f, "{type_str}(")?;

                if let Some(arg) = &arg {
                    f.write_str(&string_repr(arg))?;
                }
                f.write_char(')')
            }
            Self::Repr(s) => write!(f, "Repr({})", string_repr(s)),
            Self::Cycle(_, placeholder) => f.write_str(placeholder),
            Self::Type(t) => write!(f, "<class '{t}'>"),
        }
    }

    /// Returns `true` if this value is "truthy" according to Python's truth testing rules.
    ///
    /// In Python, the following values are considered falsy:
    /// - `None` and `Ellipsis`
    /// - `False`
    /// - Zero numeric values (`0`, `0.0`)
    /// - Empty sequences and collections (`""`, `b""`, `[]`, `()`, `{}`)
    ///
    /// All other values are truthy, including `Exception` and `Repr` variants.
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::None => false,
            Self::Ellipsis => true,
            Self::Bool(b) => *b,
            Self::Int(i) => *i != 0,
            Self::Float(f) => *f != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::Bytes(b) => !b.is_empty(),
            Self::List(l) => !l.is_empty(),
            Self::Tuple(t) => !t.is_empty(),
            Self::Dict(d) => !d.is_empty(),
            Self::Exception { .. } => true,
            Self::Repr(_) => true,
            Self::Cycle(_, _) => true,
            Self::Type(_) => true,
        }
    }

    /// Returns the Python type name for this value (e.g., `"int"`, `"str"`, `"list"`).
    ///
    /// These are the same names returned by Python's `type(x).__name__`.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::None => "NoneType",
            Self::Ellipsis => "ellipsis",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "str",
            Self::Bytes(_) => "bytes",
            Self::List(_) => "list",
            Self::Tuple(_) => "tuple",
            Self::Dict(_) => "dict",
            Self::Exception { .. } => "Exception",
            Self::Repr(_) => "repr",
            Self::Cycle(_, _) => "cycle",
            Self::Type(_) => "type",
        }
    }
}

impl Hash for PyObject {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash the discriminant first
        std::mem::discriminant(self).hash(state);

        match self {
            Self::Ellipsis => {}
            Self::None => {}
            Self::Bool(bool) => bool.hash(state),
            Self::Int(i64) => i64.hash(state),
            Self::Float(f64) => f64.to_bits().hash(state),
            Self::String(string) => string.hash(state),
            Self::Bytes(bytes) => bytes.hash(state),
            Self::Type(t) => {
                let type_str: &'static str = t.into();
                type_str.hash(state);
            }
            Self::Cycle(_, _) => panic!("cycle values are not hashable"),
            _ => panic!("{} python values are not hashable", self.type_name()),
        }
    }
}

impl PartialEq for PyObject {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ellipsis, Self::Ellipsis) => true,
            (Self::None, Self::None) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            // Use to_bits() for float comparison to be consistent with Hash
            (Self::Float(a), Self::Float(b)) => a.to_bits() == b.to_bits(),
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (Self::Dict(a), Self::Dict(b)) => a == b,
            (
                Self::Exception {
                    exc_type: a_type,
                    arg: a_arg,
                },
                Self::Exception {
                    exc_type: b_type,
                    arg: b_arg,
                },
            ) => a_type == b_type && a_arg == b_arg,
            (Self::Repr(a), Self::Repr(b)) => a == b,
            (Self::Cycle(a, _), Self::Cycle(b, _)) => a == b,
            (Self::Type(a), Self::Type(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for PyObject {}

impl AsRef<PyObject> for PyObject {
    fn as_ref(&self) -> &PyObject {
        self
    }
}

/// Error returned when a `PyObject` cannot be converted to the requested Rust type.
///
/// This error is returned by the `TryFrom` implementations when attempting to extract
/// a specific type from a `PyObject` that holds a different variant.
#[derive(Debug)]
pub struct ConversionError {
    /// The type name that was expected (e.g., "int", "str").
    pub expected: &'static str,
    /// The actual type name of the `PyObject` (e.g., "list", "NoneType").
    pub actual: &'static str,
}

impl ConversionError {
    /// Creates a new `ConversionError` with the expected and actual type names.
    #[must_use]
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

/// Error returned when a `PyObject` cannot be used as an input to code execution.
///
/// This can occur when:
/// - A `PyObject` variant (like `Repr`) is only valid as an output, not an input
/// - A resource limit (memory, allocations) is exceeded during conversion
#[derive(Debug, Clone)]
pub enum InvalidInputError {
    /// The input type is not valid for conversion to a runtime Value.
    InvalidType {
        /// The type name of the invalid input value
        type_name: &'static str,
    },
    /// A resource limit was exceeded during conversion.
    Resource(crate::resource::ResourceError),
}

impl InvalidInputError {
    /// Creates a new `InvalidInputError` for the given type name.
    #[must_use]
    pub fn new(type_name: &'static str) -> Self {
        Self::InvalidType { type_name }
    }
}

impl fmt::Display for InvalidInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidType { type_name } => {
                write!(f, "'{type_name}' is not a valid input value")
            }
            Self::Resource(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InvalidInputError {}

impl From<crate::resource::ResourceError> for InvalidInputError {
    fn from(err: crate::resource::ResourceError) -> Self {
        Self::Resource(err)
    }
}

/// Attempts to convert a PyObject to an i64 integer.
/// Returns an error if the object is not an Int variant.
impl TryFrom<&PyObject> for i64 {
    type Error = ConversionError;

    fn try_from(value: &PyObject) -> Result<Self, Self::Error> {
        match value {
            PyObject::Int(i) => Ok(*i),
            _ => Err(ConversionError::new("int", value.type_name())),
        }
    }
}

/// Attempts to convert a PyObject to an f64 float.
/// Returns an error if the object is not a Float or Int variant.
/// Int values are automatically converted to f64 to match python's behavior.
impl TryFrom<&PyObject> for f64 {
    type Error = ConversionError;

    fn try_from(value: &PyObject) -> Result<Self, Self::Error> {
        match value {
            PyObject::Float(f) => Ok(*f),
            PyObject::Int(i) => Ok(*i as f64),
            _ => Err(ConversionError::new("float", value.type_name())),
        }
    }
}

/// Attempts to convert a PyObject to a String.
/// Returns an error if the object is not a heap-allocated Str variant.
impl TryFrom<&PyObject> for String {
    type Error = ConversionError;

    fn try_from(value: &PyObject) -> Result<Self, Self::Error> {
        if let PyObject::String(s) = value {
            Ok(s.clone())
        } else {
            Err(ConversionError::new("str", value.type_name()))
        }
    }
}

/// Attempts to convert a `PyObject` to a bool.
/// Returns an error if the object is not a True or False variant.
/// Note: This does NOT use Python's truthiness rules (use PyObject::bool for that).
impl TryFrom<&PyObject> for bool {
    type Error = ConversionError;

    fn try_from(value: &PyObject) -> Result<Self, Self::Error> {
        match value {
            PyObject::Bool(b) => Ok(*b),
            _ => Err(ConversionError::new("bool", value.type_name())),
        }
    }
}

/// Custom JSON serialization for `PyObject`.
///
/// Serializes Python values to natural JSON representations where possible.
/// Output-only types (Ellipsis, Tuple, Bytes, Exception, Repr) use tagged objects.
impl Serialize for PyObject {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::None => serializer.serialize_none(),
            Self::Bool(b) => serializer.serialize_bool(*b),
            Self::Int(i) => serializer.serialize_i64(*i),
            Self::Float(f) => serializer.serialize_f64(*f),
            Self::String(s) => serializer.serialize_str(s),
            Self::List(items) => items.serialize(serializer),
            Self::Dict(map) => {
                // Serialize as JSON object with string keys
                let mut map_ser = serializer.serialize_map(Some(map.len()))?;
                for (k, v) in map {
                    // Extract string key or convert to repr for non-string keys
                    let key_str = match k {
                        Self::String(s) => s.clone(),
                        other => other.py_repr(),
                    };
                    map_ser.serialize_entry(&key_str, v)?;
                }
                map_ser.end()
            }
            // Output-only types use tagged format
            Self::Ellipsis => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("$ellipsis", &true)?;
                map.end()
            }
            Self::Tuple(items) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("$tuple", items)?;
                map.end()
            }
            Self::Bytes(bytes) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("$bytes", bytes)?;
                map.end()
            }
            Self::Exception { exc_type, arg } => {
                #[derive(Serialize)]
                struct ExcData<'a> {
                    r#type: &'a str,
                    arg: &'a Option<String>,
                }
                let mut map = serializer.serialize_map(Some(1))?;
                let type_str: &'static str = exc_type.into();
                map.serialize_entry("$exception", &ExcData { r#type: type_str, arg })?;
                map.end()
            }
            Self::Repr(s) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("$repr", s)?;
                map.end()
            }
            Self::Cycle(_, placeholder) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("$cycle", placeholder)?;
                map.end()
            }
            Self::Type(t) => {
                let mut map = serializer.serialize_map(Some(1))?;
                let type_str: &'static str = t.into();
                map.serialize_entry("$type", type_str)?;
                map.end()
            }
        }
    }
}

/// Custom JSON deserialization for `PyObject`.
///
/// Deserializes natural JSON values to Python types:
/// - `null` → `None`
/// - `true`/`false` → `Bool`
/// - integers → `Int`
/// - floats → `Float`
/// - strings → `String`
/// - arrays → `List`
/// - objects → `Dict` (keys become `String` variants)
///
/// Note: Tuple, Bytes, Exception, Ellipsis, and Repr cannot be deserialized from JSON.
impl<'de> Deserialize<'de> for PyObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(PyObjectVisitor)
    }
}

/// Visitor for deserializing JSON into `PyObject`.
struct PyObjectVisitor;

impl<'de> Visitor<'de> for PyObjectVisitor {
    type Value = PyObject;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON value (null, bool, number, string, array, or object)")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(PyObject::None)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(PyObject::None)
    }

    fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(PyObject::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(PyObject::Int(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        // Convert to i64 if possible, otherwise error
        i64::try_from(v)
            .map(PyObject::Int)
            .map_err(|_| de::Error::custom(format!("integer {v} is too large for i64")))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(PyObject::Float(v))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(PyObject::String(v.to_owned()))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(PyObject::String(v))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(PyObject::List(items))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut dict = IndexMap::new();
        while let Some((key, value)) = map.next_entry::<String, PyObject>()? {
            dict.insert(PyObject::String(key), value);
        }
        Ok(PyObject::Dict(dict))
    }
}
