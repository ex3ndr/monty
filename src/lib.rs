mod args;
mod builtins;
mod callable;
mod evaluate;
pub mod exceptions;
mod exit;
mod expressions;
mod function;
mod heap;
mod object;
mod operators;
mod parse;
mod parse_error;
mod prepare;
mod run;
mod values;

#[cfg(feature = "ref-counting")]
use std::collections::HashMap;

#[cfg(feature = "ref-counting")]
use ahash::AHashMap;

use crate::exceptions::{InternalRunError, RunError};
pub use crate::exit::{Exit, Value};
use crate::expressions::Node;
use crate::heap::{Heap, HeapData};
use crate::object::Object;
use crate::parse::parse;
pub use crate::parse_error::ParseError;
use crate::prepare::prepare;
use crate::run::RunFrame;

/// Main executor that compiles and runs Python code.
///
/// The executor stores the compiled AST and initial namespace as literals (not runtime
/// objects). When `run()` is called, literals are converted to heap-allocated runtime
/// objects, ensuring proper reference counting from the start of execution.
///
/// When the `ref-counting` feature is enabled, `run_ref_counts()` can be used to
/// execute code and retrieve reference count data for testing purposes.
#[derive(Debug)]
pub struct Executor<'c> {
    namespace_size: usize,
    /// Maps variable names to their indices in the namespace. Used for ref-count testing.
    #[cfg(feature = "ref-counting")]
    name_map: AHashMap<String, usize>,
    nodes: Vec<Node<'c>>,
}

impl<'c> Executor<'c> {
    pub fn new(code: &'c str, filename: &'c str, input_names: &[&str]) -> Result<Self, ParseError<'c>> {
        let nodes = parse(code, filename)?;
        let prepared = prepare(nodes, input_names)?;
        Ok(Self {
            namespace_size: prepared.namespace_size,
            #[cfg(feature = "ref-counting")]
            name_map: prepared.name_map,
            nodes: prepared.nodes,
        })
    }

    /// Executes the code with the given input values.
    ///
    /// The heap is created fresh for each run, ensuring no state leaks between
    /// executions.
    ///
    /// # Arguments
    /// * `inputs` - Values to fill the first N slots of the namespace (e.g., function parameters)
    pub fn run<'e>(&'e self, inputs: Vec<Object<'c, 'e>>) -> Result<Exit<'c, 'e>, InternalRunError> {
        let namespace = self.prepare_namespace(inputs)?;
        let mut heap = Heap::default();

        let mut frame = RunFrame::new(namespace);
        let result = frame.execute(&mut heap, &self.nodes);

        match result {
            Ok(v) => Ok(Exit::new(v, heap)),
            Err(e) => match e {
                RunError::Exc(exc) => Ok(Exit::Raise(exc)),
                RunError::Internal(internal) => Err(internal),
            },
        }
    }

    /// Executes the code and returns both the result and reference count data.
    ///
    /// This is used for testing reference counting behavior. Returns:
    /// - The execution result (`Exit`)
    /// - Reference count data as a tuple of:
    ///   - A map from variable names to their reference counts (only for heap-allocated objects)
    ///   - The number of unique heap object IDs referenced by variables
    ///   - The total number of live heap objects
    ///
    /// For strict matching validation, compare unique_refs_count with heap_object_count.
    /// If they're equal, all heap objects are accounted for by named variables.
    ///
    /// Only available when the `ref-counting` feature is enabled.
    #[cfg(feature = "ref-counting")]
    pub fn run_ref_counts<'e>(
        &'e self,
        inputs: Vec<Object<'c, 'e>>,
    ) -> Result<(Exit<'c, 'e>, (HashMap<String, usize>, usize, usize)), InternalRunError> {
        use std::collections::HashSet;

        let namespace = self.prepare_namespace(inputs)?;
        let mut heap = Heap::default();

        let mut frame = RunFrame::new(namespace);
        let result = frame.execute(&mut heap, &self.nodes);

        // Compute ref counts before consuming the heap
        let final_namespace = frame.into_namespace();
        let mut counts = HashMap::new();
        let mut unique_ids = HashSet::new();

        for (name, &index) in &self.name_map {
            if let Some(Object::Ref(id)) = final_namespace.get(index) {
                counts.insert(name.clone(), heap.get_refcount(*id));
                unique_ids.insert(*id);
            }
        }
        let ref_count_data = (counts, unique_ids.len(), heap.object_count());

        let exit = match result {
            Ok(v) => Exit::new(v, heap),
            Err(e) => match e {
                RunError::Exc(exc) => Exit::Raise(exc),
                RunError::Internal(internal) => return Err(internal),
            },
        };

        Ok((exit, ref_count_data))
    }

    /// Prepares the namespace for execution by filling input slots and padding with Undefined.
    ///
    /// Returns the prepared namespace or an error if there are too many inputs.
    fn prepare_namespace<'e>(&self, inputs: Vec<Object<'c, 'e>>) -> Result<Vec<Object<'c, 'e>>, InternalRunError> {
        let Some(extra) = self.namespace_size.checked_sub(inputs.len()) else {
            return Err(InternalRunError::Error(
                format!("input length should be <={}", self.namespace_size).into(),
            ));
        };
        let mut namespace: Vec<Object<'c, 'e>> = inputs;
        if extra > 0 {
            namespace.extend((0..extra).map(|_| Object::Undefined));
        }
        Ok(namespace)
    }
}

/// parse code and show the parsed AST, mostly for testing
pub fn parse_show(code: &str, filename: &str) -> Result<String, String> {
    match parse(code, filename) {
        Ok(ast) => Ok(format!("{ast:#?}")),
        Err(e) => Err(e.to_string()),
    }
}
