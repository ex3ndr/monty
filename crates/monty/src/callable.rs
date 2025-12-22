use crate::{
    args::ArgValues,
    builtins::Builtins,
    evaluate::{EvalResult, ExternalCall},
    exceptions::{exc_fmt, ExcType},
    expressions::Identifier,
    heap::{Heap, HeapData},
    intern::Interns,
    io::PrintWriter,
    namespace::{NamespaceId, Namespaces},
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{PyTrait, Type},
    value::Value,
};

/// Target of a function call expression.
///
/// Represents a callable that can be either:
/// - A builtin function or exception resolved at parse time (`print`, `len`, `ValueError`, etc.)
/// - A name that will be looked up in the namespace at runtime (for callable variables)
///
/// Separate from Value to allow deriving Clone without Value's Clone restrictions.
#[derive(Debug, Clone, Copy)]
pub enum Callable {
    /// A builtin function like `print`, `len`, `str`, etc.
    Builtin(Builtins),
    /// A name to be looked up in the namespace at runtime (e.g., `x` in `x = len; x('abc')`).
    Name(Identifier),
}

impl Callable {
    /// Calls this callable with the given arguments.
    ///
    /// # Arguments
    /// * `namespaces` - The namespace namespaces containing all namespaces
    /// * `local_idx` - Index of the local namespace in namespaces
    /// * `heap` - The heap for allocating objects
    /// * `args` - The arguments to pass to the callable
    /// * `interns` - String storage for looking up interned names in error messages
    /// * `writer` - The writer for print output
    pub fn call(
        &self,
        namespaces: &mut Namespaces,
        local_idx: NamespaceId,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        interns: &Interns,
        writer: &mut impl PrintWriter,
    ) -> RunResult<EvalResult<Value>> {
        match self {
            Callable::Builtin(b) => b.call(heap, args, interns, writer).map(EvalResult::Value),
            Callable::Name(ident) => {
                // Look up the callable in the namespace
                let value = namespaces.get_var(local_idx, ident, interns)?;

                match value {
                    Value::Builtin(builtin) => return builtin.call(heap, args, interns, writer).map(EvalResult::Value),
                    Value::Function(f_id) => {
                        // Simple function without defaults - pass empty slice
                        return interns
                            .get_function(*f_id)
                            .call(namespaces, heap, args, &[], interns, writer)
                            .map(EvalResult::Value);
                    }
                    Value::ExtFunction(f_id) => {
                        let f_id = *f_id;
                        return if let Some(return_value) = namespaces.take_return_value(heap) {
                            Ok(EvalResult::Value(return_value))
                        } else {
                            Ok(EvalResult::ExternalCall(ExternalCall::new(f_id, args)))
                        };
                    }
                    // Check for heap-allocated closure or function with defaults
                    Value::Ref(heap_id) => {
                        let heap_id = *heap_id;
                        // Use with_entry_mut to temporarily take the HeapData out,
                        // allowing us to borrow heap mutably for the function call
                        return heap
                            .with_entry_mut(heap_id, |heap, data| {
                                match data {
                                    HeapData::Closure(f_id, cells, defaults) => {
                                        let f = interns.get_function(*f_id);
                                        f.call_with_cells(namespaces, heap, args, cells, defaults, interns, writer)
                                    }
                                    HeapData::FunctionDefaults(f_id, defaults) => {
                                        let f = interns.get_function(*f_id);
                                        f.call(namespaces, heap, args, defaults, interns, writer)
                                    }
                                    _ => {
                                        // Not a callable heap type
                                        let type_name = data.py_type(Some(heap));
                                        let err = exc_fmt!(ExcType::TypeError; "'{type_name}' object is not callable");
                                        Err(err.with_position(ident.position).into())
                                    }
                                }
                            })
                            .map(EvalResult::Value);
                    }
                    _ => {}
                }
                let type_name = value.py_type(Some(heap));
                let err = exc_fmt!(ExcType::TypeError; "'{type_name}' object is not callable");
                Err(err.with_position(ident.position).into())
            }
        }
    }

    /// Returns true if this Callable is equal to another Callable.
    ///
    /// We assume functions with the same name and position in code are equal.
    pub fn py_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Builtin(b1), Self::Builtin(b2)) => b1 == b2,
            (Self::Name(n1), Self::Name(n2)) => n1.py_eq(n2),
            _ => false,
        }
    }

    pub fn py_type(&self) -> Type {
        match self {
            Self::Builtin(b) => b.py_type(),
            Self::Name(_) => Type::Function,
        }
    }

    /// Returns the callable name for error messages.
    ///
    /// For builtins, returns the builtin name (e.g., "print", "len") as a static str.
    /// For named callables, returns the function name from interns.
    pub fn name<'a>(&self, interns: &'a Interns) -> &'a str {
        match self {
            Self::Builtin(Builtins::Function(f)) => (*f).into(),
            Self::Builtin(Builtins::ExcType(e)) => (*e).into(),
            Self::Builtin(Builtins::Type(t)) => (*t).into(),
            Self::Name(ident) => interns.get_str(ident.name_id),
        }
    }
}
