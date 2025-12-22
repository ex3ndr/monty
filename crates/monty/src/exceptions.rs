use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::fmt::{self, Write};
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::args::ArgValues;
use crate::expressions::ExprLoc;

use crate::heap::{Heap, HeapData};
use crate::intern::{Interns, StringId};
use crate::operators::{CmpOperator, Operator};
use crate::parse::CodeRange;
use crate::resource::{ResourceError, ResourceTracker};
use crate::run_frame::RunResult;
use crate::types::str::string_repr;
use crate::types::{PyTrait, Type};
use crate::value::{Attr, Value};

/// Python exception types supported by the interpreter.
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `Into<&'static str>` implementations.
/// The string representation matches the variant name exactly (e.g., `ValueError` -> "ValueError").
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, IntoStaticStr, Serialize, Deserialize)]
pub enum ExcType {
    /// Base exception class - matches any exception in isinstance checks.
    Exception,
    AssertionError,
    ValueError,
    TypeError,
    NameError,
    AttributeError,
    KeyError,
    IndexError,
    SyntaxError,
    NotImplementedError,
    ZeroDivisionError,
    OverflowError,
}

/// Formats a list of parameter names for error messages.
///
/// Examples:
/// - `["a"]` -> `'a'`
/// - `["a", "b"]` -> `'a' and 'b'`
/// - `["a", "b", "c"]` -> `'a', 'b' and 'c'`
fn format_param_names(names: &[&str]) -> String {
    match names.len() {
        0 => String::new(),
        1 => format!("'{}'", names[0]),
        2 => format!("'{}' and '{}'", names[0], names[1]),
        _ => {
            let last = names.last().unwrap();
            let rest: Vec<_> = names[..names.len() - 1].iter().map(|n| format!("'{n}'")).collect();
            format!("{} and '{last}'", rest.join(", "))
        }
    }
}

impl ExcType {
    /// Creates an exception instance from an exception type and arguments.
    ///
    /// Handles exception constructors like `ValueError('message')`.
    /// Currently supports zero or one string argument.
    ///
    /// The `interns` parameter provides access to interned string content.
    pub(crate) fn call(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        match args {
            ArgValues::Empty => Ok(Value::Exc(SimpleException::new(self, None))),
            ArgValues::One(value) => {
                // Borrow the value to inspect its type, then clean up with drop_with_heap
                let result = match &value {
                    Value::InternString(string_id) => Ok(Value::Exc(SimpleException::new(
                        self,
                        Some(interns.get_str(*string_id).to_owned()),
                    ))),
                    Value::Ref(heap_id) => {
                        if let HeapData::Str(s) = heap.get(*heap_id) {
                            Ok(Value::Exc(SimpleException::new(self, Some(s.as_str().to_owned()))))
                        } else {
                            internal_err!(InternalRunError::TodoError; "Exceptions can only be called with zero or one string argument")
                        }
                    }
                    _ => {
                        internal_err!(InternalRunError::TodoError; "Exceptions can only be called with zero or one string argument")
                    }
                };
                // Properly clean up the value using drop_with_heap which handles dec-ref-check
                value.drop_with_heap(heap);
                result
            }
            _ => {
                // Clean up any args before returning error
                args.drop_with_heap(heap);
                internal_err!(InternalRunError::TodoError; "Exceptions can only be called with zero or one string argument")
            }
        }
    }

    #[must_use]
    pub fn attribute_error(type_: Type, attr: &Attr) -> RunError {
        exc_fmt!(Self::AttributeError; "'{type_}' object has no attribute '{attr}'").into()
    }

    #[must_use]
    pub fn type_error_not_sub(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "'{type_}' object is not subscriptable").into()
    }

    /// Creates a TypeError for item assignment on types that don't support it.
    ///
    /// Matches CPython's format: `TypeError: '{type}' object does not support item assignment`
    #[must_use]
    pub fn type_error_not_sub_assignment(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "'{type_}' object does not support item assignment").into()
    }

    /// Creates a TypeError for unhashable types (e.g., list, dict used as dict keys).
    ///
    /// This matches Python's error message: `TypeError: unhashable type: 'list'`
    #[must_use]
    pub fn type_error_unhashable(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "unhashable type: '{type_}'").into()
    }

    /// Creates a KeyError for a missing dict key.
    ///
    /// For string keys, uses the raw string value without extra quoting.
    /// For other types, uses repr.
    #[must_use]
    pub fn key_error(key: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> RunError {
        let key_str = match key {
            Value::InternString(string_id) => interns.get_str(*string_id).to_owned(),
            Value::Ref(id) => {
                if let HeapData::Str(s) = heap.get(*id) {
                    s.as_str().to_owned()
                } else {
                    key.py_repr(heap, interns).into_owned()
                }
            }
            _ => key.py_repr(heap, interns).into_owned(),
        };
        SimpleException::new(Self::KeyError, Some(key_str)).into()
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
    pub fn type_error_arg_count(name: &str, expected: usize, actual: usize) -> RunError {
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
    pub fn type_error_no_args(name: &str, actual: usize) -> RunError {
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
    pub fn type_error_at_least(name: &str, min: usize, actual: usize) -> RunError {
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
    pub fn type_error_at_most(name: &str, max: usize, actual: usize) -> RunError {
        // CPython: "get expected at most 2 arguments, got 3"
        exc_fmt!(Self::TypeError; "{} expected at most {} arguments, got {}", name, max, actual).into()
    }

    /// Creates a TypeError for missing positional arguments.
    ///
    /// Matches CPython's format: `{name}() missing {count} required positional argument(s): 'a' and 'b'`
    #[must_use]
    pub fn type_error_missing_positional_with_names(name: &str, missing_names: &[&str]) -> RunError {
        let count = missing_names.len();
        let names_str = format_param_names(missing_names);
        if count == 1 {
            exc_fmt!(Self::TypeError; "{}() missing 1 required positional argument: {}", name, names_str).into()
        } else {
            exc_fmt!(Self::TypeError; "{}() missing {} required positional arguments: {}", name, count, names_str)
                .into()
        }
    }

    /// Creates a TypeError for missing keyword-only arguments.
    ///
    /// Matches CPython's format: `{name}() missing {count} required keyword-only argument(s): 'a' and 'b'`
    #[must_use]
    pub fn type_error_missing_kwonly_with_names(name: &str, missing_names: &[&str]) -> RunError {
        let count = missing_names.len();
        let names_str = format_param_names(missing_names);
        if count == 1 {
            exc_fmt!(Self::TypeError; "{}() missing 1 required keyword-only argument: {}", name, names_str).into()
        } else {
            exc_fmt!(Self::TypeError; "{}() missing {} required keyword-only arguments: {}", name, count, names_str)
                .into()
        }
    }

    /// Creates a TypeError for too many positional arguments.
    ///
    /// Matches CPython's format:
    /// - Simple: `{name}() takes {max} positional argument(s) but {actual} were given`
    /// - With kwonly: `{name}() takes {max} positional argument(s) but {actual} positional argument(s) (and N keyword-only argument(s)) were given`
    #[must_use]
    pub fn type_error_too_many_positional(name: &str, max: usize, actual: usize, kwonly_given: usize) -> RunError {
        let takes_word = if max == 1 { "argument" } else { "arguments" };

        if kwonly_given > 0 {
            // CPython includes keyword-only args in the "given" part when present
            let given_word = if actual == 1 { "argument" } else { "arguments" };
            let kwonly_word = if kwonly_given == 1 { "argument" } else { "arguments" };
            exc_fmt!(
                Self::TypeError;
                "{}() takes {} positional {} but {} positional {} (and {} keyword-only {}) were given",
                name, max, takes_word, actual, given_word, kwonly_given, kwonly_word
            )
            .into()
        } else if max == 0 {
            exc_fmt!(Self::TypeError; "{}() takes 0 positional arguments but {} were given", name, actual).into()
        } else {
            exc_fmt!(Self::TypeError; "{}() takes {} positional {} but {} were given", name, max, takes_word, actual)
                .into()
        }
    }

    /// Creates a TypeError for positional-only parameter passed as keyword.
    ///
    /// Matches CPython's format: `{name}() got some positional-only arguments passed as keyword arguments: '{param}'`
    #[must_use]
    pub fn type_error_positional_only(name: &str, param: &str) -> RunError {
        exc_fmt!(Self::TypeError; "{}() got some positional-only arguments passed as keyword arguments: '{}'", name, param).into()
    }

    /// Creates a TypeError for duplicate argument.
    ///
    /// Matches CPython's format: `{name}() got multiple values for argument '{param}'`
    #[must_use]
    pub fn type_error_duplicate_arg(name: &str, param: &str) -> RunError {
        exc_fmt!(Self::TypeError; "{}() got multiple values for argument '{}'", name, param).into()
    }

    /// Creates a TypeError for unexpected keyword argument.
    ///
    /// Matches CPython's format: `{name}() got an unexpected keyword argument '{key}'`
    #[must_use]
    pub fn type_error_unexpected_keyword(name: &str, key: &str) -> RunError {
        exc_fmt!(Self::TypeError; "{}() got an unexpected keyword argument '{}'", name, key).into()
    }

    /// Creates a simple TypeError with a custom message.
    #[must_use]
    pub fn type_error(msg: &str) -> RunError {
        exc_fmt!(Self::TypeError; "{}", msg).into()
    }

    /// Creates a TypeError for bytes() constructor with invalid type.
    ///
    /// Matches CPython's format: `TypeError: cannot convert '{type}' object to bytes`
    #[must_use]
    pub fn type_error_bytes_init(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "cannot convert '{}' object to bytes", type_).into()
    }

    /// Creates a TypeError for calling a non-callable type.
    ///
    /// Matches CPython's format: `TypeError: cannot create '{type}' instances`
    #[must_use]
    pub fn type_error_not_callable(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "cannot create '{}' instances", type_).into()
    }

    /// Creates a TypeError for non-iterable type in list/tuple/etc constructors.
    ///
    /// Matches CPython's format: `TypeError: '{type}' object is not iterable`
    #[must_use]
    pub fn type_error_not_iterable(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "'{}' object is not iterable", type_).into()
    }

    /// Creates a TypeError for int() constructor with invalid type.
    ///
    /// Matches CPython's format: `TypeError: int() argument must be a string, a bytes-like object or a real number, not '{type}'`
    #[must_use]
    pub fn type_error_int_conversion(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "int() argument must be a string, a bytes-like object or a real number, not '{}'", type_).into()
    }

    /// Creates a TypeError for float() constructor with invalid type.
    ///
    /// Matches CPython's format: `TypeError: float() argument must be a string or a real number, not '{type}'`
    #[must_use]
    pub fn type_error_float_conversion(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "float() argument must be a string or a real number, not '{}'", type_).into()
    }

    /// Creates a ValueError for negative count in bytes().
    ///
    /// Matches CPython's format: `ValueError: negative count`
    #[must_use]
    pub fn value_error_negative_bytes_count() -> RunError {
        exc_static!(Self::ValueError; "negative count").into()
    }

    /// Creates a TypeError for isinstance() arg 2.
    ///
    /// Matches CPython's format: `TypeError: isinstance() arg 2 must be a type, a tuple of types, or a union`
    #[must_use]
    pub fn isinstance_arg2_error() -> RunError {
        exc_static!(Self::TypeError; "isinstance() arg 2 must be a type, a tuple of types, or a union").into()
    }

    /// Creates a ValueError for range() step argument being zero.
    ///
    /// Matches CPython's format: `ValueError: range() arg 3 must not be zero`
    #[must_use]
    pub fn value_error_range_step_zero() -> RunError {
        exc_static!(Self::ValueError; "range() arg 3 must not be zero").into()
    }

    /// Creates a TypeError for functions that don't accept keyword arguments.
    ///
    /// Matches CPython's format: `TypeError: {name}() takes no keyword arguments`
    #[must_use]
    pub fn type_error_no_kwargs(name: &str) -> RunError {
        exc_fmt!(Self::TypeError; "{}() takes no keyword arguments", name).into()
    }

    /// Creates an IndexError for list index out of range.
    ///
    /// Matches CPython's format: `IndexError('list index out of range')`
    #[must_use]
    pub fn list_index_error() -> RunError {
        exc_static!(Self::IndexError; "list index out of range").into()
    }

    /// Creates an IndexError for tuple index out of range.
    ///
    /// Matches CPython's format: `IndexError('tuple index out of range')`
    #[must_use]
    pub fn tuple_index_error() -> RunError {
        exc_static!(Self::IndexError; "tuple index out of range").into()
    }

    /// Creates a TypeError for non-integer sequence indices.
    ///
    /// Matches CPython's format: `TypeError('{type}' indices must be integers, not '{index_type}')`
    #[must_use]
    pub fn type_error_indices(type_str: Type, index_type: Type) -> RunError {
        exc_fmt!(Self::TypeError; "{} indices must be integers, not '{}'", type_str, index_type).into()
    }

    /// Creates a SyntaxError for using a name before the `global` declaration.
    ///
    /// Matches CPython's format: `SyntaxError: name 'x' is assigned to before global declaration`
    #[must_use]
    pub fn syntax_error_assigned_before_global(name: &str) -> SimpleException {
        exc_fmt!(Self::SyntaxError; "name '{}' is assigned to before global declaration", name)
    }

    /// Creates a SyntaxError for using a name before the `global` declaration.
    ///
    /// Matches CPython's format: `SyntaxError: name 'x' is used prior to global declaration`
    #[must_use]
    pub fn syntax_error_used_before_global(name: &str) -> SimpleException {
        exc_fmt!(Self::SyntaxError; "name '{}' is used prior to global declaration", name)
    }

    /// Creates a SyntaxError for nonlocal declaration at module level.
    ///
    /// Matches CPython's format: `SyntaxError: nonlocal declaration not allowed at module level`
    #[must_use]
    pub fn syntax_error_nonlocal_at_module() -> SimpleException {
        exc_static!(Self::SyntaxError; "nonlocal declaration not allowed at module level")
    }

    /// Creates a SyntaxError when nonlocal variable doesn't exist in enclosing scope.
    ///
    /// Matches CPython's format: `SyntaxError: no binding for nonlocal 'x' found`
    #[must_use]
    pub fn syntax_error_no_binding_nonlocal(name: &str) -> SimpleException {
        exc_fmt!(Self::SyntaxError; "no binding for nonlocal '{}' found", name)
    }

    /// Creates a SyntaxError for assigning before nonlocal declaration.
    ///
    /// Matches CPython's format: `SyntaxError: name 'x' is assigned to before nonlocal declaration`
    #[must_use]
    pub fn syntax_error_assigned_before_nonlocal(name: &str) -> SimpleException {
        exc_fmt!(Self::SyntaxError; "name '{}' is assigned to before nonlocal declaration", name)
    }

    /// Creates a SyntaxError for using a name before the nonlocal declaration.
    ///
    /// Matches CPython's format: `SyntaxError: name 'x' is used prior to nonlocal declaration`
    #[must_use]
    pub fn syntax_error_used_before_nonlocal(name: &str) -> SimpleException {
        exc_fmt!(Self::SyntaxError; "name '{}' is used prior to nonlocal declaration", name)
    }

    /// Creates a NameError for accessing a free variable (nonlocal/closure) before it's assigned.
    ///
    /// Matches CPython's format: `NameError: cannot access free variable 'x' where it is not
    /// associated with a value in enclosing scope`
    #[must_use]
    pub fn name_error_free_variable(name: &str) -> SimpleException {
        exc_fmt!(Self::NameError; "cannot access free variable '{}' where it is not associated with a value in enclosing scope", name)
    }

    /// Creates a NotImplementedError for an unimplemented Python feature.
    ///
    /// Used during parsing when encountering Python syntax that Monty doesn't yet support.
    /// The message format is: "The monty syntax parser does not yet support {feature}"
    #[must_use]
    pub fn not_implemented(feature: &str) -> SimpleException {
        exc_fmt!(Self::NotImplementedError; "The monty syntax parser does not yet support {}", feature)
    }

    /// Creates a ZeroDivisionError for true division by zero (int / int case).
    ///
    /// Matches CPython's format: `ZeroDivisionError('division by zero')`
    #[must_use]
    pub fn zero_division() -> SimpleException {
        exc_static!(Self::ZeroDivisionError; "division by zero")
    }

    /// Creates a ZeroDivisionError for float division by zero.
    ///
    /// Matches CPython's format: `ZeroDivisionError('float division by zero')`
    #[must_use]
    pub fn zero_division_float() -> SimpleException {
        exc_static!(Self::ZeroDivisionError; "float division by zero")
    }

    /// Creates a ZeroDivisionError for integer division or modulo by zero.
    ///
    /// Matches CPython's format: `ZeroDivisionError('integer division or modulo by zero')`
    #[must_use]
    pub fn zero_division_int() -> SimpleException {
        exc_static!(Self::ZeroDivisionError; "integer division or modulo by zero")
    }

    /// Creates a ZeroDivisionError for float floor division by zero.
    ///
    /// Matches CPython's format: `ZeroDivisionError('float floor division by zero')`
    #[must_use]
    pub fn zero_division_float_floor() -> SimpleException {
        exc_static!(Self::ZeroDivisionError; "float floor division by zero")
    }

    /// Creates a ZeroDivisionError for 0 raised to a negative power.
    ///
    /// Matches CPython's format: `ZeroDivisionError('0.0 cannot be raised to a negative power')`
    #[must_use]
    pub fn zero_pow_negative() -> SimpleException {
        exc_static!(Self::ZeroDivisionError; "0.0 cannot be raised to a negative power")
    }

    /// Creates an OverflowError for string/sequence repetition with count too large.
    ///
    /// Matches CPython's format: `OverflowError('cannot fit 'int' into an index-sized integer')`
    #[must_use]
    pub fn overflow_repeat_count() -> SimpleException {
        exc_static!(Self::OverflowError; "cannot fit 'int' into an index-sized integer")
    }

    /// Generates a consistent error for invalid `**kwargs` types.
    #[must_use]
    pub fn kwargs_type_error(callable_name: Option<&str>, type_: Type) -> SimpleException {
        let message = match callable_name {
            Some(name) => format!("{name}() argument after ** must be a mapping, not {type_}"),
            None => format!("argument after ** must be a mapping, not {type_}"),
        };
        SimpleException::new(ExcType::TypeError, Some(message))
    }

    /// Generates the duplicate keyword argument error.
    #[must_use]
    pub fn duplicate_kwarg_error(callable_name: Option<&str>, key: &str) -> SimpleException {
        let message = match callable_name {
            Some(name) => format!("{name}() got multiple values for keyword argument '{key}'"),
            None => format!("got multiple values for keyword argument '{key}'"),
        };
        SimpleException::new(ExcType::TypeError, Some(message))
    }
}

/// Simple lightweight representation of an exception.
///
/// This is used for performance reasons for common exception patterns.
/// Exception messages use `String` for owned storage.
#[derive(Debug, Clone, PartialEq)]
pub struct SimpleException {
    exc_type: ExcType,
    arg: Option<String>,
}

impl fmt::Display for SimpleException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.py_repr_fmt(f)
    }
}

impl SimpleException {
    /// Creates a new exception with the given type and optional argument message.
    #[must_use]
    pub fn new(exc_type: ExcType, arg: Option<String>) -> Self {
        SimpleException { exc_type, arg }
    }

    #[must_use]
    pub fn exc_type(&self) -> ExcType {
        self.exc_type
    }

    #[must_use]
    pub fn arg(&self) -> Option<&String> {
        self.arg.as_ref()
    }

    pub(crate) fn py_type(&self) -> Type {
        Type::Exception(self.exc_type)
    }

    /// Returns the exception formatted as Python would display it to the user.
    ///
    /// Format: `ExceptionType: message` (e.g., `NotImplementedError: feature not supported`)
    /// If there's no message, just returns the exception type name.
    #[must_use]
    pub fn py_str(&self) -> String {
        let type_str: &'static str = self.exc_type.into();
        match &self.arg {
            Some(arg) => format!("{type_str}: {arg}"),
            None => type_str.to_string(),
        }
    }

    /// Returns the exception formatted as Python would repr it.
    pub fn py_repr_fmt(&self, f: &mut impl Write) -> std::fmt::Result {
        let type_str: &'static str = self.exc_type.into();
        write!(f, "{type_str}(")?;

        if let Some(arg) = &self.arg {
            f.write_str(&string_repr(arg))?;
        }

        f.write_char(')')
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

    /// Creates a TypeError for binary operator type mismatches.
    ///
    /// For `+` with str/list on the left side, uses CPython's special format:
    /// `can only concatenate {type} (not "{other}") to {type}`
    ///
    /// For other cases, uses the generic format:
    /// `unsupported operand type(s) for {op}: '{left}' and '{right}'`
    pub(crate) fn operand_type_error<T>(
        left: &ExprLoc,
        op: &Operator,
        right: &ExprLoc,
        left_type: Type,
        right_type: Type,
    ) -> RunResult<T> {
        let new_position = left.position.extend(&right.position);

        // CPython uses a special message for str/list + operations
        let message = if *op == Operator::Add && (left_type == Type::Str || left_type == Type::List) {
            format!("can only concatenate {left_type} (not \"{right_type}\") to {left_type}")
        } else {
            format!("unsupported operand type(s) for {op}: '{left_type}' and '{right_type}'")
        };

        Err(SimpleException::new(ExcType::TypeError, Some(message))
            .with_position(new_position)
            .into())
    }

    pub(crate) fn cmp_type_error<T>(
        left: &ExprLoc,
        op: &CmpOperator,
        right: &ExprLoc,
        left_type: Type,
        right_type: Type,
    ) -> RunResult<T> {
        let new_position = left.position.extend(&right.position);

        let e =
            exc_fmt!(ExcType::TypeError; "'{op}' not supported between instances of '{left_type}' and '{right_type}'");

        Err(e.with_position(new_position).into())
    }

    /// Creates a TypeError for augmented assignment operator type mismatches (e.g., `+=`).
    ///
    /// For `+=` with str/list on the left side, uses CPython's special format:
    /// `can only concatenate {type} (not "{other}") to {type}`
    ///
    /// For other cases, uses the generic format:
    /// `unsupported operand type(s) for {op}: '{left}' and '{right}'`
    ///
    /// Returns a `SimpleException` without frame info - caller should add the frame.
    pub(crate) fn augmented_assign_type_error(op: &Operator, left_type: Type, right_type: Type) -> Self {
        let message = if *op == Operator::Add && (left_type == Type::Str || left_type == Type::List) {
            format!("can only concatenate {left_type} (not \"{right_type}\") to {left_type}")
        } else {
            format!("unsupported operand type(s) for {op}: '{left_type}' and '{right_type}'")
        };
        Self::new(ExcType::TypeError, Some(message))
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

/// A raised exception with optional stack frame for traceback.
#[derive(Debug, Clone)]
pub struct ExceptionRaise {
    pub exc: SimpleException,
    /// The stack frame where the exception was raised (first in vec is closest "bottom" frame).
    pub(crate) frame: Option<StackFrame>,
}

impl fmt::Display for ExceptionRaise {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref frame) = self.frame {
            writeln!(f, "Traceback (most recent call last):")?;
            // Display without source line content (source not available in Display)
            write!(f, "{frame}")?;
        }
        write!(f, "{}", self.exc)
    }
}

impl From<SimpleException> for ExceptionRaise {
    fn from(exc: SimpleException) -> Self {
        ExceptionRaise { exc, frame: None }
    }
}

impl ExceptionRaise {
    /// Returns a compact summary of the exception for test output.
    ///
    /// Format: `(position) ExceptionType('message')` or `(<no-tb>) ExceptionType('message')` if no traceback.
    #[must_use]
    pub fn summary(&self) -> String {
        if let Some(ref frame) = self.frame {
            format!("({:?}) {}", frame.position, self.exc)
        } else {
            format!("(<no-tb>) {}", self.exc)
        }
    }

    /// Returns the exception formatted as Python would display it to the user.
    ///
    /// Format: `ExceptionType: message` (e.g., `NotImplementedError: feature not supported`)
    #[must_use]
    pub fn py_str(&self) -> String {
        self.exc.py_str()
    }
}

/// A stack frame for traceback information.
///
/// Stores position information and optional function name as StringId.
/// The actual name string must be looked up externally when formatting the traceback.
#[derive(Debug, Clone)]
pub struct StackFrame {
    pub(crate) position: CodeRange,
    /// The name of the frame (function name StringId, or None for module-level code).
    pub(crate) frame_name: Option<StringId>,
    pub(crate) parent: Option<Box<StackFrame>>,
}

impl fmt::Display for StackFrame {
    /// Display impl for debugging - shows StringId indices.
    /// For actual traceback output with names, use the Interns storage.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref parent) = self.parent {
            write!(f, "{parent}")?;
        }
        let frame_name = self.frame_name.map(|id| format!("<name:{}>", id.index()));
        // Display without source line content or filename (must be passed externally)
        self.position.traceback(f, frame_name.as_deref(), "", "<file>")
    }
}

impl StackFrame {
    pub(crate) fn new(position: CodeRange, frame_name: StringId, parent: Option<&StackFrame>) -> Self {
        Self {
            position,
            frame_name: Some(frame_name),
            parent: parent.map(|p| Box::new(p.clone())),
        }
    }

    fn from_position(position: CodeRange) -> Self {
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

/// Runtime error types that can occur during execution.
///
/// Can be an internal error (bug in interpreter), a Python exception,
/// or a resource limit error.
#[derive(Debug)]
pub enum RunError {
    Internal(InternalRunError),
    Exc(ExceptionRaise),
    /// Resource limit exceeded (allocation, time, or memory).
    Resource(ResourceError),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Internal(s) => write!(f, "{s}"),
            Self::Exc(s) => write!(f, "{s}"),
            Self::Resource(r) => write!(f, "ResourceError: {r}"),
        }
    }
}

impl From<InternalRunError> for RunError {
    fn from(internal_error: InternalRunError) -> Self {
        Self::Internal(internal_error)
    }
}

impl From<ExceptionRaise> for RunError {
    fn from(exc: ExceptionRaise) -> Self {
        Self::Exc(exc)
    }
}

impl From<SimpleException> for RunError {
    fn from(exc: SimpleException) -> Self {
        Self::Exc(exc.into())
    }
}

impl From<ResourceError> for RunError {
    fn from(err: ResourceError) -> Self {
        Self::Resource(err)
    }
}
