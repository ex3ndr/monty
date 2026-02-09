//! Public interface for running Monty code.
use std::sync::atomic::{AtomicUsize, Ordering};

use ahash::AHashMap;

use crate::{
    ExcType, MontyException,
    asyncio::CallId,
    bytecode::{Code, Compiler, FrameExit, VM, VMSnapshot},
    exception_private::RunResult,
    heap::Heap,
    intern::{ExtFunctionId, InternerBuilder, Interns},
    io::{PrintWriter, StdPrint},
    namespace::{GLOBAL_NS_IDX, NamespaceId, Namespaces},
    object::MontyObject,
    os::OsFunction,
    parse::{parse, parse_with_interner},
    prepare::{prepare, prepare_with_existing_names},
    resource::{NoLimitTracker, ResourceTracker},
    value::Value,
};

/// Primary interface for running Monty code.
///
/// `MontyRun` supports three execution modes:
/// - **Simple execution**: Use `run()` or `run_no_limits()` to run code to completion
/// - **Iterative execution**: Use `start()` to start execution which will pause at external function calls and
///   can be resumed later
/// - **Stateful REPL**: Use `into_repl()` and then `MontyRepl::feed()` to execute only new snippets (no replay)
///
/// # Example
/// ```
/// use monty::{MontyRun, MontyObject};
///
/// let runner = MontyRun::new("x + 1".to_owned(), "test.py", vec!["x".to_owned()], vec![]).unwrap();
/// let result = runner.run_no_limits(vec![MontyObject::Int(41)]).unwrap();
/// assert_eq!(result, MontyObject::Int(42));
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyRun {
    /// Script name used for parse and runtime error messages.
    script_name: String,
    /// Names of external functions available to the executed code.
    ///
    /// Stored so `into_repl()` can create a true incremental REPL that knows
    /// which host-call names are allowed.
    external_function_names: Vec<String>,
    /// The underlying executor containing parsed AST and interns.
    executor: Executor,
}

impl MontyRun {
    /// Creates a new run snapshot by parsing the given code.
    ///
    /// This only parses and prepares the code - no heap or namespaces are created yet.
    /// Call `run_snapshot()` with inputs to start execution.
    ///
    /// # Arguments
    /// * `code` - The Python code to execute
    /// * `script_name` - The script name for error messages
    /// * `input_names` - Names of input variables
    ///
    /// # Errors
    /// Returns `MontyException` if the code cannot be parsed.
    pub fn new(
        code: String,
        script_name: &str,
        input_names: Vec<String>,
        external_functions: Vec<String>,
    ) -> Result<Self, MontyException> {
        let executor = Executor::new(code, script_name, input_names, external_functions.clone())?;
        Ok(Self {
            script_name: script_name.to_owned(),
            external_function_names: external_functions,
            executor,
        })
    }

    /// Returns the code that was parsed to create this snapshot.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.executor.code
    }

    /// Executes the code and returns both the result and reference count data, used for testing only.
    #[cfg(feature = "ref-count-return")]
    pub fn run_ref_counts(&self, inputs: Vec<MontyObject>) -> Result<RefCountOutput, MontyException> {
        self.executor.run_ref_counts(inputs)
    }

    /// Executes the code to completion assuming not external functions or snapshotting.
    ///
    /// This is marginally faster than running with snapshotting enabled since we don't need
    /// to track the position in code, but does not allow calling of external functions.
    ///
    /// # Arguments
    /// * `inputs` - Values to fill the first N slots of the namespace
    /// * `resource_tracker` - Custom resource tracker implementation
    /// * `print` - print print implementation
    pub fn run(
        &self,
        inputs: Vec<MontyObject>,
        resource_tracker: impl ResourceTracker,
        print: &mut impl PrintWriter,
    ) -> Result<MontyObject, MontyException> {
        self.executor.run(inputs, resource_tracker, print)
    }

    /// Executes the code to completion with no resource limits, printing to stdout/stderr.
    pub fn run_no_limits(&self, inputs: Vec<MontyObject>) -> Result<MontyObject, MontyException> {
        self.run(inputs, NoLimitTracker, &mut StdPrint)
    }

    /// Converts this runner into a stateful REPL session.
    ///
    /// The current runner's code is executed exactly once to initialize global state.
    /// Subsequent snippets fed through [`MontyRepl::feed`] execute only the new code
    /// against that preserved state, with no replay of prior snippets.
    ///
    /// # Returns
    /// A tuple of:
    /// - `MontyRepl<T>`: the stateful REPL session
    /// - `MontyObject`: the result of executing this runner's initial code once
    ///
    /// # Errors
    /// Returns `MontyException` if initialization fails.
    pub fn into_repl<T: ResourceTracker>(
        self,
        inputs: Vec<MontyObject>,
        resource_tracker: T,
        print: &mut impl PrintWriter,
    ) -> Result<(MontyRepl<T>, MontyObject), MontyException> {
        let Self {
            script_name,
            external_function_names,
            executor,
        } = self;

        let heap_capacity = executor.heap_capacity.load(Ordering::Relaxed);
        let mut heap = Heap::new(heap_capacity, resource_tracker);
        let mut namespaces = executor.prepare_namespaces(inputs, &mut heap)?;

        let mut vm = VM::new(&mut heap, &mut namespaces, &executor.interns, print);
        let frame_exit_result = vm.run_module(&executor.module_code);
        vm.cleanup();

        let output = frame_exit_to_object(frame_exit_result, &mut heap, &executor.interns)
            .map_err(|e| e.into_python_exception(&executor.interns, &executor.code))?;

        let repl = MontyRepl {
            script_name,
            external_function_names,
            global_name_map: executor.name_map,
            interns: executor.interns,
            heap,
            namespaces,
        };
        Ok((repl, output))
    }

    /// Serializes the runner to a binary format.
    ///
    /// The serialized data can be stored and later restored with `load()`.
    /// This allows caching parsed code to avoid re-parsing on subsequent runs.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    pub fn dump(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }

    /// Deserializes a runner from binary format.
    ///
    /// # Arguments
    /// * `bytes` - The serialized runner data from `dump()`
    ///
    /// # Errors
    /// Returns an error if deserialization fails.
    pub fn load(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }

    /// Starts execution with the given inputs and resource tracker, consuming self.
    ///
    /// Creates the heap and namespaces, then begins execution.
    ///
    /// For iterative execution, `start()` consumes self and returns a `RunProgress`:
    /// - `RunProgress::FunctionCall { ..., state }` - external function call, call `state.run(return_value)` to resume
    /// - `RunProgress::Complete(value)` - execution finished
    ///
    /// This enables snapshotting execution state and returning control to the host
    /// application during long-running computations.
    ///
    /// # Arguments
    /// * `inputs` - Initial input values (must match length of `input_names` from `new()`)
    /// * `resource_tracker` - Resource tracker for the execution
    /// * `print` - Writer for print output
    ///
    /// # Errors
    /// Returns `MontyException` if:
    /// - The number of inputs doesn't match the expected count
    /// - An input value is invalid (e.g., `MontyObject::Repr`)
    /// - A runtime error occurs during execution
    ///
    /// # Panics
    /// This method should not panic under normal operation. Internal assertions
    /// may panic if the VM reaches an inconsistent state (indicating a bug).
    pub fn start<T: ResourceTracker>(
        self,
        inputs: Vec<MontyObject>,
        resource_tracker: T,
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException> {
        let executor = self.executor;

        // Create heap and prepare namespaces
        let mut heap = Heap::new(executor.namespace_size, resource_tracker);
        let mut namespaces = executor.prepare_namespaces(inputs, &mut heap)?;

        // Create and run VM - scope the VM borrow so we can move heap/namespaces after
        let mut vm = VM::new(&mut heap, &mut namespaces, &executor.interns, print);

        // Start execution
        let vm_result = vm.run_module(&executor.module_code);

        let vm_state = vm.check_snapshot(&vm_result);

        // Handle the result using the destructured parts
        handle_vm_result(vm_result, vm_state, executor, heap, namespaces)
    }
}

/// Stateful REPL session that executes snippets incrementally without replay.
///
/// `MontyRepl` preserves heap and global namespace state between snippets.
/// Each `feed()` compiles and executes only the new snippet against the current
/// state, avoiding the cost and semantic risks of replaying prior code.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct MontyRepl<T: ResourceTracker> {
    /// Script name used for parse and runtime error messages.
    script_name: String,
    /// External function names declared for this session.
    external_function_names: Vec<String>,
    /// Stable mapping of global variable names to namespace slot IDs.
    global_name_map: AHashMap<String, NamespaceId>,
    /// Persistent intern table across snippets so intern/function IDs remain valid.
    interns: Interns,
    /// Persistent heap across snippets.
    heap: Heap<T>,
    /// Persistent namespace stack across snippets.
    namespaces: Namespaces,
}

impl<T: ResourceTracker> MontyRepl<T> {
    /// Starts executing a new snippet and returns suspendable REPL progress.
    ///
    /// This is the REPL equivalent of [`MontyRun::start`]: execution may complete,
    /// or suspend at external calls / OS calls / unresolved futures. Resume with the
    /// returned state object and eventually recover the updated REPL from
    /// [`ReplProgress::into_complete`].
    ///
    /// Unlike [`MontyRepl::feed`], this method consumes `self` so runtime state can be
    /// safely moved into snapshot objects for serialization and cross-process resume.
    ///
    /// # Errors
    /// Returns `MontyException` for syntax/compile/runtime failures.
    pub fn start(self, code: &str, print: &mut impl PrintWriter) -> Result<ReplProgress<T>, MontyException> {
        let mut this = self;
        if code.is_empty() {
            return Ok(ReplProgress::Complete {
                repl: this,
                value: MontyObject::None,
            });
        }

        let executor = Executor::new_repl_snippet(
            code.to_owned(),
            &this.script_name,
            this.external_function_names.clone(),
            this.global_name_map.clone(),
            &this.interns,
        )?;

        this.ensure_global_namespace_size(executor.namespace_size);

        let (vm_result, vm_state) = {
            let mut vm = VM::new(&mut this.heap, &mut this.namespaces, &executor.interns, print);
            let vm_result = vm.run_module(&executor.module_code);
            let vm_state = vm.check_snapshot(&vm_result);
            (vm_result, vm_state)
        };

        handle_repl_vm_result(vm_result, vm_state, executor, this)
    }

    /// Starts snippet execution with `StdPrint` and no additional host output wiring.
    pub fn start_no_print(self, code: &str) -> Result<ReplProgress<T>, MontyException> {
        self.start(code, &mut StdPrint)
    }

    /// Feeds and executes a new snippet against the current REPL state.
    ///
    /// This compiles only `code` using the existing global slot map, extends the
    /// global namespace if new names are introduced, and executes the snippet once.
    /// Previously executed snippets are never replayed. If execution raises after
    /// partially mutating globals, those mutations remain visible in later feeds,
    /// matching Python REPL semantics.
    ///
    /// # Errors
    /// Returns `MontyException` for syntax/compile/runtime failures.
    pub fn feed(&mut self, code: &str, print: &mut impl PrintWriter) -> Result<MontyObject, MontyException> {
        if code.is_empty() {
            return Ok(MontyObject::None);
        }

        let executor = Executor::new_repl_snippet(
            code.to_owned(),
            &self.script_name,
            self.external_function_names.clone(),
            self.global_name_map.clone(),
            &self.interns,
        )?;

        let Executor {
            namespace_size,
            name_map,
            module_code,
            interns,
            code,
            ..
        } = executor;

        self.ensure_global_namespace_size(namespace_size);

        let mut vm = VM::new(&mut self.heap, &mut self.namespaces, &interns, print);
        let frame_exit_result = vm.run_module(&module_code);
        vm.cleanup();

        // Commit the compiler metadata even when execution returns an error.
        // Snippets can mutate globals before raising, and those values may contain
        // FunctionId/StringId values that must be interpreted with the new intern table.
        self.global_name_map = name_map;
        self.interns = interns;

        frame_exit_to_object(frame_exit_result, &mut self.heap, &self.interns)
            .map_err(|e| e.into_python_exception(&self.interns, &code))
    }

    /// Executes a snippet with `StdPrint` and no additional host output wiring.
    pub fn feed_no_print(&mut self, code: &str) -> Result<MontyObject, MontyException> {
        self.feed(code, &mut StdPrint)
    }

    /// Grows the global namespace to at least `namespace_size`, filling new slots with `Undefined`.
    fn ensure_global_namespace_size(&mut self, namespace_size: usize) {
        let global = self.namespaces.get_mut(GLOBAL_NS_IDX).mut_vec();
        if global.len() < namespace_size {
            global.resize_with(namespace_size, || Value::Undefined);
        }
    }
}

impl<T: ResourceTracker + serde::Serialize> MontyRepl<T> {
    /// Serializes the REPL session state to bytes.
    ///
    /// This includes heap + namespaces + global slot mapping, allowing snapshot/restore
    /// of interactive state between process runs.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    pub fn dump(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }
}

impl<T: ResourceTracker + serde::de::DeserializeOwned> MontyRepl<T> {
    /// Restores a REPL session from bytes produced by [`MontyRepl::dump`].
    ///
    /// # Errors
    /// Returns an error if deserialization fails.
    pub fn load(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

impl<T: ResourceTracker> Drop for MontyRepl<T> {
    fn drop(&mut self) {
        #[cfg(feature = "ref-count-panic")]
        self.namespaces.drop_global_with_heap(&mut self.heap);
    }
}

/// Result of a single step of iterative execution.
///
/// This enum owns the execution state, ensuring type-safe state transitions.
/// - `FunctionCall` contains info about an external function call and state to resume
/// - `ResolveFutures` contains pending futures that need resolution before continuing
/// - `Complete` contains just the final value (execution is done)
///
/// # Type Parameters
/// * `T` - Resource tracker implementation (e.g., `NoLimitTracker` or `LimitedTracker`)
///
/// Serialization requires `T: Serialize + Deserialize`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub enum RunProgress<T: ResourceTracker> {
    /// Execution paused at an external function call.
    ///
    /// The host can choose how to handle this:
    /// - **Sync resolution**: Call `state.run(return_value)` to push the result and continue
    /// - **Async resolution**: Call `state.run_pending()` to push an `ExternalFuture` and continue
    ///
    /// When using async resolution, the code continues and may `await` the future later.
    /// If the future isn't resolved when awaited, execution yields with `ResolveFutures`.
    FunctionCall {
        /// The name of the function being called.
        function_name: String,
        /// The positional arguments passed to the function.
        args: Vec<MontyObject>,
        /// The keyword arguments passed to the function (key, value pairs).
        kwargs: Vec<(MontyObject, MontyObject)>,
        /// Unique identifier for this call (used for async correlation).
        call_id: u32,
        /// The execution state that can be resumed with a return value.
        state: Snapshot<T>,
    },
    /// Execution paused for an OS-level operation.
    ///
    /// The host should execute the OS operation (filesystem, network, etc.) and
    /// call `state.run(return_value)` to provide the result and continue.
    ///
    /// This enables sandboxed execution where the interpreter never directly performs I/O.
    OsCall {
        /// The OS function to execute.
        function: OsFunction,
        /// The positional arguments for the OS function.
        args: Vec<MontyObject>,
        /// The keyword arguments passed to the function (key, value pairs).
        kwargs: Vec<(MontyObject, MontyObject)>,
        /// Unique identifier for this call (used for async correlation).
        call_id: u32,
        /// The execution state that can be resumed with a return value.
        state: Snapshot<T>,
    },
    /// All async tasks are blocked waiting for external futures to resolve.
    ///
    /// The host must resolve some or all of the pending calls before continuing.
    /// Use `state.resume(results)` to provide results for pending calls.
    ///
    /// access the pending call ids with `.pending_call_ids()`
    ResolveFutures(FutureSnapshot<T>),
    /// Execution completed with a final result.
    Complete(MontyObject),
}

impl<T: ResourceTracker> RunProgress<T> {
    /// Consumes the `RunProgress` and returns external function call info and state.
    ///
    /// Returns (function_name, positional_args, keyword_args, call_id, state).
    #[must_use]
    #[expect(clippy::type_complexity)]
    pub fn into_function_call(
        self,
    ) -> Option<(
        String,
        Vec<MontyObject>,
        Vec<(MontyObject, MontyObject)>,
        u32,
        Snapshot<T>,
    )> {
        match self {
            Self::FunctionCall {
                function_name,
                args,
                kwargs,
                call_id,
                state,
            } => Some((function_name, args, kwargs, call_id, state)),
            _ => None,
        }
    }

    /// Consumes the `RunProgress` and returns the final value.
    #[must_use]
    pub fn into_complete(self) -> Option<MontyObject> {
        match self {
            Self::Complete(value) => Some(value),
            _ => None,
        }
    }

    /// Consumes the `RunProgress` and returns pending futures info and state.
    ///
    /// Returns (pending_calls, state) if this is a ResolveFutures, None otherwise.
    #[must_use]
    pub fn into_resolve_futures(self) -> Option<FutureSnapshot<T>> {
        match self {
            Self::ResolveFutures(state) => Some(state),
            _ => None,
        }
    }
}

impl<T: ResourceTracker + serde::Serialize> RunProgress<T> {
    /// Serializes the execution state to a binary format.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    pub fn dump(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }
}

impl<T: ResourceTracker + serde::de::DeserializeOwned> RunProgress<T> {
    /// Deserializes execution state from binary format.
    ///
    /// # Errors
    /// Returns an error if deserialization fails.
    pub fn load(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

/// Result of a single suspendable REPL snippet execution.
///
/// This mirrors [`RunProgress`] but returns the updated [`MontyRepl`] on completion
/// so callers can continue feeding additional snippets without replaying prior code.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub enum ReplProgress<T: ResourceTracker> {
    /// Execution paused at an external function call.
    FunctionCall {
        /// The name of the function being called.
        function_name: String,
        /// The positional arguments passed to the function.
        args: Vec<MontyObject>,
        /// The keyword arguments passed to the function (key, value pairs).
        kwargs: Vec<(MontyObject, MontyObject)>,
        /// Unique identifier for this call (used for async correlation).
        call_id: u32,
        /// Repl execution state that can be resumed.
        state: ReplSnapshot<T>,
    },
    /// Execution paused for an OS-level operation.
    OsCall {
        /// The OS function to execute.
        function: OsFunction,
        /// The positional arguments for the OS function.
        args: Vec<MontyObject>,
        /// The keyword arguments passed to the function (key, value pairs).
        kwargs: Vec<(MontyObject, MontyObject)>,
        /// Unique identifier for this call (used for async correlation).
        call_id: u32,
        /// Repl execution state that can be resumed.
        state: ReplSnapshot<T>,
    },
    /// All async tasks are blocked waiting for external futures to resolve.
    ResolveFutures(ReplFutureSnapshot<T>),
    /// Snippet execution completed with the updated REPL and result value.
    Complete {
        /// Updated REPL session state to continue feeding snippets.
        repl: MontyRepl<T>,
        /// Final result produced by the snippet.
        value: MontyObject,
    },
}

impl<T: ResourceTracker> ReplProgress<T> {
    /// Consumes the progress and returns external function call info and state.
    ///
    /// Returns (function_name, positional_args, keyword_args, call_id, state).
    #[must_use]
    #[expect(clippy::type_complexity)]
    pub fn into_function_call(
        self,
    ) -> Option<(
        String,
        Vec<MontyObject>,
        Vec<(MontyObject, MontyObject)>,
        u32,
        ReplSnapshot<T>,
    )> {
        match self {
            Self::FunctionCall {
                function_name,
                args,
                kwargs,
                call_id,
                state,
            } => Some((function_name, args, kwargs, call_id, state)),
            _ => None,
        }
    }

    /// Consumes the progress and returns pending futures state.
    #[must_use]
    pub fn into_resolve_futures(self) -> Option<ReplFutureSnapshot<T>> {
        match self {
            Self::ResolveFutures(state) => Some(state),
            _ => None,
        }
    }

    /// Consumes the progress and returns the completed REPL and value.
    #[must_use]
    pub fn into_complete(self) -> Option<(MontyRepl<T>, MontyObject)> {
        match self {
            Self::Complete { repl, value } => Some((repl, value)),
            _ => None,
        }
    }
}

impl<T: ResourceTracker + serde::Serialize> ReplProgress<T> {
    /// Serializes the REPL execution progress to a binary format.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    pub fn dump(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }
}

impl<T: ResourceTracker + serde::de::DeserializeOwned> ReplProgress<T> {
    /// Deserializes REPL execution progress from a binary format.
    ///
    /// # Errors
    /// Returns an error if deserialization fails.
    pub fn load(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

/// REPL execution state that can be resumed after an external call.
///
/// This is the REPL-aware counterpart to [`Snapshot`]. Resuming continues the
/// same snippet and ultimately returns [`ReplProgress::Complete`] with the
/// updated REPL session.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ReplSnapshot<T: ResourceTracker> {
    /// Persistent REPL session state while this snippet is suspended.
    repl: MontyRepl<T>,
    /// Compiled snippet and intern/function tables for this execution.
    executor: Executor,
    /// VM stack/frame state at suspension.
    vm_state: VMSnapshot,
    /// call_id used when resuming with an unresolved future.
    pending_call_id: u32,
}

impl<T: ResourceTracker> ReplSnapshot<T> {
    /// Continues snippet execution with an external result.
    ///
    /// # Arguments
    /// * `result` - Return value, raised exception, or pending future marker
    /// * `print` - Writer used for Python `print()`
    pub fn run(
        self,
        result: impl Into<ExternalResult>,
        print: &mut impl PrintWriter,
    ) -> Result<ReplProgress<T>, MontyException> {
        let Self {
            mut repl,
            executor,
            vm_state,
            pending_call_id,
        } = self;

        let ext_result = result.into();

        let mut vm = VM::restore(
            vm_state,
            &executor.module_code,
            &mut repl.heap,
            &mut repl.namespaces,
            &executor.interns,
            print,
        );

        let vm_result = match ext_result {
            ExternalResult::Return(obj) => vm.resume(obj),
            ExternalResult::Error(exc) => vm.resume_with_exception(exc.into()),
            ExternalResult::Future => {
                let call_id = CallId::new(pending_call_id);
                vm.add_pending_call(call_id);
                vm.push(Value::ExternalFuture(call_id));
                vm.run()
            }
        };

        let vm_state = vm.check_snapshot(&vm_result);

        handle_repl_vm_result(vm_result, vm_state, executor, repl)
    }

    /// Continues snippet execution by pushing an unresolved `ExternalFuture`.
    ///
    /// This is the REPL-aware async pattern equivalent to [`Snapshot::run_pending`].
    pub fn run_pending(self, print: &mut impl PrintWriter) -> Result<ReplProgress<T>, MontyException> {
        self.run(MontyFuture, print)
    }
}

/// REPL execution state blocked on unresolved external futures.
///
/// This is the REPL-aware counterpart to [`FutureSnapshot`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ReplFutureSnapshot<T: ResourceTracker> {
    /// Persistent REPL session state while this snippet is suspended.
    repl: MontyRepl<T>,
    /// Compiled snippet and intern/function tables for this execution.
    executor: Executor,
    /// VM stack/frame state at suspension.
    vm_state: VMSnapshot,
    /// Pending call IDs expected by this snapshot.
    pending_call_ids: Vec<u32>,
}

impl<T: ResourceTracker> ReplFutureSnapshot<T> {
    /// Returns unresolved call IDs for this suspended state.
    #[must_use]
    pub fn pending_call_ids(&self) -> &[u32] {
        &self.pending_call_ids
    }

    /// Resumes snippet execution with zero or more resolved futures.
    ///
    /// Supports incremental resolution: callers can provide only a subset of
    /// pending call IDs and continue resolving over multiple resumes.
    ///
    /// # Errors
    /// Returns `MontyException` if an unknown call ID is provided.
    pub fn resume(
        self,
        results: Vec<(u32, ExternalResult)>,
        print: &mut impl PrintWriter,
    ) -> Result<ReplProgress<T>, MontyException> {
        use crate::exception_private::RunError;

        let Self {
            mut repl,
            executor,
            vm_state,
            pending_call_ids,
        } = self;

        let invalid_call_id = results
            .iter()
            .find(|(call_id, _)| !pending_call_ids.contains(call_id))
            .map(|(call_id, _)| *call_id);

        let mut vm = VM::restore(
            vm_state,
            &executor.module_code,
            &mut repl.heap,
            &mut repl.namespaces,
            &executor.interns,
            print,
        );

        if let Some(call_id) = invalid_call_id {
            vm.cleanup();
            #[cfg(feature = "ref-count-panic")]
            repl.namespaces.drop_global_with_heap(&mut repl.heap);
            return Err(MontyException::runtime_error(format!(
                "unknown call_id {call_id}, expected one of: {pending_call_ids:?}"
            )));
        }

        for (call_id, ext_result) in results {
            match ext_result {
                ExternalResult::Return(obj) => vm.resolve_future(call_id, obj).map_err(|e| {
                    MontyException::runtime_error(format!("Invalid return type for call {call_id}: {e}"))
                })?,
                ExternalResult::Error(exc) => vm.fail_future(call_id, RunError::from(exc)),
                ExternalResult::Future => {}
            }
        }

        if let Some(error) = vm.take_failed_task_error() {
            vm.cleanup();
            #[cfg(feature = "ref-count-panic")]
            repl.namespaces.drop_global_with_heap(&mut repl.heap);
            return Err(error.into_python_exception(&executor.interns, &executor.code));
        }

        let main_task_ready = vm.prepare_main_task_after_resolve();

        let loaded_task = match vm.load_ready_task_if_needed() {
            Ok(loaded) => loaded,
            Err(e) => {
                vm.cleanup();
                #[cfg(feature = "ref-count-panic")]
                repl.namespaces.drop_global_with_heap(&mut repl.heap);
                return Err(e.into_python_exception(&executor.interns, &executor.code));
            }
        };

        if !main_task_ready && !loaded_task {
            let pending_call_ids = vm.get_pending_call_ids();
            if !pending_call_ids.is_empty() {
                let vm_state = vm.snapshot();
                let pending_call_ids: Vec<u32> = pending_call_ids.iter().map(|id| id.raw()).collect();
                return Ok(ReplProgress::ResolveFutures(Self {
                    repl,
                    executor,
                    vm_state,
                    pending_call_ids,
                }));
            }
        }

        let vm_result = vm.run();
        let vm_state = vm.check_snapshot(&vm_result);

        handle_repl_vm_result(vm_result, vm_state, executor, repl)
    }
}

/// Execution state that can be resumed after an external function call.
///
/// This struct owns all runtime state and provides methods to continue execution:
/// - `run(result)`: Resume with the external function's return value (sync pattern)
/// - `run_pending()`: Resume with an `ExternalFuture` that can be awaited later (async pattern)
///
/// External function calls occur when calling a function that is not a builtin,
/// exception, or user-defined function.
///
/// # Type Parameters
/// * `T` - Resource tracker implementation
///
/// Serialization requires `T: Serialize + Deserialize`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct Snapshot<T: ResourceTracker> {
    /// The executor containing compiled code and interns.
    executor: Executor,
    /// The VM state containing stack, frames, and exception state.
    vm_state: VMSnapshot,
    /// The heap containing all allocated objects.
    heap: Heap<T>,
    /// The namespaces containing all variable bindings.
    namespaces: Namespaces,
    /// The call_id from the most recent FunctionCall that created this Snapshot.
    /// Used by `run_pending()` to push the correct `ExternalFuture`.
    pending_call_id: u32,
}

#[derive(Debug)]
pub struct MontyFuture;

/// Return value or exception from an external function.
#[derive(Debug)]
pub enum ExternalResult {
    /// Continues execution with the return value from the external function.
    Return(MontyObject),
    /// Continues execution with the exception raised by the external function.
    Error(MontyException),
    /// Pending future - when the external function is a coroutine.
    Future,
}

impl From<MontyObject> for ExternalResult {
    fn from(value: MontyObject) -> Self {
        Self::Return(value)
    }
}

impl From<MontyException> for ExternalResult {
    fn from(exception: MontyException) -> Self {
        Self::Error(exception)
    }
}

impl From<MontyFuture> for ExternalResult {
    fn from(_: MontyFuture) -> Self {
        Self::Future
    }
}

impl<T: ResourceTracker> Snapshot<T> {
    /// Continues execution with the return value or exception from the external function.
    ///
    /// Consumes self and returns the next execution progress.
    ///
    /// # Arguments
    /// * `result` - The return value or exception from the external function
    /// * `print` - The print writer to use for output
    ///
    /// # Panics
    /// This method should not panic under normal operation. Internal assertions
    /// may panic if the VM reaches an inconsistent state (indicating a bug).
    pub fn run(
        mut self,
        result: impl Into<ExternalResult>,
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException> {
        let ext_result = result.into();

        // Restore the VM from the snapshot
        let mut vm = VM::restore(
            self.vm_state,
            &self.executor.module_code,
            &mut self.heap,
            &mut self.namespaces,
            &self.executor.interns,
            print,
        );

        // Convert return value or exception before creating VM (to avoid borrow conflicts)
        let vm_result = match ext_result {
            ExternalResult::Return(obj) => vm.resume(obj),
            ExternalResult::Error(exc) => vm.resume_with_exception(exc.into()),
            ExternalResult::Future => {
                // Get the call_id and ext_function_id that were stored when this Snapshot was created
                let call_id = CallId::new(self.pending_call_id);

                // Store pending call data in the scheduler so we can track the creator task
                // and ignore results if the task is cancelled
                vm.add_pending_call(call_id);

                // Push the ExternalFuture value onto the stack
                // This allows the code to continue and potentially await this future later
                vm.push(Value::ExternalFuture(call_id));

                // Continue execution
                vm.run()
            }
        };

        let vm_state = vm.check_snapshot(&vm_result);

        // Handle the result using the destructured parts
        handle_vm_result(vm_result, vm_state, self.executor, self.heap, self.namespaces)
    }

    /// Continues execution by pushing an ExternalFuture instead of a concrete value.
    ///
    /// This is the async resolution pattern: instead of providing the result immediately,
    /// the host calls this method to continue execution with a pending future. The code
    /// can then `await` this future later.
    ///
    /// If the code awaits the future before it's resolved, execution will yield with
    /// `RunProgress::ResolveFutures`. The host can then provide the result via
    /// `FutureSnapshot::resume()`.
    ///
    /// # Arguments
    /// * `print` - Writer for print output
    ///
    /// # Returns
    /// The next execution progress - may be another `FunctionCall`, `ResolveFutures`, or `Complete`.
    ///
    /// # Panics
    /// Panics if the VM reaches an inconsistent state (indicating a bug in the interpreter).
    pub fn run_pending(self, print: &mut impl PrintWriter) -> Result<RunProgress<T>, MontyException> {
        self.run(MontyFuture, print)
    }
}

/// Execution state paused while waiting for external future results.
///
/// Unlike `Snapshot` (used for sync external calls), `FutureSnapshot` supports
/// incremental resolution - you can provide partial results and Monty will
/// continue running until all tasks are blocked again.
///
/// # Type Parameters
/// * `T` - Resource tracker implementation
///
/// Serialization requires `T: Serialize + Deserialize`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct FutureSnapshot<T: ResourceTracker> {
    /// The executor containing compiled code and interns.
    executor: Executor,
    /// The VM state containing stack, frames, and exception state.
    vm_state: VMSnapshot,
    /// The heap containing all allocated objects.
    heap: Heap<T>,
    /// The namespaces containing all variable bindings.
    namespaces: Namespaces,
    /// The pending call_ids that this snapshot is waiting on.
    /// Used to validate that resume() only receives known call_ids.
    pending_call_ids: Vec<u32>,
}

impl<T: ResourceTracker> FutureSnapshot<T> {
    pub fn pending_call_ids(&self) -> &[u32] {
        &self.pending_call_ids
    }

    /// Resumes execution with results for some or all pending futures.
    ///
    /// **Incremental resolution**: You don't need to provide all results at once.
    /// If you provide a partial list, Monty will:
    /// 1. Mark those futures as resolved
    /// 2. Unblock any tasks waiting on those futures
    /// 3. Continue running until all tasks are blocked again
    /// 4. Return `ResolveFutures` with the remaining pending calls
    ///
    /// This allows the host to resolve futures as they complete, rather than
    /// waiting for all of them.
    ///
    /// # Arguments
    /// * `results` - List of (call_id, result) pairs. Can be a subset of pending calls.
    /// * `print` - Writer for print output
    ///
    /// # Returns
    /// * `RunProgress::ResolveFutures` - More futures need resolution
    /// * `RunProgress::FunctionCall` - VM hit another external call
    /// * `RunProgress::Complete` - All tasks completed successfully
    /// * `Err(MontyException)` - An unhandled exception occurred
    ///
    /// # Errors
    /// Returns `Err(MontyException)` if any call_id in `results` is not in the pending set.
    ///
    /// # Panics
    /// Panics if the VM state cannot be snapshotted (internal error).
    pub fn resume(
        self,
        results: Vec<(u32, ExternalResult)>,
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException> {
        use crate::exception_private::RunError;

        // Destructure self to avoid partial move issues
        let Self {
            executor,
            vm_state,
            mut heap,
            mut namespaces,
            pending_call_ids,
        } = self;

        // Validate that all provided call_ids are in the pending set before restoring VM
        let invalid_call_id = results
            .iter()
            .find(|(call_id, _)| !pending_call_ids.contains(call_id))
            .map(|(call_id, _)| *call_id);

        // Restore the VM from the snapshot (must happen before any error return to clean up properly)
        let mut vm = VM::restore(
            vm_state,
            &executor.module_code,
            &mut heap,
            &mut namespaces,
            &executor.interns,
            print,
        );

        // Now check for invalid call_ids after VM is restored
        if let Some(call_id) = invalid_call_id {
            vm.cleanup();
            #[cfg(feature = "ref-count-panic")]
            namespaces.drop_global_with_heap(&mut heap);
            return Err(MontyException::runtime_error(format!(
                "unknown call_id {call_id}, expected one of: {pending_call_ids:?}"
            )));
        }

        for (call_id, ext_result) in results {
            match ext_result {
                // Resolve successful futures in the scheduler
                ExternalResult::Return(obj) => vm.resolve_future(call_id, obj).map_err(|e| {
                    MontyException::runtime_error(format!("Invalid return type for call {call_id}: {e}"))
                })?,
                // Fail futures that returned errors
                ExternalResult::Error(exc) => vm.fail_future(call_id, RunError::from(exc)),
                // do nothing, same as not returning this id
                ExternalResult::Future => {}
            }
        }

        // Check if the current task has failed (e.g., external future failed for a gather).
        // If so, propagate the error immediately without continuing execution.
        if let Some(error) = vm.take_failed_task_error() {
            vm.cleanup();
            #[cfg(feature = "ref-count-panic")]
            namespaces.drop_global_with_heap(&mut heap);
            return Err(error.into_python_exception(&executor.interns, &executor.code));
        }

        // Push resolved value for main task if it was blocked.
        // Returns true if the main task was unblocked and a value was pushed.
        let main_task_ready = vm.prepare_main_task_after_resolve();

        // Load a ready task if frames are empty (e.g., gather completed while
        // tasks were running and we yielded with no frames)
        let loaded_task = match vm.load_ready_task_if_needed() {
            Ok(loaded) => loaded,
            Err(e) => {
                vm.cleanup();
                #[cfg(feature = "ref-count-panic")]
                namespaces.drop_global_with_heap(&mut heap);
                return Err(e.into_python_exception(&executor.interns, &executor.code));
            }
        };

        // Check if we can continue execution.
        // If the main task wasn't unblocked, no task was loaded, and there are still frames
        // (meaning the main task is still blocked waiting for futures), we need to return
        // ResolveFutures without calling vm.run().
        if !main_task_ready && !loaded_task {
            let pending_call_ids = vm.get_pending_call_ids();
            if !pending_call_ids.is_empty() {
                let vm_state = vm.snapshot();
                let pending_call_ids: Vec<u32> = pending_call_ids.iter().map(|id| id.raw()).collect();
                return Ok(RunProgress::ResolveFutures(Self {
                    executor,
                    vm_state,
                    heap,
                    namespaces,
                    pending_call_ids,
                }));
            }
        }

        // Continue execution
        let result = vm.run();

        let vm_state = vm.check_snapshot(&result);

        // Handle the result using the destructured parts
        handle_vm_result(result, vm_state, executor, heap, namespaces)
    }
}

/// Handles a FrameExit result and converts it to RunProgress for FutureSnapshot.
///
/// This is a standalone function to avoid partial move issues when destructuring FutureSnapshot.
#[cfg_attr(not(feature = "ref-count-panic"), expect(unused_mut))]
fn handle_vm_result<T: ResourceTracker>(
    result: RunResult<FrameExit>,
    vm_state: Option<VMSnapshot>,
    executor: Executor,
    mut heap: Heap<T>,
    mut namespaces: Namespaces,
) -> Result<RunProgress<T>, MontyException> {
    macro_rules! new_snapshot {
        ($call_id: expr) => {
            Snapshot {
                executor,
                vm_state: vm_state.expect("snapshot should exist for ExternalCall"),
                heap,
                namespaces,
                pending_call_id: $call_id.raw(),
            }
        };
    }

    match result {
        Ok(FrameExit::Return(value)) => {
            #[cfg(feature = "ref-count-panic")]
            namespaces.drop_global_with_heap(&mut heap);

            let obj = MontyObject::new(value, &mut heap, &executor.interns);
            Ok(RunProgress::Complete(obj))
        }
        Ok(FrameExit::ExternalCall {
            ext_function_id,
            args,
            call_id,
        }) => {
            let function_name = executor.interns.get_external_function_name(ext_function_id);
            let (args_py, kwargs_py) = args.into_py_objects(&mut heap, &executor.interns);

            Ok(RunProgress::FunctionCall {
                function_name,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
                state: new_snapshot!(call_id),
            })
        }
        Ok(FrameExit::OsCall {
            function,
            args,
            call_id,
        }) => {
            let (args_py, kwargs_py) = args.into_py_objects(&mut heap, &executor.interns);

            Ok(RunProgress::OsCall {
                function,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
                state: new_snapshot!(call_id),
            })
        }
        Ok(FrameExit::ResolveFutures(pending_call_ids)) => {
            let pending_call_ids: Vec<u32> = pending_call_ids.iter().map(|id| id.raw()).collect();
            Ok(RunProgress::ResolveFutures(FutureSnapshot {
                executor,
                vm_state: vm_state.expect("snapshot should exist for ResolveFutures"),
                heap,
                namespaces,
                pending_call_ids,
            }))
        }
        Err(err) => {
            #[cfg(feature = "ref-count-panic")]
            namespaces.drop_global_with_heap(&mut heap);

            Err(err.into_python_exception(&executor.interns, &executor.code))
        }
    }
}

/// Handles a FrameExit result and converts it to REPL progress.
///
/// This mirrors [`handle_vm_result`] but preserves REPL heap/namespaces on
/// completion by returning `ReplProgress::Complete { repl, value }`.
fn handle_repl_vm_result<T: ResourceTracker>(
    result: RunResult<FrameExit>,
    vm_state: Option<VMSnapshot>,
    executor: Executor,
    mut repl: MontyRepl<T>,
) -> Result<ReplProgress<T>, MontyException> {
    macro_rules! new_repl_snapshot {
        ($call_id: expr) => {
            ReplSnapshot {
                repl,
                executor,
                vm_state: vm_state.expect("snapshot should exist for ExternalCall"),
                pending_call_id: $call_id.raw(),
            }
        };
    }

    match result {
        Ok(FrameExit::Return(value)) => {
            let output = MontyObject::new(value, &mut repl.heap, &executor.interns);
            let Executor { name_map, interns, .. } = executor;
            repl.global_name_map = name_map;
            repl.interns = interns;
            Ok(ReplProgress::Complete { repl, value: output })
        }
        Ok(FrameExit::ExternalCall {
            ext_function_id,
            args,
            call_id,
        }) => {
            let function_name = executor.interns.get_external_function_name(ext_function_id);
            let (args_py, kwargs_py) = args.into_py_objects(&mut repl.heap, &executor.interns);

            Ok(ReplProgress::FunctionCall {
                function_name,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
                state: new_repl_snapshot!(call_id),
            })
        }
        Ok(FrameExit::OsCall {
            function,
            args,
            call_id,
        }) => {
            let (args_py, kwargs_py) = args.into_py_objects(&mut repl.heap, &executor.interns);

            Ok(ReplProgress::OsCall {
                function,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
                state: new_repl_snapshot!(call_id),
            })
        }
        Ok(FrameExit::ResolveFutures(pending_call_ids)) => {
            let pending_call_ids: Vec<u32> = pending_call_ids.iter().map(|id| id.raw()).collect();
            Ok(ReplProgress::ResolveFutures(ReplFutureSnapshot {
                repl,
                executor,
                vm_state: vm_state.expect("snapshot should exist for ResolveFutures"),
                pending_call_ids,
            }))
        }
        Err(err) => {
            #[cfg(feature = "ref-count-panic")]
            repl.namespaces.drop_global_with_heap(&mut repl.heap);

            Err(err.into_python_exception(&executor.interns, &executor.code))
        }
    }
}

/// Lower level interface to parse code and run it to completion.
///
/// This is an internal type used by [`MontyRun`]. It stores the compiled bytecode and source code
/// for error reporting.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Executor {
    /// Number of slots needed in the global namespace.
    namespace_size: usize,
    /// Maps variable names to their indices in the namespace.
    ///
    /// Used both for ref-count testing and incremental REPL compilation where
    /// slot assignments must stay stable across snippets.
    name_map: AHashMap<String, NamespaceId>,
    /// Compiled bytecode for the module.
    module_code: Code,
    /// Interned strings used for looking up names and filenames during execution.
    interns: Interns,
    /// IDs to create values to inject into the the namespace to represent external functions.
    external_function_ids: Vec<ExtFunctionId>,
    /// Source code for error reporting (extracting preview lines for tracebacks).
    code: String,
    /// Estimated heap capacity for pre-allocation on subsequent runs.
    /// Uses AtomicUsize for thread-safety (required by PyO3's Sync bound).
    heap_capacity: AtomicUsize,
}

impl Clone for Executor {
    fn clone(&self) -> Self {
        Self {
            namespace_size: self.namespace_size,
            name_map: self.name_map.clone(),
            module_code: self.module_code.clone(),
            interns: self.interns.clone(),
            external_function_ids: self.external_function_ids.clone(),
            code: self.code.clone(),
            heap_capacity: AtomicUsize::new(self.heap_capacity.load(Ordering::Relaxed)),
        }
    }
}

impl Executor {
    /// Creates a new executor with the given code, filename, input names, and external functions.
    fn new(
        code: String,
        script_name: &str,
        input_names: Vec<String>,
        external_functions: Vec<String>,
    ) -> Result<Self, MontyException> {
        let parse_result = parse(&code, script_name).map_err(|e| e.into_python_exc(script_name, &code))?;
        let prepared = prepare(parse_result, input_names, &external_functions)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        // Incrementing order matches the indexes used in intern::Interns::get_external_function_name
        let external_function_ids = (0..external_functions.len()).map(ExtFunctionId::new).collect();

        // Create interns with empty functions (functions will be set after compilation)
        let mut interns = Interns::new(prepared.interner, Vec::new(), external_functions);

        // Compile the module to bytecode, which also compiles all nested functions
        let namespace_size_u16 = u16::try_from(prepared.namespace_size).expect("module namespace size exceeds u16");
        let compile_result = Compiler::compile_module(&prepared.nodes, &interns, namespace_size_u16)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        // Set the compiled functions in the interns
        interns.set_functions(compile_result.functions);

        Ok(Self {
            namespace_size: prepared.namespace_size,
            name_map: prepared.name_map,
            module_code: compile_result.code,
            interns,
            external_function_ids,
            code,
            heap_capacity: AtomicUsize::new(prepared.namespace_size),
        })
    }

    /// Compiles one incremental REPL snippet against existing session metadata.
    ///
    /// This differs from [`Executor::new`] in three ways that are required for a
    /// true no-replay REPL:
    /// - Seeds parsing from `existing_interns`, so old `StringId`/literal IDs remain stable.
    /// - Seeds compilation from `existing_interns` functions, so old `FunctionId` values
    ///   stored on the heap/global namespace remain valid.
    /// - Reuses `existing_name_map` as the starting global slot map, appending only new names.
    ///
    /// The returned executor's intern/function tables are supersets of the prior
    /// session tables, preserving ID compatibility with previously executed snippets.
    fn new_repl_snippet(
        code: String,
        script_name: &str,
        external_functions: Vec<String>,
        existing_name_map: AHashMap<String, NamespaceId>,
        existing_interns: &Interns,
    ) -> Result<Self, MontyException> {
        let seeded_interner = InternerBuilder::from_interns(existing_interns, &code);
        let parse_result = parse_with_interner(&code, script_name, seeded_interner)
            .map_err(|e| e.into_python_exc(script_name, &code))?;
        let prepared = prepare_with_existing_names(parse_result, existing_name_map)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        // Must match ext function index semantics used by Interns::get_external_function_name.
        let external_function_ids = (0..external_functions.len()).map(ExtFunctionId::new).collect();

        let existing_functions = existing_interns.functions_clone();
        let mut interns = Interns::new(prepared.interner, Vec::new(), external_functions);
        let namespace_size_u16 = u16::try_from(prepared.namespace_size).expect("module namespace size exceeds u16");
        let compile_result =
            Compiler::compile_module_with_functions(&prepared.nodes, &interns, namespace_size_u16, existing_functions)
                .map_err(|e| e.into_python_exc(script_name, &code))?;
        interns.set_functions(compile_result.functions);

        Ok(Self {
            namespace_size: prepared.namespace_size,
            name_map: prepared.name_map,
            module_code: compile_result.code,
            interns,
            external_function_ids,
            code,
            heap_capacity: AtomicUsize::new(prepared.namespace_size),
        })
    }

    /// Executes the code with a custom resource tracker.
    ///
    /// This provides full control over resource tracking and garbage collection
    /// scheduling. The tracker is called on each allocation and periodically
    /// during execution to check time limits and trigger GC.
    ///
    /// # Arguments
    /// * `inputs` - Values to fill the first N slots of the namespace
    /// * `resource_tracker` - Custom resource tracker implementation
    /// * `print` - Print implementation for print() output
    fn run(
        &self,
        inputs: Vec<MontyObject>,
        resource_tracker: impl ResourceTracker,
        print: &mut impl PrintWriter,
    ) -> Result<MontyObject, MontyException> {
        let heap_capacity = self.heap_capacity.load(Ordering::Relaxed);
        let mut heap = Heap::new(heap_capacity, resource_tracker);
        let mut namespaces = self.prepare_namespaces(inputs, &mut heap)?;

        // Create and run VM
        let mut vm = VM::new(&mut heap, &mut namespaces, &self.interns, print);
        let frame_exit_result = vm.run_module(&self.module_code);

        // Clean up VM state before it goes out of scope
        vm.cleanup();

        if heap.size() > heap_capacity {
            self.heap_capacity.store(heap.size(), Ordering::Relaxed);
        }

        // Clean up the global namespace before returning (only needed with ref-count-panic)
        #[cfg(feature = "ref-count-panic")]
        namespaces.drop_global_with_heap(&mut heap);

        frame_exit_to_object(frame_exit_result, &mut heap, &self.interns)
            .map_err(|e| e.into_python_exception(&self.interns, &self.code))
    }

    /// Executes the code and returns both the result and reference count data, used for testing only.
    ///
    /// This is used for testing reference counting behavior. Returns:
    /// - The execution result (`Exit`)
    /// - Reference count data as a tuple of:
    ///   - A map from variable names to their reference counts (only for heap-allocated values)
    ///   - The number of unique heap value IDs referenced by variables
    ///   - The total number of live heap values
    ///
    /// For strict matching validation, compare unique_refs_count with heap_entry_count.
    /// If they're equal, all heap values are accounted for by named variables.
    ///
    /// Only available when the `ref-count-return` feature is enabled.
    #[cfg(feature = "ref-count-return")]
    fn run_ref_counts(&self, inputs: Vec<MontyObject>) -> Result<RefCountOutput, MontyException> {
        use std::collections::HashSet;

        let mut heap = Heap::new(self.namespace_size, NoLimitTracker);
        let mut namespaces = self.prepare_namespaces(inputs, &mut heap)?;

        // Create and run VM with StdPrint for output
        let mut print = StdPrint;
        let mut vm = VM::new(&mut heap, &mut namespaces, &self.interns, &mut print);
        let frame_exit_result = vm.run_module(&self.module_code);

        // Compute ref counts before consuming the heap - return value is still alive
        let final_namespace = namespaces.into_global();
        let mut counts = ahash::AHashMap::new();
        let mut unique_ids = HashSet::new();

        for (name, &namespace_id) in &self.name_map {
            if let Some(Value::Ref(id)) = final_namespace.get_opt(namespace_id) {
                counts.insert(name.clone(), heap.get_refcount(*id));
                unique_ids.insert(*id);
            }
        }
        let unique_refs = unique_ids.len();
        let heap_count = heap.entry_count();

        // Clean up the namespace after reading ref counts but before moving the heap
        for obj in final_namespace {
            obj.drop_with_heap(&mut heap);
        }

        // Now convert the return value to MontyObject (this drops the Value, decrementing refcount)
        let py_object = frame_exit_to_object(frame_exit_result, &mut heap, &self.interns)
            .map_err(|e| e.into_python_exception(&self.interns, &self.code))?;

        let allocations_since_gc = heap.get_allocations_since_gc();

        Ok(RefCountOutput {
            py_object,
            counts,
            unique_refs,
            heap_count,
            allocations_since_gc,
        })
    }

    /// Prepares the namespace namespaces for execution.
    ///
    /// Converts each `MontyObject` input to a `Value`, allocating on the heap if needed.
    /// Returns the prepared Namespaces or an error if there are too many inputs or invalid input types.
    fn prepare_namespaces(
        &self,
        inputs: Vec<MontyObject>,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Namespaces, MontyException> {
        let Some(extra) = self
            .namespace_size
            .checked_sub(self.external_function_ids.len() + inputs.len())
        else {
            return Err(MontyException::runtime_error("too many inputs for namespace"));
        };
        // register external functions in the namespace first, matching the logic in prepare
        let mut namespace: Vec<Value> = Vec::with_capacity(self.namespace_size);
        for f_id in &self.external_function_ids {
            namespace.push(Value::ExtFunction(*f_id));
        }
        // Convert each MontyObject to a Value, propagating any invalid input errors
        for input in inputs {
            namespace.push(
                input
                    .to_value(heap, &self.interns)
                    .map_err(|e| MontyException::runtime_error(format!("invalid input type: {e}")))?,
            );
        }
        if extra > 0 {
            namespace.extend((0..extra).map(|_| Value::Undefined));
        }
        Ok(Namespaces::new(namespace))
    }
}

fn frame_exit_to_object(
    frame_exit_result: RunResult<FrameExit>,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<MontyObject> {
    match frame_exit_result? {
        FrameExit::Return(return_value) => Ok(MontyObject::new(return_value, heap, interns)),
        FrameExit::ExternalCall { ext_function_id, .. } => {
            let function_name = interns.get_external_function_name(ext_function_id);
            Err(ExcType::not_implemented(format!(
                "External function '{function_name}' not implemented with standard execution"
            ))
            .into())
        }
        FrameExit::OsCall { function, .. } => Err(ExcType::not_implemented(format!(
            "OS function '{function}' not implemented with standard execution"
        ))
        .into()),
        FrameExit::ResolveFutures(_) => {
            Err(ExcType::not_implemented("async futures not supported by standard execution.").into())
        }
    }
}

/// Output from `run_ref_counts` containing reference count and heap information.
///
/// Used for testing GC behavior and reference counting correctness.
#[cfg(feature = "ref-count-return")]
#[derive(Debug)]
pub struct RefCountOutput {
    pub py_object: MontyObject,
    pub counts: ahash::AHashMap<String, usize>,
    pub unique_refs: usize,
    pub heap_count: usize,
    /// Number of GC-tracked allocations since the last garbage collection.
    ///
    /// If GC ran during execution, this will be lower than the total number of
    /// allocations. Compare this against expected allocation count to verify GC ran.
    pub allocations_since_gc: u32,
}
