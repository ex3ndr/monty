mod evaluate;
mod exceptions;
mod expressions;
mod heap;
mod literal;
mod object;
mod object_types;
mod operators;
mod parse;
mod parse_error;
mod prepare;
mod run;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use crate::exceptions::{InternalRunError, RunError};
pub use crate::expressions::Exit;
use crate::expressions::Node;
use crate::literal::Literal;
pub use crate::object::Object;
use crate::parse::parse;
pub use crate::parse_error::{ParseError, ParseResult};
use crate::prepare::prepare;
use crate::run::RunFrame;
use std::cell::Ref;
use std::cell::RefCell;

// Re-export heap types for testing and debugging
pub use crate::heap::{Heap, HeapData};

/// Main executor that compiles and runs Python code.
///
/// The executor stores the compiled AST and initial namespace as literals (not runtime
/// objects). When `run()` is called, literals are converted to heap-allocated runtime
/// objects, ensuring proper reference counting from the start of execution.
#[derive(Debug)]
pub struct Executor<'c> {
    initial_namespace: Vec<Literal>,
    nodes: Vec<Node<'c>>,
    heap: RefCell<Heap>,
}

impl<'c> Executor<'c> {
    pub fn new(code: &'c str, filename: &'c str, input_names: &[&str]) -> ParseResult<'c, Self> {
        let nodes = parse(code, filename)?;
        // dbg!(&nodes);
        let (initial_namespace, nodes) = prepare(nodes, input_names)?;
        // dbg!(&initial_namespace, &nodes);
        Ok(Self {
            initial_namespace,
            nodes,
            heap: RefCell::new(Heap::default()),
        })
    }

    /// Returns a reference to the heap for accessing heap-allocated objects.
    ///
    /// This is primarily useful for testing and debugging, where you need to
    /// format or inspect objects after execution has completed.
    pub fn heap(&self) -> Ref<Heap> {
        self.heap.borrow()
    }

    /// Executes the code with the given input values.
    ///
    /// The heap is cleared at the start of each run, ensuring no state leaks between
    /// executions. The initial namespace (stored as Literals) is converted to runtime
    /// Objects with proper heap allocation and reference counting.
    ///
    /// # Arguments
    /// * `inputs` - Values to fill the first N slots of the namespace (e.g., function parameters)
    pub fn run(&self, inputs: Vec<Object>) -> Result<Exit<'c>, InternalRunError> {
        // Clear heap before starting new execution
        let mut heap = self.heap.borrow_mut();
        heap.clear();

        // Convert initial namespace from Literals to Objects with heap allocation
        let mut namespace: Vec<Object> = self
            .initial_namespace
            .iter()
            .map(|lit| lit.to_object(&mut heap))
            .collect();

        // Fill in the input values (overwriting the default Undefined slots)
        for (i, input) in inputs.into_iter().enumerate() {
            namespace[i] = input;
        }

        match RunFrame::new(namespace).execute(&mut heap, &self.nodes) {
            Ok(v) => Ok(v),
            Err(e) => match e {
                RunError::Exc(exc) => Ok(Exit::Raise(exc)),
                RunError::Internal(internal) => Err(internal),
            },
        }
    }
}

/// parse code and show the parsed AST, mostly for testing
pub fn parse_show(code: &str, filename: &str) -> Result<String, String> {
    match parse(code, filename) {
        Ok(ast) => Ok(format!("{ast:#?}")),
        Err(e) => Err(e.to_string()),
    }
}
