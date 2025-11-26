# How Monty Works

This document provides a comprehensive technical explanation of the Monty Python interpreter for engineers familiar with both Rust and Python.

## Table of Contents
1. [Project Goals and Design Philosophy](#project-goals-and-design-philosophy)
2. [High-Level Architecture](#high-level-architecture)
3. [Execution Pipeline Deep Dive](#execution-pipeline-deep-dive)
4. [Data Structures and Lifetimes](#data-structures-and-lifetimes)
5. [Module Reference](#module-reference)
6. [Example Walkthrough](#example-walkthrough)
7. [Advanced Topics](#advanced-topics)
8. [Recent Feature Development](#recent-feature-development)
9. [Critical Analysis: Design Limitations and Trade-offs](#critical-analysis-design-limitations-and-trade-offs)

## Project Goals and Design Philosophy

Monty is a **sandboxed Python interpreter** written in Rust. Unlike embedding CPython or using PyO3, Monty implements its own runtime from scratch with these goals:

- **Safety**: Execute untrusted Python code safely without FFI or C dependencies, instead sandbox will call back to host to run foreign/external functions.
- **Performance**: Fast execution through compile-time optimizations and efficient memory layout
- **Simplicity**: Clean, understandable implementation focused on a Python subset
- **Snapshotting and iteration**: Plan is to allow code to be iteratively executed and snapshotted at each function call

**Key Design Decisions:**

1. **Use Ruff's Parser**: Leverage the maintained `ruff_python_parser` infrastructure
2. **Custom Runtime**: Full control over execution model and optimizations
3. **No Garbage Collection**: Rust ownership handles memory (current limitation: no reference cycles)
4. **Compile-time Optimization**: Aggressive constant folding and dead code elimination
5. **Zero-Copy Parsing**: Lifetime `'c` ties AST to source string, avoiding allocations

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         User Code                               │
│  Executor::new(code, filename, inputs) → Executor::run(values)  │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                      PHASE 1: PARSE                             │
│  Ruff Parser → Internal AST (Node<'c>, Expr<'c>)                │
│  - Convert Python AST to internal representation                │
│  - Track code positions (CodeRange) for error messages          │
│  - Store code snippets for tracebacks                           │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                      PHASE 2: PREPARE                           │
│  Optimization Pass → Executor<'c>                               │
│  - Name resolution: variable names → numeric IDs                │
│  - Constant folding: evaluate 1+1 → 2 at compile time          │
│  - Dead code elimination: remove Pass, constant if branches     │
│  - Special optimizations: (x%y)==z → ModEq                      │
│  - Pre-allocate namespace vector                                │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                      PHASE 3: EXECUTE                           │
│  RunFrame<'c> interprets optimized AST                          │
│  - Execute statements (run.rs)                                  │
│  - Evaluate expressions (evaluate.rs)                           │
│  - Manage namespace and stack frames                            │
│  - Handle control flow and exceptions                           │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
                   ┌────────────────┐
                   │  Exit<'c>      │
                   │  - ReturnNone  │
                   │  - Return(obj) │
                   │  - Raise(exc)  │
                   └────────────────┘
```

## Execution Pipeline Deep Dive

### Phase 1: Parsing (parse.rs)

**Input**: `&str` (Python source code)
**Output**: `Vec<Node<'c>>` (statement AST nodes)

The parser converts Python source into an internal AST representation:

```rust
pub enum Node<'c> {
    Pass,                                    // No-op statement
    Expr(ExprLoc<'c>),                      // Expression statement
    Return(ExprLoc<'c>),                    // return expr
    Raise(ExprLoc<'c>),                     // raise expr
    Assign { target, value },               // x = value
    OpAssign { target, op, value },         // x += value
    For { target, iter, body },             // for target in iter: body
    If { test, body, orelse },              // if test: body else: orelse
}
```

**Key Conversions:**

1. **Ruff AST → Internal AST**: Maps `ruff_python_ast::Stmt` to `Node<'c>`
2. **Position Tracking**: Every node has a `CodeRange` with:
   - Filename
   - Start/end line and column numbers
   - The actual source code lines (for error messages)
3. **Operator Conversion**: Python operators → internal `Operator`/`CmpOperator` enums
4. **Constant Folding**: Python constants → `Object` (Int, Str, Bool, etc.)

**Example:**
```python
x = 1 + 2
```
Becomes:
```rust
Node::Assign {
    target: Identifier { name: "x", id: 0, position: ... },
    value: ExprLoc {
        expr: Expr::Op {
            left: Expr::Constant(Object::Int(1)),
            op: Operator::Add,
            right: Expr::Constant(Object::Int(2)),
        },
        position: CodeRange { ... }
    }
}
```

### Phase 2: Preparation (prepare.rs)

**Input**: `Vec<Node<'c>>` (raw AST)
**Output**: `Executor<'c>` with optimized AST and initial namespace

This phase performs **compile-time optimizations**:

#### 2.1 Name Resolution

Variables are assigned numeric IDs for O(1) runtime lookup:

```rust
let mut name_tracker = NameTracker::new();
// First occurrence of 'x' gets ID 0
// First occurrence of 'y' gets ID 1
// etc.
```

This converts:
- `Identifier { name: "x", id: None }` → `Identifier { name: "x", id: 0 }`
- Runtime uses `namespace[0]` instead of `HashMap.get("x")`

#### 2.2 Constant Folding

Expressions with constant operands are evaluated at compile time:

```python
x = 1 + 2  # Becomes: x = 3
y = "hello" + " world"  # Becomes: y = "hello world"
```

Implementation:
```rust
fn try_eval_expr(expr: &Expr) -> Option<Object> {
    match expr {
        Expr::Op { left, op, right } if can_be_const(left) && can_be_const(right) => {
            let left_obj = try_eval_expr(left)?;
            let right_obj = try_eval_expr(right)?;
            // Evaluate at compile time
            evaluate_op(left_obj, op, right_obj)
        }
        // ...
    }
}
```

**Side Effect Tracking**: Functions marked with `side_effects()` (like `print()`) prevent constant folding:
```python
x = print("hi")  # NOT folded - print has side effects
```

#### 2.3 Dead Code Elimination

```python
pass  # Removed entirely

if True:
    x = 1  # Kept
else:
    x = 2  # Removed (dead code)
```

#### 2.4 Special Optimizations

**ModEq Pattern Recognition**: Common in algorithms like Project Euler:
```python
if (i % 13) == 0:  # Becomes: CmpOperator::ModEq(0)
```

This converts two operations (modulo + comparison) into one specialized operator.

**Exception Constructor Normalization**:
```python
raise TypeError  # Becomes: raise TypeError()
```

#### 2.5 Namespace Pre-allocation

The prepare phase creates the initial namespace:
```rust
let namespace: Vec<Object> = vec![Object::Undefined; num_variables];
// Pre-populated with constants:
namespace[const_id] = Object::Int(42);
```

### Phase 3: Execution (run.rs + evaluate.rs)

**Input**: `Executor<'c>` with optimized AST
**Output**: `Exit<'c>` (return value or exception)

Execution uses a **frame-based model**:

```rust
pub struct RunFrame<'c> {
    namespace: Vec<Object>,      // Variable storage (indexed by ID)
    parent: Option<Box<StackFrame<'c>>>,  // For tracebacks
    name: Cow<'c, str>,          // Frame name (for errors)
}
```

#### 3.1 Statement Execution (run.rs)

The `execute()` function processes statements sequentially:

```rust
pub fn execute(frame: &mut RunFrame<'c>, nodes: &[Node<'c>]) -> RunResult<'c, ()> {
    for node in nodes {
        match execute_node(frame, node)? {
            None => continue,           // Normal execution
            Some(exit) => return exit,  // Early exit (return/raise)
        }
    }
    Ok(())
}
```

**Statement Handlers:**

- **Assign**: `namespace[target_id] = evaluate(value)?`
- **OpAssign**: `namespace[id].py_iadd(evaluate(value)?)` for `x += value`
- **For**: Clone iterator, loop over elements, execute body
- **If**: Evaluate test, execute appropriate branch
- **Return/Raise**: Create `Exit` and bubble up

#### 3.2 Expression Evaluation (evaluate.rs)

The key function signatures:
```rust
pub fn evaluate_use<'c, 'd>(
    namespace: &'d mut [Object],
    heap: &'d mut Heap,
    expr_loc: &'d ExprLoc<'c>,
) -> RunResult<'c, Object>

pub fn evaluate_discard<'c, 'd>(
    namespace: &'d mut [Object],
    heap: &'d mut Heap,
    expr_loc: &'d ExprLoc<'c>,
) -> RunResult<'c, ()>
```

Returns owned `Object`, using `clone_with_heap()` for proper reference counting.

**Expression Types:**

```rust
match &expr_loc.expr {
    Expr::Constant(literal) => Ok(literal.to_object(heap)),
    Expr::Name(ident) => Ok(namespace[ident.id].clone_with_heap(heap)),
    Expr::Op { left, op, right } => {
        let left_obj = evaluate_use(namespace, heap, left)?;
        let right_obj = evaluate_use(namespace, heap, right)?;
        left_obj.py_add(&right_obj, heap)
    }
    // ...
}
```

## Data Structures and Lifetimes

### The `'c` Lifetime

All AST structures carry lifetime `'c`, which represents **the lifetime of the source code string**:

```rust
pub struct Executor<'c> {
    initial_namespace: Vec<Object>,
    nodes: Vec<Node<'c>>,  // Tied to source code
}

pub struct Node<'c> {
    // Contains Identifiers with names borrowed from source
}

pub struct CodeRange<'c> {
    filename: &'c str,       // Borrowed from caller
    code_lines: Vec<&'c str>,  // Borrowed from source
}
```

**Benefits:**
- Zero allocations for variable names and code snippets
- Error messages can show actual source code
- Tracebacks include original code context

### Object Representation

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    // Immediate values (stored inline, no heap allocation)
    Undefined,
    Ellipsis,
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Range(i64),
    Exc(SimpleException),

    // Heap-allocated values (stored in arena)
    Ref(ObjectId),
}
```

**Key Design Points:**

1. **Hybrid Design**: Small immediate values (Int, Bool, None) stored inline; heap objects (List, Str, etc.) referenced via `Ref(ObjectId)`
2. **Reference Counting**: Heap objects use refcounting via `clone_with_heap()` and `drop_with_heap()`
3. **Type Coercion**: Cross-type operations (e.g., `1 < 2.5`) use `PartialOrd`
4. **Exceptions as Values**: Exceptions are first-class objects via `SimpleException`

### Namespace Design

Variables are stored in a `Vec<Object>`:

```rust
// Example namespace state:
let namespace = vec![
    Object::Int(42),        // ID 0: x = 42
    Object::Str("hi"),      // ID 1: y = "hi"
    Object::Undefined,      // ID 2: z (declared but not initialized)
];
```

**Lookup Performance**: `O(1)` array indexing vs. `O(log n)` HashMap

**Undefined Handling**: Accessing `Object::Undefined` raises `NameError`

## Module Reference

### src/bin.rs (Entry Point)
- Reads Python file from command line
- Creates `Executor` with source code
- Calls `run()` and prints result/error
- Times execution

### src/lib.rs (Public API)
```rust
pub struct Executor<'c> {
    initial_namespace: Vec<Const>,  // Literals, not runtime objects
    nodes: Vec<Node<'c>>,
    heap: Heap,  // Arena for heap-allocated objects
}

impl<'c> Executor<'c> {
    pub fn new(code: &'c str, filename: &'c str, input_names: &[&str]) -> ParseResult<'c, Self>
    pub fn run(&mut self, inputs: Vec<Object>) -> Result<Exit<'c, '_>, InternalRunError>
}
```

### src/parse.rs (518 lines)
**Responsibility**: Convert Ruff AST to the internal representation

**Key Functions:**
- `parse_program()`: Entry point, returns `Vec<Node<'c>>`
- `parse_statement()`: Converts each Python statement
- `parse_expr()`: Converts expressions recursively
- `parse_operator()`, `parse_cmp_operator()`: Map operators
- `parse_constant()`: Convert Python constants to `Object`

**CodeRange Construction**: Stores filename, line numbers, and actual source lines

### src/prepare.rs (302 lines)
**Responsibility**: Optimize AST and allocate namespace

**Key Structures:**
```rust
struct NameTracker {
    names: AHashMap<String, usize>,  // name → ID mapping
    consts: Vec<bool>,               // Which IDs are constant
}
```

**Key Functions:**
- `prepare()`: Entry point, returns `Executor<'c>`
- `prepare_node()`: Process each statement
- `prepare_expr()`: Process expressions, do constant folding
- `try_eval_expr()`: Attempt compile-time evaluation
- `can_be_const()`: Check if expression is constant
- Special handlers: `handle_const_if()`, `optimize_mod_eq()`

**Constant Tracking**: Stops tracking after loops (values may change unpredictably)

### src/run.rs (163 lines)
**Responsibility**: Execute statements and manage control flow

**Key Functions:**
- `run_root_frame()`: Creates root execution frame
- `execute()`: Execute statement sequence
- `execute_node()`: Dispatch to statement handlers
- Individual handlers: `handle_assign()`, `handle_for()`, `handle_if()`

**Stack Frame Management**: Maintains parent chain for tracebacks

### src/evaluate.rs
**Responsibility**: Evaluate expressions

**Key Functions:**
- `evaluate_use()`: Main evaluator, returns `Object`
- `evaluate_discard()`: Evaluates expression without returning value
- `evaluate_bool()`: Optimized boolean evaluation
- `eval_op()`: Binary operators (+, -, %)
- `cmp_op()`: Comparison operators (==, <, >, is, etc.)
- `call_function()`: Builtin function calls
- `attr_call()`: Method calls (e.g., `list.append()`)

### src/object.rs
**Responsibility**: Object type and operations

**Key Methods:**
- `clone_with_heap()`: Clones with proper refcounting
- `drop_with_heap()`: Drops with proper refcounting
- `id()`: Returns stable object ID, boxes immediates to heap if needed
- `is()`: Python `is` operator using object identity
- `call_attr()`: Dispatches method calls to heap objects
- `as_int()`: Extract integer value

Implements `PyValue` trait for Python-compatible operations.

### src/heap.rs
**Responsibility**: Arena allocator for heap objects

**Key Types:**
```rust
pub type ObjectId = usize;

pub enum HeapData {
    Object(Box<Object>),  // Boxed immediates for id()
    Str(Str),
    Bytes(Bytes),
    List(List),
    Tuple(Tuple),
}

pub struct Heap {
    objects: Vec<Option<HeapObject>>,
}
```

**Key Methods:**
- `allocate()`: Create new heap object
- `inc_ref()`, `dec_ref()`: Reference counting
- `get()`, `get_mut()`: Access heap data
- `with_entry_mut()`, `with_two()`: Borrow-safe mutation helpers
- `clear()`: Reset between runs

### src/values/ (module)
**Responsibility**: Heap-allocated Python types

Contains:
- `List`: Vec<Object> with append/insert
- `Str`: String wrapper
- `Bytes`: Vec<u8> wrapper
- `Tuple`: Immutable Vec<Object>
- `PyValue` trait: Common interface for all heap types

### src/builtins.rs (112 lines)
**Responsibility**: Builtin functions and exception-like constructors

**Key Structures:**
```rust
pub enum Builtins {
    Print,
    Len,
    Str,
    Repr,
    Id,
    Range,
}
```

**Functions:**
- `from_name()`: Maps a Python identifier to a builtin variant
- `call()`: Executes builtin logic with argument validation
- `name()`: Returns canonical Python spelling for display/logging

Exception types (`ExcType`) continue to live in `exceptions.rs`; the prepare phase now
directly tags calls as either `Function::Builtin(Builtins)` or `Function::Exception(ExcType)` so
runtime dispatch no longer needs an intermediate `Types` enum.

### src/expressions.rs
**Responsibility**: Internal AST types

**Key Types:**
- `Const`: Compile-time literals (Int, Str, Bytes, Bool, None, etc.)
- `Node<'c>`: Statement-level AST
- `Expr<'c>`: Expression-level AST
- `ExprLoc<'c>`: Expression + position
- `Identifier<'c>`: Variable reference with resolved ID
- `Callable<'c>`: Builtin, Exception, or Ident

### src/exit.rs
**Responsibility**: Execution result types

**Key Types:**
- `Exit<'c, 'h>`: Return(Value) or Raise(ExceptionRaise)
- `Value<'h>`: Wrapper with py_str(), py_repr(), py_type() methods

### src/operators.rs (78 lines)
**Responsibility**: Operator enums

**Key Types:**
- `Operator`: Binary operators (Add, Sub, Mult, etc.) and boolean (And, Or)
- `CmpOperator`: Comparisons (Eq, NotEq, Lt, Gt) plus special `ModEq(i64)`

Both implement `Display` for error messages.

### src/exceptions.rs
**Responsibility**: Error handling

**Exception Types:**
```rust
pub enum ExcType {
    ValueError, TypeError, NameError, AttributeError,
}

pub struct SimpleException {
    exc_type: ExcType,
    arg: Option<Cow<'static, str>>,  // Single optional string argument
}

pub struct ExceptionRaise<'c> {
    exc: SimpleException,
    frame: Option<StackFrame<'c>>,  // Where it was raised
}
```

**Internal Errors:**
```rust
pub enum InternalRunError {
    Error(Cow<'static, str>),
    TodoError(Cow<'static, str>),
    Undefined(Cow<'static, str>),
}

pub enum RunError<'c> {
    Internal(InternalRunError),
    Exc(ExceptionRaise<'c>),
}
```

**Error Macros**: `exc_static!`, `exc_fmt!`, `exc_err_static!`, `exc_err_fmt!`, `internal_error!`, `internal_err!`

**Traceback Generation**: `StackFrame` chains together with parent links

### src/parse_error.rs (60 lines)
**Responsibility**: Parse-phase error types

Bridges between parsing and runtime errors during the prepare phase.

## Example Walkthrough

Let's trace this Python code through the entire pipeline:

```python
x = 1 + 2
for i in range(3):
    if i % 2 == 0:
        x += i
print(x)
```

### Parse Phase

**Input**: Raw string
**Output**: 5 `Node` objects

```rust
vec![
    Node::Assign {
        target: Identifier { name: "x", id: None, ... },
        value: ExprLoc {
            expr: Expr::Op {
                left: Expr::Constant(Object::Int(1)),
                op: Operator::Add,
                right: Expr::Constant(Object::Int(2))
            }
        }
    },
    Node::For {
        target: Identifier { name: "i", id: None, ... },
        iter: ExprLoc {
            expr: Expr::Call {
                func: Function::Builtin(Builtins::Range),
                args: [Expr::Constant(Object::Int(3))]
            }
        },
        body: vec![
            Node::If {
                test: Expr::CmpOp {
                    left: Expr::Name(Identifier { name: "i", ... }),
                    op: CmpOperator::Eq,
                    right: Expr::Constant(Object::Int(0))
                },
                // Note: Parser will detect (i % 2) == 0 pattern in prepare
            }
        ]
    },
    Node::Expr(Expr::Call {
        func: Function::Builtin(Builtins::Print),
        args: [Expr::Name(Identifier { name: "x", ... })]
    })
]
```

### Prepare Phase

**Name Resolution:**
- "x" → ID 0
- "i" → ID 1

**Constant Folding:**
- `1 + 2` → `Object::Int(3)`
- `range(3)` → NOT folded (needs runtime)

**ModEq Optimization:**
- `(i % 2) == 0` → `CmpOperator::ModEq(0)` (single operation)

**Namespace Allocation:**
```rust
vec![
    Object::Int(3),      // ID 0: x (constant-folded)
    Object::Undefined,   // ID 1: i (for loop variable)
]
```

**Optimized AST:**
```rust
vec![
    // x = 1 + 2 is now constant assignment
    Node::Assign {
        target: Identifier { id: 0, ... },
        value: Expr::Constant(Object::Int(3))  // Pre-computed!
    },
    Node::For {
        target: Identifier { id: 1, ... },
        iter: Expr::Call { ... },
        body: vec![
            Node::If {
                test: Expr::CmpOp {
                    left: Expr::Name(Identifier { id: 1, ... }),
                    op: CmpOperator::ModEq(0),  // Optimized!
                    right: Expr::Op { ... }  // This is now implicit
                },
                body: vec![
                    Node::OpAssign {
                        target: Identifier { id: 0, ... },
                        op: Operator::Add,
                        value: Expr::Name(Identifier { id: 1, ... })
                    }
                ]
            }
        ]
    },
    Node::Expr(Expr::Call { ... })
]
```

### Execute Phase

**Initial State:**
```rust
namespace = [Object::Int(3), Object::Undefined]
```

**Iteration 1: i = 0**
- `namespace[1] = Object::Int(0)`
- Test: `ModEq(0)` checks if `0 % 2 == 0` → true
- Execute: `namespace[0] += namespace[1]` → `3 += 0` → `3`

**Iteration 2: i = 1**
- `namespace[1] = Object::Int(1)`
- Test: `ModEq(0)` checks if `1 % 2 == 0` → false
- Skip body

**Iteration 3: i = 2**
- `namespace[1] = Object::Int(2)`
- Test: `ModEq(0)` checks if `2 % 2 == 0` → true
- Execute: `namespace[0] += namespace[1]` → `3 += 2` → `5`

**Print:**
- `print(namespace[0])` → prints `5`

**Final Result:**
```rust
Exit::Return(Object::Int(5))
```

## Advanced Topics

### Heap Borrow Patterns

The heap design uses several patterns to avoid borrow checker conflicts:

**Pattern 1: Clone With Heap**
```rust
// Objects must be cloned with heap for proper refcounting
let cloned = object.clone_with_heap(heap);  // Increments refcount
object.drop_with_heap(heap);  // Decrements refcount when done
```

**Pattern 2: Temporary Extraction (Attribute Calls)**
```rust
// Heap temporarily removes entry to allow reentrant access
impl Heap {
    pub fn call_attr(&mut self, id: ObjectId, attr: &Attr, args: Vec<Object>) {
        let mut entry = self.take_entry(id);  // Remove from heap
        let result = entry.data.py_call_attr(self, attr, args);  // Safe to use heap
        self.restore_entry(id, entry);  // Put back
        result
    }
}
```

**Pattern 3: Two-Entry Access**
```rust
// Access two heap entries simultaneously for binary ops
heap.with_two(left_id, right_id, |heap, left_data, right_data| {
    left_data.py_add(right_data, heap)
})
```

### Performance Optimizations

**1. ID-Based Namespace**
- Variables use `Vec` indexing: `O(1)` vs. HashMap: `O(log n)`
- Hot path (expression evaluation) benefits most

**2. Hybrid Object Model**
- Immediate values (Int, Bool, None) stored inline - no heap allocation
- Heap objects (List, Str) share references via refcounting
- Assignment is O(1) inc_ref instead of O(n) clone

**3. Constant Folding**
- Moves computation from runtime to compile time
- Example: `range(1000)` in loop condition is constant-folded once

**4. ModEq Specialization**
- Common pattern in numeric code: `(x % y) == z`
- Single operation vs. two (modulo + comparison)
- ~15-20% speedup in modulo-heavy code

**5. Dead Code Elimination**
- Removes `pass` statements entirely
- Evaluates constant `if` branches: `if True:` → keep only body
- Smaller AST = faster iteration

**6. mimalloc Allocator**
- Drop-in replacement for system allocator
- Faster allocation/deallocation for Object cloning
- Configured via `#[global_allocator]`

**7. LTO and Single Codegen Unit**
- Aggressive inlining across modules
- Release builds are heavily optimized
- Trade compile time for runtime performance

### Error Handling Philosophy

**Two-Phase Error Model:**

1. **Parse/Prepare Phase**: `ParseResult<T> = Result<T, ParseError>`
   - Syntax errors
   - Unsupported features (TodoError)
   - Compile-time type errors (e.g., `1 + "str"`)

2. **Runtime Phase**: `RunResult<T> = Result<T, RunError>`
   - Python exceptions (ValueError, TypeError, etc.)
   - Internal errors (should never happen in correct code)
   - Uninitialized variables

**Error Context Preservation:**
- Every node has `CodeRange` with actual source lines
- Stack frames chain with parent links
- Errors show:
  - File and line number
  - Actual source code snippet
  - Full traceback (for runtime errors)

**Example Error Output:**
```
Traceback (most recent call last):
  File "test.py", line 4, in <module>
    print(x)
NameError: name 'x' is not defined
```

### Testing Strategy

**File-Based Test Fixtures:**

Tests live in `test_cases/` as Python files with expectations in comments:

```python
# test_cases/execute_ok__add_ints.py
1 + 1
# Return=2
```

**Expectation Formats:**
- `# Return=value` - Check py_repr() output
- `# Return.str=value` - Check py_str() output
- `# Return.type=typename` - Check py_type() output
- `# Raise=Exception('message')` - Expect exception
- `# ParseError=message` - Expect parse error

**Benefits:**
- Data-driven tests via datatest-stable harness
- Easy to add cases: just create a `.py` file
- Test names derived from filenames

**PyO3 Integration:**
- `dev-dependencies` includes PyO3
- Can compare Monty output vs. CPython for correctness testing

## Recent Feature Development

The following features were recently added to the main branch, representing significant improvements over the initial implementation:

### 1. Exception Handling with Arguments

**Commits**: `ec42609` (raising errors working), `1814ba2` (exception args working)

**What Changed:**
- Exceptions now support multiple arguments: `raise ValueError('msg', 123)`
- Arguments stored as `Vec<Object>` in `Exception` struct
- Single arg: `ValueError('x')` → displays as `"ValueError('x')"`
- Multiple args: `ValueError('x', 1)` → displays as `"ValueError('x', 1)"`
- Bare exception names converted to constructor calls: `raise TypeError` → `raise TypeError()`

**Files Modified:**
- `src/exceptions.rs`: Added `args: Vec<Object>` field, updated `repr()` formatting
- `src/parse.rs`: Added conversion for bare exception names
- `src/prepare.rs`: Added `convert_to_exc_constructor()` optimization
- `tests/main.rs`: Added tests for exception argument handling

**Example:**
```python
# All of these now work:
raise ValueError()                    # No args
raise ValueError('message')           # Single arg
raise ValueError('error', 42, True)   # Multiple args
raise TypeError                       # Bare name (becomes TypeError())
```

### 2. Attribute Method Calls

**Commit**: `76080f7` (attribute functions WIP)

**What Changed:**
- Added support for method calls on objects: `list.append()`, `list.insert()`
- New `Attr` enum in expressions.rs for method names
- New `Expr::AttrCall` variant for method invocations
- Object methods implemented in `object.rs::attr_call()`

**Implementation Details:**
```rust
// In expressions.rs:
pub enum Attr {
    Append,
    Insert,
}

// In evaluate.rs:
fn attr_call(namespace, object_ident, attr, args) -> Result<Object> {
    let object = namespace.get_mut(object_ident.id)?;
    object.attr_call(attr, args)  // Dispatch to method
}

// In object.rs:
impl Object {
    pub fn attr_call(&mut self, attr: &Attr, args: Vec<Cow<Object>>) -> Result<Object> {
        match (self, attr) {
            (Object::List(list), Attr::Append) => {
                list.push(args[0].into_owned());
                Ok(Object::None)
            }
            (Object::List(list), Attr::Insert) => {
                let idx = args[0].as_int()?;
                list.insert(idx, args[1].into_owned());
                Ok(Object::None)
            }
            _ => Err(AttributeError)
        }
    }
}
```

**Files Modified:**
- `src/expressions.rs`: Added `Attr` enum and `Expr::AttrCall`
- `src/parse.rs`: Parse attribute access syntax
- `src/evaluate.rs`: New `attr_call()` function with borrow-safe argument evaluation
- `src/object.rs`: Implemented `attr_call()` dispatcher and list methods

**Example:**
```python
v = []
v.append('x')      # Works!
v.insert(0, 'y')   # Works!
print(len(v))      # Prints: 2
```

### 3. Module Reorganization: Operators and Builtins Split

**Commit**: `a424293` (split builtin and operators out of types)

**What Changed:**
- Created new `operators.rs` module for operator definitions
- Renamed `types.rs` → `expressions.rs` (clearer name)
- Moved `Operator` and `CmpOperator` enums to dedicated file
- Consolidated builtin function/type definitions into dedicated module (`builtins.rs`)
- Cleaner module boundaries and responsibilities

**File Structure:**
```
Before:
- src/types.rs (mixed: AST types + operators + builtins)
- src/builtins.rs (some builtin logic)

After:
- src/expressions.rs (pure AST types: Node, Expr, etc.)
- src/operators.rs (pure operator enums)
- src/builtins.rs (builtin functions and exception constructors)
```

**Benefits:**
- Better separation of concerns
- Easier to extend each component independently
- Clearer module dependencies

### 4. ModEq Optimization

**Commit**: `629cb9d` (custom ModEq comparison operator)

**What Changed:**
- Recognized common pattern: `(x % y) == z`
- Added specialized `CmpOperator::ModEq(i64)` variant
- Combines modulo and equality check into single operation
- Optimization applied during prepare phase

**Performance Impact:**
- Reduces two operations to one
- Eliminates intermediate `Object` allocation
- ~15-20% speedup in loops with modulo conditions

**Example:**
```python
# Original code:
for i in range(1000):
    if i % 13 == 0:  # Two operations: %, ==
        # ...

# After optimization (internal representation):
for i in range(1000):
    if i ModEq(13, 0):  # One operation
        # ...
```

**Implementation:**
```rust
// In prepare.rs:
fn optimize_mod_eq(expr: &Expr) -> Option<Expr> {
    match expr {
        Expr::CmpOp {
            left: Expr::Op {
                left: x,
                op: Operator::Mod,
                right: y
            },
            op: CmpOperator::Eq,
            right: Expr::Constant(Object::Int(z))
        } => {
            // Transform to ModEq
            Some(Expr::CmpOp {
                left: x.clone(),
                op: CmpOperator::ModEq(*z),
                right: y.clone()
            })
        }
        _ => None
    }
}
```

### 5. Borrow Checker Fixes

**Commits**: `304d0bc` (work for ages ago) and subsequent fixes

**What Changed:**
- Fixed all borrow checker errors in `evaluate.rs`
- Implemented proper patterns for multiple `evaluate()` calls
- Added argument-first evaluation in `attr_call()`
- Improved `Cow<Object>` usage throughout

**Specific Fixes:**

1. **Binary Operators**: Convert left operand to owned before evaluating right
2. **Comparison Operators**: Wrap in explicit `Cow<Object>` with type annotation
3. **Function Calls**: Convert arguments to owned within closure
4. **Attribute Calls**: Evaluate arguments before getting mutable object reference

**Why This Matters:**
- Enables compilation without unsafe code
- Maintains Rust's memory safety guarantees
- Provides clear patterns for future expression types

**Files Modified:**
- `src/evaluate.rs`: All evaluation functions updated with correct borrowing patterns

### 6. mimalloc Integration

**What Changed:**
- Added `mimalloc` as global allocator
- Faster memory allocation for heap objects
- No code changes required (drop-in replacement)

**Note:** mimalloc has since been removed from the dependencies.

### Current Implementation Status

**Implemented Features:**
- ✅ Hybrid heap design with reference counting
- ✅ Python `is` operator (object identity)
- ✅ Reference semantics for heap types (list mutation visible through aliases)
- ✅ List methods: `append()`, `insert()`
- ✅ ModEq optimization for `(x % y) == z` patterns
- ✅ Strings, bytes, tuples as heap types
- ✅ `id()` builtin with stable identity

**Current Limitations:**
- ⚠️ User-defined functions still return `TodoError`
- ⚠️ No dictionary support
- ⚠️ Many operators still unimplemented (*, /, **, etc.)
- ⚠️ No `in`/`not in` operators
- ⚠️ No try/except
- ⚠️ No classes

---

These recent additions represent significant progress toward a more complete Python interpreter, with the foundation now in place for rapid feature addition.

## Critical Analysis: Design Limitations and Trade-offs

This section identifies fundamental issues with the current design that may limit performance, prevent certain features from being implemented, or make it difficult to match CPython behavior.

### Performance Issues

#### 1. ~~Clone-Based Object Model~~ (RESOLVED)

The hybrid heap design now provides reference semantics for heap types:
```rust
pub enum Object {
    Int(i64),           // Immediate - copied
    Ref(ObjectId),      // Heap reference - refcounted
}
```

List assignment is now O(1) reference increment, not O(n) clone.

#### 2. No String Interning

**Problem**: Every string is a separate allocation:
```rust
Object::Str(String)  // Each identifier/string is unique
```

**Impact**:
- Variable names duplicated across all references
- String constants duplicated
- Comparison requires full string comparison

**CPython approach**: Interns common strings, `is` works for interned strings

#### 3. Large Enum Size

**Problem**: `Object` enum size determined by largest variant:
```rust
size_of::<Object>() == size_of::<Vec<Object>>() + discriminant
```

Even `Object::Int(i64)` takes the full enum size in memory.

**Impact**:
- Wastes memory for small types (Bool, None, Int)
- More cache misses (data spread across more cache lines)
- More data movement on copy

**Better approach**: Box large variants or use pointer-based representation

#### 4. Namespace Lookup Limitations

**Current design**: Flat `Vec<Object>` indexed by ID
```rust
namespace: Vec<Object>  // All variables in one vector
```

**Problems**:
- **Nested scopes**: No support for closures that capture outer variables
- **Global scope**: Can't implement `global` keyword properly
- **Module system**: Each module needs isolated namespace
- **Class namespaces**: No way to represent `self.__dict__`

**Example that won't work**:
```python
def outer():
    x = 1
    def inner():
        return x + 1  # Can't access outer's x!
    return inner
```

**Why**: `inner()` needs access to `outer()`'s namespace, but current design only has one flat namespace per frame.

#### 5. No Lazy Evaluation

**Problem**: `range()` returns a full list:
```python
for i in range(1000000):  # Creates list of 1M integers
    break  # Only needed first element!
```

**CPython**: `range()` returns an iterator that yields values on demand

**Why it matters**: Idiomatic Python code like `range(10**9)` is impossible

### Fundamental Design Dead-Ends

#### 1. ~~Value Semantics vs Reference Semantics~~ (RESOLVED)

The hybrid heap design now provides correct Python reference semantics:

```python
a = [1, 2, 3]
b = a        # b references same list (inc_ref)
b.append(4)
print(a)     # [1, 2, 3, 4] - correctly modified!
```

Heap types (List, Str, Tuple, Bytes) now share references correctly.

#### 2. ~~No Object Identity~~ (RESOLVED)

The `is` operator is now implemented via `Object::id()`:
```python
a = [1, 2, 3]
b = a
print(a is b)  # True - same ObjectId

c = [1, 2, 3]
print(a is c)  # False - different ObjectIds
```

Immediates (None, True, False) have constant IDs; heap objects return their ObjectId.

#### 3. No Cycle Detection

**Problem**: Reference counting without cycle detection leaks circular references:
```python
a = []
a.append(a)  # Creates cycle - refcount never hits 0
```

**Impact**: Memory leaks with circular data structures. Needs mark-sweep GC to collect cycles.

#### 4. Lifetime 'c Prevents Dynamic Code

**Problem**: All AST nodes tied to source string lifetime:
```rust
pub struct Node<'c> { ... }
```

**Impact**:
- Can't implement `eval()` or `exec()` (dynamic code execution)
- Can't implement REPL properly (each line is isolated)
- Can't implement code generation or metaprogramming
- Can't load modules dynamically

**Example that can't work**:
```python
code = input("Enter Python code: ")
eval(code)  # Would need to create Node<'new_lifetime>
```

**Fix required**: Owned AST nodes (using `String` instead of `&'c str`)

#### 5. Single Namespace Per Frame

**Problem**: Can't implement Python's scope rules:

**Python has 4 scopes** (LEGB):
- Local
- Enclosing (closures)
- Global
- Builtin

**Monty has 1 scope**: Flat vector of locals

**Examples that fail**:
```python
# 1. Closures
def outer():
    x = 1
    def inner():
        return x  # Can't access enclosing scope
    return inner

# 2. Global keyword
x = 10
def modify_global():
    global x
    x = 20  # Should modify global, not create local

# 3. Nonlocal keyword
def outer():
    x = 1
    def inner():
        nonlocal x
        x = 2  # Should modify outer's x
    inner()
    print(x)  # Should be 2
```

**Fix required**: Stack of namespace frames with scope resolution

### CPython Compatibility Issues

#### 1. Mutable Default Arguments

**CPython (famous gotcha)**:
```python
def append_to(element, target=[]):
    target.append(element)
    return target

print(append_to(1))  # [1]
print(append_to(2))  # [1, 2] - same list!
```

**Monty**: Would create new list each time (actually more intuitive, but wrong)

**Why**: Default arguments evaluated once and shared. Requires persistent object identity.

#### 2. Iterator Protocol

**Problem**: No `yield` statement, no iterator protocol:
```python
def fibonacci():
    a, b = 0, 1
    while True:
        yield a  # Can't implement
        a, b = b, a + b
```

**Monty limitation**: Would need to return entire sequence or implement coroutines

**Impact**:
- No generators
- No lazy evaluation
- No `for` loop over custom iterators

#### 3. Exception Handling Recovery

**CPython**:
```python
try:
    risky_operation()
except ValueError:
    print("Handled")
# Continue execution
```

**Monty**: Has `raise` but no `try/except`, so all exceptions are fatal

**Impact**: Can't write robust error handling code

#### 4. Class Definitions and Inheritance

**Problem**: No object model for classes:
- No `class` statement
- No `self` parameter special handling
- No method resolution order (MRO)
- No inheritance
- No `__init__`, `__new__`
- No metaclasses

**Example that can't work**:
```python
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def distance(self):
        return (self.x ** 2 + self.y ** 2) ** 0.5

p = Point(3, 4)
print(p.distance())  # None of this works
```

**Fix required**: Complete object system with attribute dictionaries, method binding, inheritance

#### 5. Dynamic Attributes

**CPython**:
```python
class Obj: pass
o = Obj()
o.new_attr = 42  # Create attribute dynamically
print(o.new_attr)
```

**Monty**: No `__dict__`, can't add attributes to objects

**Also can't implement**:
- `getattr()`, `setattr()`, `delattr()`
- `__getattribute__`, `__setattr__`
- Descriptors, properties
- Dynamic class modification

#### 6. Import System

**Problem**: No module system:
```python
import math
from collections import defaultdict
```

**Required for imports**:
- Module namespace isolation
- Module caching
- Circular import handling
- Relative imports
- `__name__`, `__file__` magic variables

**Current design**: Single flat namespace can't support this

#### 7. Built-in Type Customization

**CPython allows**:
```python
class MyList(list):
    def __init__(self):
        super().__init__()
        self.created_at = time.time()
```

**Monty**: Built-in types are enum variants, can't be subclassed

#### 8. Truthiness Protocol

**CPython**:
```python
class AlwaysFalse:
    def __bool__(self):
        return False

if AlwaysFalse():  # Calls __bool__
    print("Never prints")
```

**Monty**: Hardcoded `bool()` method, can't customize

### Memory Safety Issues

#### 1. Stack Overflow on Deep Recursion

**Problem**: No tail call optimization, unbounded recursion:
```python
def recurse(n):
    if n == 0:
        return 0
    return 1 + recurse(n - 1)

recurse(100000)  # Stack overflow!
```

**CPython**: Has max recursion depth (usually 1000), controlled

**Monty**: Rust stack overflow (panic)

#### 2. Infinite Loop in Constant Folding

**Problem**: `try_eval_expr()` could infinite loop on malicious code:
```python
# Carefully crafted expression that causes
# constant folder to recurse indefinitely
```

**Mitigation**: Need depth limit in prepare phase

### Type System Limitations

#### 1. No Type Annotations Support

**CPython 3.5+**:
```python
def greet(name: str) -> str:
    return f"Hello, {name}"
```

**Monty**: Parser might handle syntax but can't use types

**Impact**: Can't implement type checking, mypy-style analysis

#### 2. No Union Types or Optional

**CPython**:
```python
def find(lst: list[int], target: int) -> int | None:
    # ...
```

**Monty**: Single `Object` enum, no fine-grained types

#### 3. No Generic Types

**CPython**:
```python
from typing import List, Dict, TypeVar

T = TypeVar('T')
def first(items: List[T]) -> T:
    return items[0]
```

**Monty**: Lists are `Vec<Object>`, no generic type parameters

### Optimization Barriers

#### 1. Can't Inline Through Dynamic Dispatch

**Problem**: All operations go through `Object` enum match:
```rust
match (self, other) {
    (Object::Int(a), Object::Int(b)) => ...
    (Object::Float(a), Object::Float(b)) => ...
    // Many more branches
}
```

**Impact**: Branch prediction misses, can't optimize hot paths

**CPython approach**: Specialized bytecode instructions for common type combinations

#### 2. No JIT Compilation

**Problem**: Interprets AST directly, no intermediate bytecode

**CPython**: Compiles to bytecode, PyPy adds JIT

**Monty**: No way to add JIT without redesign

#### 3. No Specializing Optimizer

**CPython 3.11+**: Adaptive interpreter specializes hot code paths

**Example**:
```python
# First execution: generic integer add
# After warmup: specialized "integer add" instruction
x = a + b
```

**Monty**: Same code path every time

### Concurrency Limitations

#### 1. No Threading Support

**Problem**: Single-threaded only, no `threading` module possible

**Why**:
- Namespace is `&mut`, not thread-safe
- Objects not `Send` or `Sync`
- No GIL concept

#### 2. No Async/Await

**Problem**: No coroutine support:
```python
async def fetch_data():
    await asyncio.sleep(1)
    return "data"
```

**Required**: Completely different execution model (state machines, event loop)

### Testing and Debugging Limitations

#### 1. No `__repr__` Customization

**CPython**:
```python
class Point:
    def __repr__(self):
        return f"Point({self.x}, {self.y})"
```

**Monty**: Hardcoded `Display` impl, can't customize

#### 2. No Introspection

**Can't implement**:
- `dir()` - list attributes
- `type()` - get object type
- `isinstance()` - check type
- `vars()` - get `__dict__`
- `inspect` module

#### 3. No Debugger Hooks

**Problem**: No way to implement:
- Breakpoints
- Step-through debugging
- Variable inspection
- Stack frame manipulation

### Pragmatic Assessment

#### What Works Well
1. ✅ Simple numeric computation
2. ✅ String/list manipulation with proper reference semantics
3. ✅ Basic control flow (if, for)
4. ✅ Safe sandboxing (no FFI, no file I/O)
5. ✅ Object identity (`is` operator)
6. ✅ Fast compilation (prepare phase)

#### What's Not Yet Implemented
1. ❌ User-defined functions
2. ❌ Closures and nested scopes
3. ❌ Classes
4. ❌ Exception handling (no try/except)
5. ❌ Iterators and generators
6. ❌ Module system and imports
7. ❌ Dynamic code execution

#### Viable Use Cases
- **Code challenges**: LeetCode-style problems (if they don't need classes)
- **Math competitions**: Project Euler problems
- **Educational**: Teaching basic Python syntax
- **Sandboxed computation**: Safe evaluation of untrusted numeric expressions

#### Non-Viable Use Cases
- **General Python code**: Most real Python programs use features Monty can't support
- **Data science**: NumPy/Pandas rely heavily on reference semantics
- **Web frameworks**: Django/Flask need classes, imports, exceptions
- **Scripts**: Most scripts need file I/O, imports, exception handling

### Path Forward

**Completed:**
1. ✅ Heap-allocated objects with IDs
2. ✅ Reference counting

**Still Needed:**
1. **Scope Chain for Namespaces** - for closures, global/nonlocal
2. **Cycle Detection** - mark-sweep GC for circular references
3. **User-defined Functions** - function objects, call frames
4. **Classes** - object model, MRO, descriptors
5. **Owned AST** - for eval()/exec()

### Conclusion

Monty is a **sandboxed Python interpreter** that now implements proper Python reference semantics via a hybrid heap design with reference counting. The recent architectural improvements have resolved the fundamental value/reference semantics mismatch.

The interpreter supports a **growing subset** of Python including lists, tuples, strings, bytes with correct aliasing behavior, and object identity. For broader compatibility, additional work is needed on user-defined functions, classes, and scope chains.
