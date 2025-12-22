use crate::args::ArgValues;
use crate::evaluate::{EvalResult, EvaluateExpr};
use crate::exceptions::{
    exc_err_static, exc_fmt, internal_err, ExcType, InternalRunError, RunError, SimpleException, StackFrame,
};
use crate::expressions::{ExprLoc, Identifier, NameScope, Node};
use crate::heap::{Heap, HeapData};
use crate::intern::{FunctionId, Interns, StringId, MODULE_STRING_ID};
use crate::io::PrintWriter;
use crate::namespace::{NamespaceId, Namespaces, GLOBAL_NS_IDX};
use crate::operators::Operator;
use crate::parse::CodeRange;
use crate::position::{AbstractPositionTracker, ClauseState, FrameExit};
use crate::resource::ResourceTracker;
use crate::types::PyTrait;
use crate::value::Value;

/// Result type for runtime operations.
pub type RunResult<T> = Result<T, RunError>;

/// Represents an execution frame with an index into Namespaces.
///
/// At module level, `local_idx == GLOBAL_NS_IDX` (same namespace).
/// In functions, `local_idx` points to the function's local namespace.
/// Global variables always use `GLOBAL_NS_IDX` (0) directly.
///
/// # Closure Support
///
/// Cell variables (for closures) are stored directly in the namespace as
/// `Value::Ref(cell_id)` pointing to a `HeapData::Cell`. Both captured cells
/// (from enclosing scopes) and owned cells (for variables captured by nested
/// functions) are injected into the namespace at function call time.
///
/// When accessing a variable with `NameScope::Cell`, we look up the namespace
/// slot to get the `Value::Ref(cell_id)`, then read/write through that cell.
#[derive(Debug)]
pub struct RunFrame<'i, P: AbstractPositionTracker, W: PrintWriter> {
    /// Index of this frame's local namespace in Namespaces.
    local_idx: NamespaceId,
    /// Parent stack frame for error reporting.
    parent: Option<StackFrame>,
    /// The name of the current frame (function name or "<module>").
    /// Uses string id to lookup
    name: StringId,
    /// reference to interns
    interns: &'i Interns,
    /// reference to position tracker
    position_tracker: &'i mut P,
    /// Writer for print output
    writer: &'i mut W,
}

/// Extracts a value from `EvalResult`, returning early with `FrameExit::ExternalCall` if
/// an external call is pending.
///
/// Similar to `return_ext_call!` from evaluate.rs, but returns `Ok(Some(FrameExit::ExternalCall(...)))`
/// which is the appropriate return type for `execute_node` and related methods.
macro_rules! frame_ext_call {
    ($expr:expr) => {
        match $expr {
            EvalResult::Value(value) => value,
            EvalResult::ExternalCall(ext_call) => return Ok(Some(FrameExit::ExternalCall(ext_call))),
        }
    };
}

impl<'i, P: AbstractPositionTracker, W: PrintWriter> RunFrame<'i, P, W> {
    /// Creates a new frame for module-level execution.
    ///
    /// At module level, `local_idx` is `GLOBAL_NS_IDX` (0).
    pub fn module_frame(interns: &'i Interns, position_tracker: &'i mut P, writer: &'i mut W) -> Self {
        Self {
            local_idx: GLOBAL_NS_IDX,
            parent: None,
            name: MODULE_STRING_ID,
            interns,
            position_tracker,
            writer,
        }
    }

    /// Creates a new frame for function execution.
    ///
    /// The function's local namespace is at `local_idx`. Global variables
    /// always use `GLOBAL_NS_IDX` directly.
    ///
    /// Cell variables (for closures) are already injected into the namespace
    /// by Function::call or Function::call_with_cells before this frame is created.
    ///
    /// # Arguments
    /// * `local_idx` - Index of the function's local namespace in Namespaces
    /// * `name` - The function name StringId (for error messages)
    /// * `parent` - Parent stack frame for error traceback
    /// * `position_tracker` - Tracker for the current position in the code
    /// * `writer` - Writer for print output
    pub fn function_frame(
        local_idx: NamespaceId,
        name: StringId,
        parent: Option<StackFrame>,
        interns: &'i Interns,
        position_tracker: &'i mut P,
        writer: &'i mut W,
    ) -> Self {
        Self {
            local_idx,
            parent,
            name,
            interns,
            position_tracker,
            writer,
        }
    }

    /// Executes all nodes in sequence, returning when a frame exit (return/yield) occurs.
    ///
    /// This will use `PositionTracker` to manage where in the block to resume execution.
    ///
    /// # Arguments
    /// * `namespaces` - The namespace stack
    /// * `heap` - The heap for allocations
    /// * `nodes` - The AST nodes to execute
    pub fn execute(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        nodes: &[Node],
    ) -> RunResult<Option<FrameExit>> {
        // The first position must be an Index - it tells us where to start in this block
        let position = self.position_tracker.next();
        let start_index = position.index;
        let mut clause_state = position.clause_state;

        // execute from start_index
        for (i, node) in nodes.iter().enumerate().skip(start_index) {
            // External calls are returned as Ok(Some(FrameExit::ExternalCall(...))) from execute_node
            let exit_frame = self.execute_node(namespaces, heap, node, clause_state)?;
            if let Some(exit) = exit_frame {
                // Set the index of the node to execute on resume
                // we will have called set_skip() already if we need to skip the current node
                self.position_tracker.record(i);
                return Ok(Some(exit));
            }
            clause_state = None;

            // if enabled, clear return values after executing each node
            if P::clear_return_values() {
                namespaces.clear_return_values(heap);
            }
        }
        Ok(None)
    }

    /// Executes a single node, returning exit info with positions if execution should stop.
    ///
    /// Returns `Some(exit)` if the node caused a yield/return, where:
    /// - `exit` is the FrameExit (Yield or Return)
    /// - `positions` is the position stack within this node (empty for simple yields/returns)
    fn execute_node(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        node: &Node,
        clause_state: Option<ClauseState>,
    ) -> RunResult<Option<FrameExit>> {
        // Check time limit at statement boundaries
        heap.tracker().check_time()?;

        // Trigger garbage collection if scheduler says it's time.
        // GC runs at statement boundaries because:
        // 1. This is a natural pause point where we have access to GC roots
        // 2. The namespace state is stable (not mid-expression evaluation)
        // Note: GC won't run during long-running single expressions (e.g., large list
        // comprehensions). This is acceptable because most Python code is structured
        // as multiple statements, and resource limits (time, memory) still apply.
        if heap.tracker().should_gc() {
            heap.collect_garbage(|| namespaces.iter_heap_ids());
        }

        match node {
            Node::Expr(expr) => {
                match EvaluateExpr::new(namespaces, self.local_idx, heap, self.interns, self.writer)
                    .evaluate_discard(expr)
                {
                    Ok(EvalResult::Value(())) => {}
                    Ok(EvalResult::ExternalCall(ext_call)) => return Ok(Some(FrameExit::ExternalCall(ext_call))),
                    Err(mut e) => {
                        set_name(self.name, &mut e);
                        return Err(e);
                    }
                }
            }
            Node::Return(expr) => {
                return self.execute_expr(namespaces, heap, expr).map(|result| match result {
                    EvalResult::Value(value) => Some(FrameExit::Return(value)),
                    EvalResult::ExternalCall(ext_call) => Some(FrameExit::ExternalCall(ext_call)),
                })
            }
            Node::ReturnNone => return Ok(Some(FrameExit::Return(Value::None))),
            Node::Raise(exc) => {
                if let Some(exit) = self.raise(namespaces, heap, exc.as_ref())? {
                    return Ok(Some(exit));
                }
            }
            Node::Assert { test, msg } => {
                if let Some(exit) = self.assert_(namespaces, heap, test, msg.as_ref())? {
                    return Ok(Some(exit));
                }
            }
            Node::Assign { target, object } => {
                if let Some(exit) = self.assign(namespaces, heap, target, object)? {
                    return Ok(Some(exit));
                }
            }
            Node::OpAssign { target, op, object } => {
                if let Some(exit) = self.op_assign(namespaces, heap, target, op, object)? {
                    return Ok(Some(exit));
                }
            }
            Node::SubscriptAssign { target, index, value } => {
                if let Some(exit) = self.subscript_assign(namespaces, heap, target, index, value)? {
                    return Ok(Some(exit));
                }
            }
            Node::For {
                target,
                iter,
                body,
                or_else,
            } => {
                let start_index = match clause_state {
                    Some(ClauseState::For(resume_index)) => resume_index,
                    _ => 0,
                };
                if let Some(exit_frame) = self.for_(namespaces, heap, target, iter, body, or_else, start_index)? {
                    return Ok(Some(exit_frame));
                }
            }
            Node::If { test, body, or_else } => {
                let if_test = match clause_state {
                    Some(ClauseState::If(resume_test)) => &resume_test.into(),
                    _ => test,
                };
                if let Some(exit_frame) = self.if_(namespaces, heap, if_test, body, or_else)? {
                    return Ok(Some(exit_frame));
                }
            }
            Node::FunctionDef(function_id) => self.define_function(namespaces, heap, *function_id)?,
        }
        Ok(None)
    }

    /// Evaluates an expression and returns a Value.
    fn execute_expr(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        expr: &ExprLoc,
    ) -> RunResult<EvalResult<Value>> {
        match EvaluateExpr::new(namespaces, self.local_idx, heap, self.interns, self.writer).evaluate_use(expr) {
            Ok(value) => Ok(value),
            Err(mut e) => {
                set_name(self.name, &mut e);
                Err(e)
            }
        }
    }

    fn execute_expr_bool(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        expr: &ExprLoc,
    ) -> RunResult<EvalResult<bool>> {
        match EvaluateExpr::new(namespaces, self.local_idx, heap, self.interns, self.writer).evaluate_bool(expr) {
            Ok(value) => Ok(value),
            Err(mut e) => {
                set_name(self.name, &mut e);
                Err(e)
            }
        }
    }

    /// Executes a raise statement.
    ///
    /// Handles:
    /// * Exception instance (Value::Exc) - raise directly
    /// * Exception type (Value::Callable with ExcType) - instantiate then raise
    /// * Anything else - TypeError
    fn raise(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        op_exc_expr: Option<&ExprLoc>,
    ) -> RunResult<Option<FrameExit>> {
        if let Some(exc_expr) = op_exc_expr {
            let value = frame_ext_call!(self.execute_expr(namespaces, heap, exc_expr)?);
            match &value {
                Value::Exc(_) => {
                    // Match on the reference then use into_exc() due to issues with destructuring Value
                    let exc = value.into_exc();
                    return Err(exc.with_frame(self.stack_frame(exc_expr.position)).into());
                }
                Value::Builtin(builtin) => {
                    // Callable is inline - call it to get the exception
                    let builtin = *builtin;
                    let result = builtin.call(heap, ArgValues::Empty, self.interns, self.writer)?;
                    if matches!(&result, Value::Exc(_)) {
                        // No need to drop value - Callable is Copy and doesn't need cleanup
                        let exc = result.into_exc();
                        return Err(exc.with_frame(self.stack_frame(exc_expr.position)).into());
                    }
                }
                _ => {}
            }
            value.drop_with_heap(heap);
            exc_err_static!(ExcType::TypeError; "exceptions must derive from BaseException")
        } else {
            internal_err!(InternalRunError::TodoError; "plain raise not yet supported")
        }
    }

    /// Executes an assert statement by evaluating the test expression and raising
    /// `AssertionError` if the test is falsy.
    ///
    /// If a message expression is provided, it is evaluated and used as the exception message.
    fn assert_(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        test: &ExprLoc,
        msg: Option<&ExprLoc>,
    ) -> RunResult<Option<FrameExit>> {
        let ok = frame_ext_call!(self.execute_expr_bool(namespaces, heap, test)?);
        if !ok {
            let msg = if let Some(msg_expr) = msg {
                let msg_value = frame_ext_call!(self.execute_expr(namespaces, heap, msg_expr)?);
                Some(msg_value.py_str(heap, self.interns).to_string())
            } else {
                None
            };
            return Err(SimpleException::new(ExcType::AssertionError, msg)
                .with_frame(self.stack_frame(test.position))
                .into());
        }
        Ok(None)
    }

    fn assign(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        target: &Identifier,
        expr: &ExprLoc,
    ) -> RunResult<Option<FrameExit>> {
        let new_value = frame_ext_call!(self.execute_expr(namespaces, heap, expr)?);

        // Determine which namespace to use
        let ns_idx = match target.scope {
            NameScope::Global => GLOBAL_NS_IDX,
            _ => self.local_idx, // Local and Cell both use local namespace
        };

        if target.scope == NameScope::Cell {
            // Cell assignment - look up cell HeapId from namespace slot, then write through it
            let namespace = namespaces.get_mut(ns_idx);
            let Value::Ref(cell_id) = namespace.get(target.namespace_id()) else {
                panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug")
            };
            heap.set_cell_value(*cell_id, new_value);
        } else {
            // Direct assignment to namespace slot (Local or Global)
            let namespace = namespaces.get_mut(ns_idx);
            let old_value = std::mem::replace(namespace.get_mut(target.namespace_id()), new_value);
            old_value.drop_with_heap(heap);
        }
        Ok(None)
    }

    fn op_assign(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        target: &Identifier,
        op: &Operator,
        expr: &ExprLoc,
    ) -> RunResult<Option<FrameExit>> {
        let rhs = frame_ext_call!(self.execute_expr(namespaces, heap, expr)?);
        // Capture rhs type before it's consumed
        let rhs_type = rhs.py_type(Some(heap));

        // Cell variables need special handling - read through cell, modify, write back
        let err_target_type = if target.scope == NameScope::Cell {
            let namespace = namespaces.get_mut(self.local_idx);
            let Value::Ref(cell_id) = namespace.get(target.namespace_id()) else {
                panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug")
            };
            let mut cell_value = heap.get_cell_value(*cell_id);
            // Capture type before potential drop
            let cell_value_type = cell_value.py_type(Some(heap));
            let result: RunResult<Option<Value>> = match op {
                // In-place add has special optimization for mutable types
                Operator::Add => {
                    let ok = cell_value.py_iadd(rhs, heap, None, self.interns)?;
                    if ok {
                        Ok(Some(cell_value))
                    } else {
                        Ok(None)
                    }
                }
                // For other operators, use binary op + replace
                Operator::Mult => {
                    let new_val = cell_value.py_mult(&rhs, heap, self.interns)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Div => {
                    let new_val = cell_value.py_div(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::FloorDiv => {
                    let new_val = cell_value.py_floordiv(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Pow => {
                    let new_val = cell_value.py_pow(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Sub => {
                    let new_val = cell_value.py_sub(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Mod => {
                    let new_val = cell_value.py_mod(&rhs);
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                _ => return internal_err!(InternalRunError::TodoError; "Assign operator {op:?} not yet implemented"),
            };
            match result? {
                Some(new_value) => {
                    heap.set_cell_value(*cell_id, new_value);
                    None
                }
                None => Some(cell_value_type),
            }
        } else {
            // Direct access for Local/Global scopes
            let target_val = namespaces.get_var_mut(self.local_idx, target, self.interns)?;
            let target_type = target_val.py_type(Some(heap));
            let result: RunResult<Option<()>> = match op {
                // In-place add has special optimization for mutable types
                Operator::Add => {
                    let ok = target_val.py_iadd(rhs, heap, None, self.interns)?;
                    if ok {
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                // For other operators, use binary op + replace
                Operator::Mult => {
                    let new_val = target_val.py_mult(&rhs, heap, self.interns)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Div => {
                    let new_val = target_val.py_div(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::FloorDiv => {
                    let new_val = target_val.py_floordiv(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Pow => {
                    let new_val = target_val.py_pow(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Sub => {
                    let new_val = target_val.py_sub(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Mod => {
                    let new_val = target_val.py_mod(&rhs);
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                _ => return internal_err!(InternalRunError::TodoError; "Assign operator {op:?} not yet implemented"),
            };
            match result? {
                Some(()) => None,
                None => Some(target_type),
            }
        };

        if let Some(target_type) = err_target_type {
            let e = SimpleException::augmented_assign_type_error(op, target_type, rhs_type);
            Err(e.with_frame(self.stack_frame(expr.position)).into())
        } else {
            Ok(None)
        }
    }

    fn subscript_assign(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        target: &Identifier,
        index: &ExprLoc,
        value: &ExprLoc,
    ) -> RunResult<Option<FrameExit>> {
        let key = frame_ext_call!(self.execute_expr(namespaces, heap, index)?);
        let val = frame_ext_call!(self.execute_expr(namespaces, heap, value)?);
        let target_val = namespaces.get_var_mut(self.local_idx, target, self.interns)?;
        if let Value::Ref(id) = target_val {
            let id = *id;
            heap.with_entry_mut(id, |heap, data| data.py_setitem(key, val, heap, self.interns))?;
            Ok(None)
        } else {
            let e = exc_fmt!(ExcType::TypeError; "'{}' object does not support item assignment", target_val.py_type(Some(heap)));
            Err(e.with_frame(self.stack_frame(index.position)).into())
        }
    }

    /// Executes a for loop, propagating any `FrameExit` (yield/return) from the body.
    ///
    /// Returns `Some(FrameExit)` if a yield or explicit return occurred in the body,
    /// `None` if the loop completed normally.
    ///
    /// # Note on Yield Resumption
    ///
    /// TODO: For loop resumption after yield is not yet supported. Currently, yielding
    /// inside a for loop will detect the yield and return it, but resumption will
    /// continue after the entire for loop rather than from within the loop body.
    /// Supporting this requires tracking the loop iteration index in the position stack.
    #[allow(clippy::too_many_arguments)]
    fn for_(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        target: &Identifier,
        iter: &ExprLoc,
        body: &[Node],
        _or_else: &[Node],
        start_index: usize,
    ) -> RunResult<Option<FrameExit>> {
        let iter_value = frame_ext_call!(self.execute_expr(namespaces, heap, iter)?);
        let Value::Range(range) = iter_value else {
            return internal_err!(InternalRunError::TodoError; "`for` iter must be a range");
        };

        let namespace_id = target.namespace_id();
        for value in range.iter().skip(start_index) {
            // For loop target is always local scope
            let namespace = namespaces.get_mut(self.local_idx);
            namespace.set(namespace_id, Value::Int(value));
            if let Some(exit) = self.execute(namespaces, heap, body)? {
                self.position_tracker.set_clause_state(ClauseState::For(value as usize));
                return Ok(Some(exit));
            }
        }
        Ok(None)
    }

    /// Executes an if statement.
    ///
    /// Evaluates the test condition and executes the appropriate branch.
    fn if_(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        test: &ExprLoc,
        body: &[Node],
        or_else: &[Node],
    ) -> RunResult<Option<FrameExit>> {
        if frame_ext_call!(self.execute_expr_bool(namespaces, heap, test)?) {
            if let Some(frame_exit) = self.execute(namespaces, heap, body)? {
                self.position_tracker.set_clause_state(ClauseState::If(true));
                return Ok(Some(frame_exit));
            }
        } else if let Some(frame_exit) = self.execute(namespaces, heap, or_else)? {
            self.position_tracker.set_clause_state(ClauseState::If(false));
            return Ok(Some(frame_exit));
        }
        Ok(None)
    }

    /// Defines a function (or closure) by storing it in the namespace.
    ///
    /// If the function has free_var_enclosing_slots (captures variables from enclosing scope),
    /// this captures the cells from the enclosing namespace and stores a Closure.
    /// If the function has default values, they are evaluated at definition time and stored.
    /// Otherwise, it stores a simple Function reference.
    ///
    /// # Cell Sharing
    ///
    /// Closures share cells with their enclosing scope. The cell HeapIds are
    /// looked up from the enclosing namespace slots specified in free_var_enclosing_slots.
    /// This ensures modifications through `nonlocal` are visible to both scopes.
    fn define_function(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        function_id: FunctionId,
    ) -> RunResult<()> {
        let function = self.interns.get_function(function_id);

        // Evaluate default expressions at definition time
        // These are evaluated in the enclosing scope (not the function's own scope)
        let defaults = if function.has_defaults() {
            let mut defaults = Vec::with_capacity(function.default_exprs.len());
            for expr in &function.default_exprs {
                match self.execute_expr(namespaces, heap, expr) {
                    Ok(EvalResult::Value(value)) => defaults.push(value),
                    Ok(EvalResult::ExternalCall(_)) => {
                        // External calls in default expressions are not supported
                        for value in defaults.drain(..) {
                            value.drop_with_heap(heap);
                        }
                        return Err(ExcType::not_implemented(
                            "external function calls in default parameter expressions",
                        )
                        .into());
                    }
                    Err(err) => {
                        for value in defaults.drain(..) {
                            value.drop_with_heap(heap);
                        }
                        return Err(err);
                    }
                }
            }
            defaults
        } else {
            Vec::new()
        };

        let new_value = if function.is_closure() {
            // This function captures variables from enclosing scopes.
            // Look up the cell HeapIds from the enclosing namespace.
            let enclosing_namespace = namespaces.get(self.local_idx);
            let mut captured_cells = Vec::with_capacity(function.free_var_enclosing_slots.len());

            for &enclosing_slot in &function.free_var_enclosing_slots {
                // The enclosing namespace slot contains Value::Ref(cell_id)
                let Value::Ref(cell_id) = enclosing_namespace.get(enclosing_slot) else {
                    panic!("Expected cell in enclosing namespace slot {enclosing_slot:?} - prepare-time bug")
                };

                // Increment the cell's refcount since this closure now holds a reference
                heap.inc_ref(*cell_id);
                captured_cells.push(*cell_id);
            }

            Value::Ref(heap.allocate(HeapData::Closure(function_id, captured_cells, defaults))?)
        } else if !defaults.is_empty() {
            // Non-closure function with defaults needs heap allocation
            Value::Ref(heap.allocate(HeapData::FunctionDefaults(function_id, defaults))?)
        } else {
            // Simple function without captures or defaults
            Value::Function(function_id)
        };

        let namespace = namespaces.get_mut(self.local_idx);
        let old_value = std::mem::replace(namespace.get_mut(function.name.namespace_id()), new_value);
        // Drop the old value properly (dec_ref for Refs, no-op for others)
        old_value.drop_with_heap(heap);
        Ok(())
    }

    fn stack_frame(&self, position: CodeRange) -> StackFrame {
        StackFrame::new(position, self.name, self.parent.as_ref())
    }
}

fn set_name(name: StringId, error: &mut RunError) {
    if let RunError::Exc(ref mut exc) = error {
        if let Some(ref mut stack_frame) = exc.frame {
            stack_frame.frame_name = Some(name);
        }
    }
}
