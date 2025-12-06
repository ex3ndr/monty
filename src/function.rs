use std::{borrow::Cow, fmt};

use crate::{
    args::ArgObjects,
    exceptions::{ExcType, SimpleException, StackFrame},
    expressions::{FrameExit, Identifier, Node},
    heap::Heap,
    object::{heap_tagged_id, Object},
    run::{RunFrame, RunResult},
    values::str::string_repr,
};

/// Stores a function definition.
///
/// Contains everything needed to execute a user-defined function: the body AST,
/// initial namespace layout, and captured closure cells. Functions are stored
/// on the heap and referenced via ObjectId.
#[derive(Debug, Clone)]
pub(crate) struct Function<'c> {
    /// The function name (used for error messages and repr).
    pub name: Identifier<'c>,
    /// The function parameters (used for error message).
    pub params: Vec<&'c str>,
    /// The prepared function body AST nodes.
    pub body: Vec<Node<'c>>,
    /// Size of the initial namespace
    pub namespace_size: usize,
    // /// References to shared cells for captured variables.
    // /// Each ObjectId points to a HeapData::Cell on the heap.
    // pub closure_cells: Vec<ObjectId>,
}

impl fmt::Display for Function<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name.name)
    }
}

impl<'c> Function<'c> {
    /// Create a new function definition.
    pub fn new(name: Identifier<'c>, params: Vec<&'c str>, body: Vec<Node<'c>>, namespace_size: usize) -> Self {
        Self {
            name,
            params,
            body,
            namespace_size,
        }
    }

    pub fn call<'e>(&'e self, heap: &mut Heap<'c, 'e>, args: ArgObjects<'c, 'e>) -> RunResult<'c, Object<'c, 'e>>
    where
        'c: 'e,
    {
        let mut namespace = Vec::with_capacity(self.namespace_size);
        args.inject_into_namespace(&mut namespace);
        if namespace.len() == self.params.len() {
            let extra = self.namespace_size - namespace.len();
            namespace.extend((0..extra).map(|_| Object::Undefined));

            // Create stack frame for error tracebacks
            let parent_frame = StackFrame::new(&self.name.position, self.name.name, None);

            // Execute the function body in a new frame
            let mut frame = RunFrame::new_for_function(namespace, self.name.name, Some(parent_frame));

            let result = frame.execute(heap, &self.body);

            match result {
                Ok(FrameExit::Return(obj)) => Ok(obj),
                Ok(FrameExit::Raise(exc)) => Err(exc.into()),
                Err(e) => Err(e),
            }
        } else {
            let msg = if let Some(missing_count) = self.params.len().checked_sub(namespace.len()) {
                let mut msg = format!(
                    "{}() missing {} required positional argument{}: ",
                    self.name.name,
                    missing_count,
                    if missing_count == 1 { "" } else { "s" }
                );
                let mut missing_names: Vec<_> = self
                    .params
                    .iter()
                    .skip(namespace.len())
                    .map(|param| string_repr(param))
                    .collect();
                let last = missing_names.pop().unwrap();
                if !missing_names.is_empty() {
                    // Insert "and" before the last element (with Oxford comma)
                    msg.push_str(&missing_names.join(", "));
                    msg.push_str(", and ");
                }
                msg.push_str(&last);
                msg
            } else {
                format!(
                    "{}() takes {} positional argument{} but {} {} given",
                    self.name.name,
                    self.params.len(),
                    if self.params.len() == 1 { "" } else { "s" },
                    namespace.len(),
                    if namespace.len() == 1 { "was" } else { "were" }
                )
            };
            Err(SimpleException::new(ExcType::TypeError, Some(msg.into()))
                .with_position(self.name.position)
                .into())
        }
    }

    pub fn py_repr(&self) -> Cow<'_, str> {
        format!("<function '{}' at 0x{:x}>", self, self.id()).into()
    }

    pub fn id(&self) -> usize {
        heap_tagged_id(self.name.heap_id())
    }
}
