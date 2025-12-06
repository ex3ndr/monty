use core::fmt;
use std::borrow::Cow;

use crate::{
    args::ArgObjects,
    builtins::Builtins,
    evaluate::namespace_get_mut,
    exceptions::{exc_fmt, ExcType},
    expressions::Identifier,
    heap::Heap,
    object::Object,
    run::RunResult,
    values::PyValue,
};

/// Target of a function call expression.
///
/// Represents a callable that can be either:
/// - A builtin function resolved at parse time (`print`, `len`, etc.)
/// - An exception type constructor resolved at parse time (`ValueError`, etc.)
/// - A name that will be looked up in the namespace at runtime (for callable variables)
///
/// Separate from Object to allow deriving Clone without Object's Clone restrictions.
#[derive(Debug, Clone)]
pub(crate) enum Callable<'c> {
    /// A builtin function like `print`, `len`, `str`, etc.
    Builtin(Builtins),
    /// An exception type constructor like `ValueError`, `TypeError`, etc.
    ExcType(ExcType),
    /// A name to be looked up in the namespace at runtime (e.g., `x` in `x = len; x('abc')`).
    Name(Identifier<'c>),
}

impl fmt::Display for Callable<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin(b) => write!(f, "{b}"),
            Self::ExcType(e) => {
                let type_str: &'static str = (*e).into();
                f.write_str(type_str)
            }
            Self::Name(ident) => f.write_str(ident.name),
        }
    }
}

impl<'c> Callable<'c> {
    pub fn call<'e>(
        &self,
        namespace: &mut [Object<'c, 'e>],
        heap: &mut Heap<'c, 'e>,
        args: ArgObjects<'c, 'e>,
    ) -> RunResult<'c, Object<'c, 'e>> {
        match self {
            Callable::Builtin(b) => b.call(heap, args),
            Callable::ExcType(exc) => exc.call(heap, args),
            Callable::Name(ident) => {
                // Look up the callable in the namespace and clone it to release the borrow
                // before making the recursive call that needs `namespace`
                let callable_obj = namespace_get_mut(namespace, ident)?;
                match callable_obj {
                    Object::Callable(callable) => {
                        let callable = callable.clone();
                        callable.call(namespace, heap, args)
                    }
                    Object::Function(f) => f.call(heap, args),
                    _ => {
                        let type_name = callable_obj.py_type(heap);
                        let err = exc_fmt!(ExcType::TypeError; "'{type_name}' object is not callable");
                        Err(err.with_position(ident.position).into())
                    }
                }
            }
        }
    }

    pub fn to_object(&self) -> Object<'c, '_> {
        Object::Callable(self.clone())
    }

    pub fn py_repr<'a, 'e>(&'a self, heap: &'a Heap<'c, 'e>) -> Cow<'a, str> {
        match self {
            Self::Builtin(b) => format!("<built-in function {}>", b.as_ref()).into(),
            Self::ExcType(e) => format!("<class '{}'>", <&'static str>::from(*e)).into(),
            Self::Name(name) => heap.get(name.heap_id()).py_repr(heap),
        }
    }

    pub fn py_type(&self, _heap: &Heap<'c, '_>) -> &'static str {
        match self {
            Self::Builtin(_) => "builtin_function_or_method",
            Self::ExcType(_) => "type",
            Self::Name(_) => "function",
        }
    }
}
