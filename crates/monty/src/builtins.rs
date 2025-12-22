use std::fmt::Write;
use std::str::FromStr;

use strum::{Display, EnumString, IntoStaticStr};

use crate::args::{ArgValues, KwargsValues};
use crate::exceptions::{exc_err_fmt, exc_fmt, ExcType};
use crate::RunError;

use crate::heap::{Heap, HeapData};
use crate::intern::Interns;
use crate::io::PrintWriter;
use crate::resource::ResourceTracker;
use crate::run_frame::RunResult;
use crate::types::{PyTrait, Type};
use crate::value::Value;

/// Enumerates every interpreter-native Python builtins
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `AsRef<str>` implementations.
/// All variants serialize to lowercase (e.g., `Print` -> "print").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtins {
    /// A builtin function like `print`, `len`, `type`, etc.
    Function(BuiltinsFunctions),
    /// An exception type constructor like `ValueError`, `TypeError`, etc.
    ExcType(ExcType),
    /// A type constructor like `list`, `dict`, `int`, etc.
    Type(Type),
}

impl Builtins {
    /// Calls this builtin with the given arguments.
    ///
    /// # Arguments
    /// * `heap` - The heap for allocating objects
    /// * `args` - The arguments to pass to the callable
    /// * `interns` - String storage for looking up interned names in error messages
    /// * `writer` - The writer for print output
    pub fn call(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        interns: &Interns,
        writer: &mut impl PrintWriter,
    ) -> RunResult<Value> {
        match self {
            Self::Function(b) => b.call(heap, args, interns, writer),
            Self::ExcType(exc) => exc.call(heap, args, interns),
            Self::Type(t) => t.call(heap, args, interns),
        }
    }

    /// Writes the Python repr() string for this callable to a formatter.
    pub fn py_repr_fmt<W: Write>(self, f: &mut W) -> std::fmt::Result {
        match self {
            Self::Function(b) => write!(f, "<built-in function {b}>"),
            Self::ExcType(e) => write!(f, "<class '{e}'>"),
            Self::Type(t) => write!(f, "<class '{t}'>"),
        }
    }

    /// Returns the type of this builtin.
    pub fn py_type(self) -> Type {
        match self {
            Self::Function(_) => Type::BuiltinFunction,
            Self::ExcType(_) => Type::Type,
            Self::Type(_) => Type::Type,
        }
    }
}

impl FromStr for Builtins {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Priority: BuiltinsFunctions > ExcType > Type
        if let Ok(b) = BuiltinsFunctions::from_str(s) {
            Ok(Self::Function(b))
        } else if let Ok(exc) = ExcType::from_str(s) {
            Ok(Self::ExcType(exc))
        } else if let Ok(t) = Type::from_str(s) {
            Ok(Self::Type(t))
        } else {
            Err(())
        }
    }
}

/// Enumerates every interpreter-native Python builtin functions like `print`, `len`, etc.
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `IntoStaticStr` implementations.
/// All variants serialize to lowercase (e.g., `Print` -> "print").
#[derive(Debug, Clone, Copy, Display, EnumString, IntoStaticStr, PartialEq, Eq)]
#[strum(serialize_all = "lowercase")]
pub enum BuiltinsFunctions {
    Print,
    Len,
    Repr,
    Id,
    Hash,
    Type,
    Isinstance,
}

impl BuiltinsFunctions {
    /// Executes the builtin with the provided positional arguments.
    ///
    /// The `interns` parameter provides access to interned string content for py_str and py_repr.
    /// The `writer` parameter is used for print output.
    fn call(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        interns: &Interns,
        writer: &mut impl PrintWriter,
    ) -> RunResult<Value> {
        match self {
            Self::Print => builtin_print(heap, args, interns, writer),
            Self::Len => {
                let value = args.get_one_arg("len")?;
                let result = match value.py_len(heap, interns) {
                    Some(len) => Ok(Value::Int(len as i64)),
                    None => {
                        exc_err_fmt!(ExcType::TypeError; "object of type {} has no len()", value.py_repr(heap, interns))
                    }
                };
                value.drop_with_heap(heap);
                result
            }
            Self::Repr => {
                let value = args.get_one_arg("repr")?;
                let heap_id = heap.allocate(HeapData::Str(value.py_repr(heap, interns).into_owned().into()))?;
                value.drop_with_heap(heap);
                Ok(Value::Ref(heap_id))
            }
            Self::Id => {
                let value = args.get_one_arg("id")?;
                let id = value.id();
                // For heap values, we intentionally don't drop to prevent heap slot reuse
                // which would cause id([]) == id([]) to return True (same slot reused).
                // For immediate values, dropping is a no-op since they don't use heap slots.
                // This is an acceptable trade-off: small leak for heap values passed to id(),
                // but correct semantics for value identity.
                if matches!(value, Value::Ref(_)) {
                    #[cfg(feature = "dec-ref-check")]
                    std::mem::forget(value);
                } else {
                    value.drop_with_heap(heap);
                }
                Ok(Value::Int(id as i64))
            }
            Self::Hash => {
                let value = args.get_one_arg("hash")?;
                let result = match value.py_hash_u64(heap, interns) {
                    Some(hash) => Ok(Value::Int(hash as i64)),
                    None => Err(ExcType::type_error_unhashable(value.py_type(Some(heap)))),
                };
                value.drop_with_heap(heap);
                result
            }
            Self::Type => {
                let value = args.get_one_arg("type")?;
                let type_obj = value.py_type(Some(heap));
                value.drop_with_heap(heap);
                Ok(Value::Builtin(Builtins::Type(type_obj)))
            }
            Self::Isinstance => {
                let (obj, classinfo) = args.get_two_args("isinstance")?;
                let obj_type = obj.py_type(Some(heap));

                let Ok(result) = isinstance_check(obj_type, &classinfo, heap) else {
                    obj.drop_with_heap(heap);
                    classinfo.drop_with_heap(heap);
                    return Err(ExcType::isinstance_arg2_error());
                };

                obj.drop_with_heap(heap);
                classinfo.drop_with_heap(heap);
                Ok(Value::Bool(result))
            }
        }
    }
}

/// Recursively checks if obj_type matches classinfo for isinstance().
///
/// Returns `Ok(true)` if the type matches, `Ok(false)` if it doesn't,
/// or `Err(())` if classinfo is invalid (not a type or tuple of types).
///
/// Supports:
/// - Single types: `isinstance(x, int)`
/// - Exception types: `isinstance(err, ValueError)`
/// - Nested tuples: `isinstance(x, (int, (str, bytes)))`
fn isinstance_check(obj_type: Type, classinfo: &Value, heap: &Heap<impl ResourceTracker>) -> Result<bool, ()> {
    match classinfo {
        // Single type: isinstance(x, int)
        Value::Builtin(Builtins::Type(t)) => Ok(obj_type.is_instance_of(*t)),

        // Exception type: isinstance(err, ValueError) or isinstance(err, Exception)
        Value::Builtin(Builtins::ExcType(exc)) => {
            // Exception base class matches any exception type
            if *exc == ExcType::Exception {
                Ok(matches!(obj_type, Type::Exception(_)))
            } else {
                Ok(matches!(obj_type, Type::Exception(e) if e == *exc))
            }
        }

        // Tuple of types (possibly nested): isinstance(x, (int, (str, bytes)))
        Value::Ref(id) => {
            if let HeapData::Tuple(tuple) = heap.get(*id) {
                for v in tuple.as_vec() {
                    if isinstance_check(obj_type, v, heap)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            } else {
                Err(()) // Not a tuple - invalid
            }
        }
        _ => Err(()), // Invalid classinfo
    }
}

/// Implementation of the print() builtin function.
///
/// Supports the following keyword arguments:
/// - `sep`: separator between values (default: " ")
/// - `end`: string appended after the last value (default: "\n")
/// - `flush`: whether to flush the stream (accepted but ignored)
///
/// The `file` kwarg is not supported.
fn builtin_print(
    heap: &mut Heap<impl ResourceTracker>,
    args: ArgValues,
    interns: &Interns,
    writer: &mut impl PrintWriter,
) -> RunResult<Value> {
    // Split into positional args and kwargs
    let (positional, kwargs) = args.split();

    // Extract kwargs first, consuming them - this handles cleanup on error
    let (sep, end) = match extract_print_kwargs(kwargs, heap, interns) {
        Ok(se) => se,
        Err(err) => {
            for value in positional {
                value.drop_with_heap(heap);
            }
            return Err(err);
        }
    };

    // Print positional args with separator
    let mut iter = positional.iter();
    if let Some(value) = iter.next() {
        writer.stdout_write(value.py_str(heap, interns));
        for value in iter {
            if let Some(sep) = &sep {
                writer.stdout_write(sep.as_str().into());
            } else {
                writer.stdout_push(' ');
            }
            writer.stdout_write(value.py_str(heap, interns));
        }
    }

    // Append end string
    if let Some(end) = end {
        writer.stdout_write(end.into());
    } else {
        writer.stdout_push('\n');
    }

    // Drop positional args
    for value in positional {
        value.drop_with_heap(heap);
    }

    Ok(Value::None)
}

/// Extracts sep and end kwargs from print() arguments.
///
/// Consumes the kwargs, dropping all values after extraction.
/// Returns (sep, end, error) where error is Some if a kwarg error occurred.
fn extract_print_kwargs(
    kwargs: KwargsValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(Option<String>, Option<String>)> {
    let mut sep: Option<String> = None;
    let mut end: Option<String> = None;
    let mut error: Option<RunError> = None;

    for (key, value) in kwargs {
        // If we already hit an error, just drop remaining values
        if error.is_some() {
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            continue;
        }

        let Some(keyword_name) = key.as_either_str(heap) else {
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            error = Some(exc_fmt!(ExcType::TypeError; "keywords must be strings").into());
            continue;
        };

        let key_str = keyword_name.as_str(interns);
        match key_str {
            "sep" => match extract_string_kwarg(&value, "sep", heap, interns) {
                Ok(custom_sep) => sep = custom_sep,
                Err(e) => error = Some(e),
            },
            "end" => match extract_string_kwarg(&value, "end", heap, interns) {
                Ok(custom_end) => end = custom_end,
                Err(e) => error = Some(e),
            },
            "flush" => {} // Accepted but ignored (we don't buffer output)
            "file" => {
                error = Some(exc_fmt!(ExcType::TypeError; "print() 'file' argument is not supported").into());
            }
            _ => {
                error = Some(
                    exc_fmt!(ExcType::TypeError; "'{}' is an invalid keyword argument for print()", key_str).into(),
                );
            }
        }
        key.drop_with_heap(heap);
        value.drop_with_heap(heap);
    }

    if let Some(error) = error {
        Err(error)
    } else {
        Ok((sep, end))
    }
}

/// Extracts a string value from a print() kwarg.
///
/// The kwarg can be None (returns empty string) or a string.
/// Raises TypeError for other types.
fn extract_string_kwarg(
    value: &Value,
    name: &str,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Option<String>> {
    match value {
        Value::None => return Ok(None),
        Value::InternString(string_id) => return Ok(Some(interns.get_str(*string_id).to_owned())),
        Value::Ref(id) => {
            if let HeapData::Str(s) = heap.get(*id) {
                return Ok(Some(s.as_str().to_owned()));
            }
        }
        _ => {}
    }
    exc_err_fmt!(ExcType::TypeError; "{} must be None or a string, not {}", name, value.py_type(Some(heap)))
}
