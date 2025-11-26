# Arena Hybrid Design

This document outlines the architecture for Monty to achieve Python-compatible reference semantics while maintaining performance and safety.

## Implementation Status

**COMPLETED:**
- ✅ Phase 0: Literal (`Const`) enum for compile-time constants
- ✅ Phase 1: Core heap infrastructure (`src/heap.rs`)
- ✅ Phase 2: Evaluation threading heap through all functions
- ✅ Phase 3: Assignment & cloning semantics with `clone_with_heap`/`drop_with_heap`
- ✅ Phase 4: Object identity & `is` operator
- ✅ Phase 5: List methods with reference semantics

**NOT YET IMPLEMENTED:**
- ⬜ Phase 6: Exception objects on heap (currently `Object::Exc(SimpleException)`)
- ⬜ Phase 7: Small integer caching (not needed - immediates are inline)
- ⬜ Phase 8: String interning
- ⬜ Dictionary support
- ⬜ Cycle detection / mark-sweep GC

## Executive Summary

**Problem Solved**: The hybrid heap design provides Python-compatible reference semantics.

**Current Architecture**:
- **Immediate values** (Int, Bool, None) stored inline
- **Heap objects** (List, Str, Dict) allocated in arena with unique IDs
- **Reference counting** for memory management
- **Monotonically increasing IDs** (never reused) for simplicity and safety

**Key Benefit**: Enables correct Python behavior for shared mutable state while maintaining performance for common cases.

## Why This Approach?

### Compared to Arc/Rc

| Issue | Arc<Mutex<Object>> | Rc<RefCell<Object>> | Arena + IDs |
|-------|-------------------|---------------------|-------------|
| **Object Identity** | ❌ Can't distinguish equal objects | ❌ Can't distinguish equal objects | ✅ Unique IDs |
| **Python `is` operator** | ❌ Impossible | ❌ Impossible | ✅ Compare IDs |
| **Mutability** | ⚠️ Mutex overhead | ⚠️ Runtime panics | ✅ Direct mutation |
| **Performance** | ⚠️ Atomic operations | 🟢 Good | 🟢 Excellent |
| **Implementation Complexity** | 🟢 Simple | 🟢 Simple | 🟢 Simple (no free list) |
| **Cache locality** | ❌ Scattered allocations | ❌ Scattered allocations | ✅ Contiguous arena |
| **GC-ready** | ⚠️ Hard to add cycle detection | ⚠️ Hard to add cycle detection | ✅ All objects in one place |
| **Debugging** | ⚠️ Hard to inspect | ⚠️ Hard to inspect | ✅ Can dump entire heap |

### Core Insight

Python's `is` operator requires **object identity**, which neither `Arc` nor `Rc` provides:

```python
a = [1, 2, 3]
b = [1, 2, 3]
c = a

# These must work:
assert a is c      # Same object (identity)
assert a == b      # Equal value
assert not (a is b)  # Different objects
```

With `Arc<Object>` or `Rc<Object>`, you cannot distinguish between "same object" and "equal objects" because you have no stable object ID.

### Design Simplification: No Free List

**Key Decision**: IDs are **never reused during a single execution** - always append to vector. After a run finishes we call `Heap::clear()` which resets the arena wholesale, so stale IDs can never leak across runs.

**Alternative Considered**: Free list to recycle IDs (more memory efficient but complex)

**Why Simpler is Better**:

1. **No Use-After-Free Confusion**
   - With reuse: `id=5` might point to different objects at different times
   - Without reuse: `id=5` always refers to same object (or None if freed)
   - Stale references fail clearly instead of silently corrupting data

2. **Easier Debugging**
   - Monotonic IDs (0, 1, 2, 3...) are easier to trace
   - Object lifetime tracking is straightforward
   - No "ID 42 was reused 7 times" confusion

3. **Simpler Implementation**
   - No free list management logic
   - No choosing between "reuse slot" vs "allocate new"
   - `allocate()` is just: push and increment

4. **Natural Safety**
   - No need for generational indices
   - Accessing freed ID returns clear error
   - Thread-safe atomic increment is trivial (future enhancement)

**Trade-offs Accepted**:

- ❌ Vector keeps growing (freed slots still hold an `Option<HeapObject>` shell)
- ❌ Can't reclaim vector capacity without compacting
- ❌ Iteration must skip `None` entries

**For Monty's Use Case**: These trade-offs are acceptable because:
- Executions are short-lived (heap cleared between runs via `Heap::clear()`)
- The extra `Option` wrapper is predictable overhead and compaction is on the roadmap
- Simplicity enables faster development and fewer bugs

## Design Overview

### Object Representation

```rust
/// Primary value type - fits in 16 bytes (2 words)
/// NOTE: We intentionally do not derive `Clone`/`PartialEq` so that every
/// duplication routes through helper methods that can touch the heap and bump
/// reference counts.
#[derive(Debug)]
pub enum Object {
    // Immediate values (stored inline, no heap allocation)
    Int(i64),
    Bool(bool),
    None,

    // Heap-allocated values (stored in arena)
    Ref(ObjectId),
}

/// Index into heap arena
pub type ObjectId = usize;

/// Borrowed handle that ensures refcount operations fire on clone/drop.
/// Without this guard a plain `Object::Ref` clone would just copy the ID and
/// silently skip `inc_ref`, which inevitably leaks or double-frees.
pub struct HeapRef<'a> {
    heap: &'a mut Heap,
}

impl<'a> HeapRef<'a> {
    pub fn clone_object(&mut self, object: &Object) -> Object {
        match object {
            Object::Ref(id) => {
                self.heap.inc_ref(*id);
                Object::Ref(*id)
            }
            other => other.clone_immediate(),
        }
    }

    pub fn drop_object(&mut self, object: &Object) {
        if let Object::Ref(id) = object {
            self.heap.dec_ref(*id);
        }
    }
}

impl Object {
    /// Helper used by `HeapRef` so immediate values can still be duplicated
    /// cheaply while keeping heap-backed refs centralized.
    fn clone_immediate(&self) -> Object {
        match self {
            Object::Int(v) => Object::Int(*v),
            Object::Bool(v) => Object::Bool(*v),
            Object::None => Object::None,
            Object::Ref(_) => unreachable!(
                "Ref clones must go through HeapRef to maintain refcounts"
            ),
        }
    }
}
```

### Heap Structure

```rust
/// Central heap managing all allocated objects
pub struct Heap {
    /// All heap-allocated objects. None = freed slot.
    /// IDs are never reused during a run - always append new objects until `clear()`.
    objects: Vec<Option<HeapObject>>,

    /// Next ID to allocate (monotonically increasing)
    next_id: ObjectId,
}

/// A single heap-allocated object
struct HeapObject {
    /// Reference count for memory management
    refcount: usize,

    /// Optional cached hash for immutable objects (str, tuple, bytes, etc.).
    /// Needed because hashing `Object::Ref` requires heap context.
    cached_hash: Option<u64>,

    /// Actual object data
    data: HeapData,
}

/// Data stored on heap (actual implementation)
#[derive(Debug)]
pub enum HeapData {
    Object(Box<Object>),  // Boxed immediates for id()
    Str(Str),
    Bytes(Bytes),
    List(List),
    Tuple(Tuple),
    // Future: Dict, Set, FrozenSet, Function, Class, Instance, etc.
}
```

**Why cache hashes?**

- The Python dictionary model allows strings, tuples, and other immutable types as keys.
- Adding `FrozenSet` to `HeapData` makes it hashable (like CPython) while `Set` stays mutable/unhashable.
- Rust's `Hash` trait does not accept extra context, so hashing `Object::Ref(id)` cannot look inside the heap unless we pre-compute a hash when the object is created.
- `cached_hash` stores that value for immutable heap entries. Mutable types (lists, dicts) leave it as `None` so any attempt to hash them raises `TypeError`, matching CPython.

### Dictionary Hashing Strategy

1. **Allocation time**: When creating immutable heap data (str, bytes, tuple/frozenset of hashable elements), compute its hash once and store it in `cached_hash`. Reuse CPython's rules: tuples/frozensets are hashable only if every element is hashable.
2. **Hash implementation**: `impl Hash for Object` matches on immediates; for `Object::Ref(id)` it fetches the heap slot and feeds the cached hash to the hasher. If `cached_hash` is `None`, raise `TypeError` from the caller instead of trying to hash a mutable object.
3. **Dictionary storage**: Dictionaries continue to use `HashMap<Object, Object>`, but only `Object`s known to be hashable (checked via `heap.is_hashable(&key)`) reach `HashMap::insert`; otherwise `TypeError` is raised.
4. **Invalidating hashes**: Because only immutable data can have a cached hash, we never need to recompute or invalidate—it stays correct for the lifetime of the heap entry.

`util::hash_frozenset` sorts the element hashes before folding them so the final value is order-independent, matching CPython's behavior.

## Implementation Plan

### Phase 0: Literal Object Layer (New)

**Goal**: Introduce a dedicated `Literal` representation so parse/prepare stages can continue folding/inspecting constants without depending on runtime heap objects.

**Key Tasks**:

1. Create `Literal` (or `ConstObject`) enum mirroring the current `Object` variants needed by parser/prepare.
2. Update `Expr::Constant` and prepare-time evaluation to traffic in `Literal` values only.
3. Provide conversion helpers (`Literal::to_runtime(&mut Heap) -> Object`) so compile-time constants can be materialized on demand once execution begins.
4. Add concise rustdoc/docstrings to all helper methods clarifying whether they operate on literals or runtime objects.

### Phase 0.5: Disable Fragile Parse-Time Optimizations

**Goal**: Ensure only heap-backed runtime objects are evaluated/executed; remove constant-folding passes that rely on cloning full runtime `Object` graphs.

**Key Tasks**:

1. In `prepare.rs`, gate or remove `can_be_const` logic that attempts to partially evaluate expressions using runtime semantics.
2. Replace it with a literal-only pass (safe operations like combining literal ints/strings) or defer entirely until after heap migration is complete.
3. Document (with TODO comments) any temporarily-disabled optimizations so they can be revisited once the literal/runtime split settles.

### Phase 1: Core Heap Infrastructure (Foundation)

**Goal**: Basic heap allocation with reference counting

**Files to Create**:
- `src/heap.rs` - Heap and HeapObject implementation

**Changes Required**:
- `src/object.rs` - Modify Object enum to use hybrid design
- `src/lib.rs` - Add heap to Executor

**Implementation Steps**:

1. **Create `src/heap.rs`**:
```rust
pub struct Heap {
    objects: Vec<Option<HeapObject>>,
    next_id: ObjectId,
}

impl Heap {
    pub fn new() -> Self {
        Heap {
            objects: Vec::new(),
            next_id: 0,
        }
    }

    /// Allocate a new heap object, returns its ID
    /// IDs are never reused - always append
    pub fn allocate(&mut self, data: HeapData) -> ObjectId {
        let id = self.next_id;
        let cached_hash = self.compute_hash(&data);
        self.objects.push(Some(HeapObject {
            refcount: 1,
            cached_hash,
            data,
        }));
        self.next_id += 1;
        id
    }

    fn compute_hash(&self, data: &HeapData) -> Option<u64> {
        match data {
            HeapData::Str(s) => Some(util::hash_str(s)),
            HeapData::Bytes(b) => Some(util::hash_bytes(b)),
            HeapData::Tuple(items) => {
                if items.iter().all(|o| self.is_hashable(o)) {
                    Some(util::hash_tuple(items))
                } else {
                    None
                }
            }
            HeapData::FrozenSet(items) => {
                if items.iter().all(|o| self.is_hashable(o)) {
                    Some(util::hash_frozenset(items))
                } else {
                    None
                }
            }
            _ => None, // Lists/dicts/exceptions are unhashable
        }
    }

    fn is_hashable(&self, object: &Object) -> bool {
        match object {
            Object::Int(_) | Object::Bool(_) | Object::None => true,
            Object::Ref(id) => self
                .objects
                .get(*id)
                .and_then(|slot| slot.as_ref())
                .map(|obj| obj.cached_hash.is_some())
                .unwrap_or(false),
        }
    }

    /// Increment reference count
    pub fn inc_ref(&mut self, id: ObjectId) {
        if let Some(Some(obj)) = self.objects.get_mut(id) {
            obj.refcount += 1;
        }
    }

    /// Decrement reference count, free if zero (iteratively to avoid stack overflow)
    pub fn dec_ref(&mut self, id: ObjectId) {
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let Some(slot) = self.objects.get_mut(current) else { continue };
            let Some(obj) = slot.as_mut() else { continue };

            if obj.refcount > 1 {
                obj.refcount -= 1;
                continue;
            }

            // Take ownership of the data so we can walk children without new allocations
            let taken = slot.take().map(|mut owned| {
                owned.refcount = 0;
                owned.data
            });

            if let Some(data) = taken {
                self.enqueue_children(&data, &mut stack);
            }
        }
    }

    /// Get immutable reference to object data
    pub fn get(&self, id: ObjectId) -> Result<&HeapData, HeapError> {
        self.objects
            .get(id)
            .and_then(|slot| slot.as_ref())
            .map(|obj| &obj.data)
            .ok_or(HeapError::InvalidId)
    }

    /// Get mutable reference to object data
    pub fn get_mut(&mut self, id: ObjectId) -> Result<&mut HeapData, HeapError> {
        self.objects
            .get_mut(id)
            .and_then(|slot| slot.as_mut())
            .map(|obj| &mut obj.data)
            .ok_or(HeapError::InvalidId)
    }

    fn free_object(&mut self, id: ObjectId) {
        self.dec_ref(id);
    }

    fn enqueue_children(&mut self, data: &HeapData, stack: &mut Vec<ObjectId>) {
        match data {
            HeapData::List(items) | HeapData::Tuple(items) => {
                for obj in items {
                    if let Object::Ref(id) = obj {
                        stack.push(*id);
                    }
                }
            }
            HeapData::Dict(map) => {
                for (k, v) in map {
                    if let Object::Ref(id) = k {
                        stack.push(*id);
                    }
                    if let Object::Ref(id) = v {
                        stack.push(*id);
                    }
                }
            }
            _ => {}
        }
    }

    /// Frees all objects and resets arena between executions.
    /// Safe because callers only invoke it once no references escape a run.
    pub fn clear(&mut self) {
        for id in 0..self.objects.len() {
            if self.objects[id].is_some() {
                self.free_object(id);
            }
        }
        self.objects.clear();
        self.next_id = 0;
    }
}
```

This iterative `dec_ref` implementation protects us from stack overflows caused by deeply nested data (it uses an explicit stack) and avoids the temporary `Vec<ObjectId>` allocation by `take`-ing ownership of each heap slot before visiting its children.

2. **Update `src/object.rs`**:
```rust
// Change Object enum
pub enum Object {
    Int(i64),
    Float(f64),
    Bool(bool),
    None,
    Ref(ObjectId),
}

// Update methods to work with heap
impl Object {
    // Operations now take &mut Heap parameter
    pub fn add(&self, other: &Object, heap: &mut Heap) -> Option<Object> {
        match (self, other) {
            (Object::Int(a), Object::Int(b)) => Some(Object::Int(a + b)),
            (Object::Ref(id_a), Object::Ref(id_b)) => {
                // Get data from heap (handle Result)
                let data_a = heap.get(*id_a).ok()?;
                let data_b = heap.get(*id_b).ok()?;

                match (data_a, data_b) {
                    (HeapData::Str(a), HeapData::Str(b)) => {
                        let result = format!("{}{}", a, b);
                        let id = heap.allocate(HeapData::Str(result));
                        Some(Object::Ref(id))
                    }
                    (HeapData::List(a), HeapData::List(b)) => {
                        let mut result = a.clone();
                        result.extend_from_slice(b);
                        // Inc ref for all items
                        for obj in &result {
                            if let Object::Ref(id) = obj {
                                heap.inc_ref(*id);
                            }
                        }
                        let id = heap.allocate(HeapData::List(result));
                        Some(Object::Ref(id))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    // Similar updates for sub, modulus, etc.
}
```

3. **Update `src/lib.rs`**:
```rust
pub struct Executor<'c> {
    initial_namespace: Vec<Object>,
    nodes: Vec<Node<'c>>,
    heap: Heap,  // Shared heap reused per run
}

impl<'c> Executor<'c> {
    pub fn run(&mut self, inputs: Vec<Object>) -> RunResult<'c, Exit<'c>> {
        self.heap.clear(); // Drop any refs from a previous invocation
        let heap = &mut self.heap;
        // Pass `heap` as a &mut reference through execution
        // ...
    }
}
```

**Testing**: Create tests for basic heap operations (allocate, inc_ref, dec_ref, free)

### Phase 2: Update Evaluation & Execution

**Goal**: Thread heap through all evaluation and execution

**Changes Required**:
- `src/evaluate.rs` - Add heap parameter to all functions
- `src/run.rs` - Pass heap through execution
- `src/prepare.rs` - Pre-allocate constants on heap

**Implementation Steps**:

1. **Update function signatures**:
```rust
// evaluate.rs
pub fn evaluate<'c, 'd>(
    namespace: &'d mut [Object],
    heap: &'d mut Heap,  // Borrowed heap shared across all frames
    expr_loc: &'d ExprLoc<'c>,
) -> RunResult<'c, Cow<'d, Object>>

// run.rs
pub struct RunFrame<'c> {
    namespace: Vec<Object>,
    heap: &'c mut Heap,  // Frames borrow the single heap
    parent: Option<Box<StackFrame<'c>>>,
    name: Cow<'c, str>,
}
```

2. **Update all call sites**: This is mechanical but extensive - every function that evaluates expressions needs the heap parameter

3. **Update prepare phase** to allocate constants:
```rust
// In prepare.rs
let mut heap = Heap::new();

// When creating constants:
let s = "hello world";
let id = heap.allocate(HeapData::Str(s.to_string()));
namespace[const_id] = Object::Ref(id);
```

**Testing**: All existing tests should still pass (behavior unchanged, just using heap internally)

### Phase 3: Assignment & Cloning Semantics

**Goal**: Transition heap-managed types (lists first, then str/bytes/dict/set/frozenset/tuples/exceptions) into the arena, introduce helper APIs for safe cloning/dropping, and update the runtime to inc/dec refs on assignment.

**Key Milestones**:

1. **Helper APIs**:
   - Add `Object::clone_with_heap`, `Object::drop_with_heap`, and a `CowObjectExt` trait so evaluation/frames can acquire owned values without leaking references.
   - Teach `Object::bool`, `Object::len`, `Object::py_eq`, etc., to accept a `Heap` parameter since their semantics will depend on the arena contents.

2. **Move Types Incrementally**:
   - Start with `list`: store list storage in `HeapData::List(Rc<RefCell<Vec<Object>>>`), make `Object::Ref(ObjectId)` the only heap-backed variant, and update list literals/attributes to allocate into the heap.
   - Follow up with `tuple`, `str`, `bytes`, `dict`, `set`, `frozenset`, and exception objects in later iterations, keeping each conversion small and testable.

3. **Assignment & Drop Semantics**:
   - Update `RunFrame` to call `clone_with_heap`/`drop_with_heap` when storing or discarding values (including temporaries from expression statements).
   - Add explicit cleanup (before returning from `Executor::run`) to release any remaining references in namespaces.

4. **Testing**:
   - Add regression tests for shared list mutation (`b = a; b.append(2)`), default mutable arguments, and any other aliasing scenarios as each type migrates.

### Phase 4: Object Identity & `is` Operator

**Goal**: Implement Python's `is` operator

**Changes Required**:
- `src/operators.rs` - Add `Is` and `IsNot` to CmpOperator (already there!)
- `src/evaluate.rs` - Implement `is` comparison
- `src/object.rs` - Add identity comparison method

**Implementation Steps**:

1. **Add identity check**:
```rust
impl Object {
    pub fn is_identical(&self, other: &Object) -> bool {
        match (self, other) {
            // Immediate values: compare by value
            (Object::Int(a), Object::Int(b)) => a == b,
            (Object::Bool(a), Object::Bool(b)) => a == b,
            (Object::None, Object::None) => true,

            // Heap values: compare by ID
            (Object::Ref(id_a), Object::Ref(id_b)) => id_a == id_b,

            // Different types or immediate vs ref
            _ => false,
        }
    }
}
```

2. **Implement in evaluator**:
```rust
// In cmp_op()
CmpOperator::Is => Ok(left_object.is_identical(&right_object)),
CmpOperator::IsNot => Ok(!left_object.is_identical(&right_object)),
```

**Testing**:
```python
a = [1, 2, 3]
b = a
c = [1, 2, 3]

assert a is b        # Same object
assert a is not c    # Different objects
assert a == c        # But equal values
```

### Phase 5: List Methods & Mutation

**Goal**: Fix list methods to work with reference semantics

**Changes Required**:
- `src/evaluate.rs` - Update attr_call to pass heap
- `src/object.rs` - Update list methods to use heap

**Implementation Steps**:

1. **Update attr_call signature**:
```rust
fn attr_call<'c, 'd>(
    namespace: &'d mut [Object],
    heap: &'d mut Heap,  // Add heap
    expr_loc: &'d ExprLoc<'c>,
    object_ident: &Identifier<'c>,
    attr: &Attr,
    args: &'d [ExprLoc<'c>],
) -> RunResult<'c, Cow<'d, Object>>
```

2. **Update list.append()**:
```rust
// In Object::attr_call()
match (self, attr) {
    (Object::Ref(id), Attr::Append) => {
        if let HeapData::List(list) = heap.get_mut(*id) {
            let item = args[0].clone();

            // Inc ref if heap object
            if let Object::Ref(item_id) = item {
                heap.inc_ref(item_id);
            }

            list.push(item);
            Ok(Cow::Owned(Object::None))
        } else {
            Err(AttributeError)
        }
    }
    // ...
}
```

**Testing**: Verify mutation works correctly:
```python
a = [1, 2]
b = a
b.append(3)
assert a == [1, 2, 3]  # Both see the change
```

### Phase 6: Exception Objects on Heap

**Goal**: Move exceptions to heap to support exception instances

**Changes Required**:
- Remove `Object::Exc` variant
- Exceptions always stored as `Object::Ref` with `HeapData::Exception`
- Update exception raising/catching

**Implementation Steps**:

1. **Update exception creation**:
```rust
// In exceptions.rs
impl Exception {
    pub fn to_object(self, heap: &mut Heap) -> Object {
        let id = heap.allocate(HeapData::Exception(self));
        Object::Ref(id)
    }
}
```

2. **Update raise handling**:
```rust
// When raising
let exc_obj = Exception::new(args).to_object(heap);
return Err(ExceptionRaise { exc: exc_obj, frame });
```

**Testing**: Ensure exceptions work and can be passed around as values

### Phase 7: Optimization - Small Integer Caching

**Goal**: Cache small integers like CPython (-5 to 256)

**Implementation**:

```rust
impl Heap {
    pub fn new() -> Self {
        let mut heap = Heap {
            objects: Vec::new(),
            free_list: Vec::new(),
            next_id: 0,
            small_ints: [None; 262], // -5 to 256
        };

        // Pre-allocate small integers
        // (Actually, keep as immediate values - no need to cache!)
        heap
    }
}
```

**Note**: With immediate values, small integer caching is automatic!

### Phase 8: String Interning

**Goal**: Intern commonly used strings

```rust
pub struct Heap {
    objects: Vec<HeapObject>,
    free_list: Vec<ObjectId>,
    next_id: ObjectId,

    /// Interned strings map: content -> ObjectId
    interned_strings: HashMap<String, ObjectId>,
}

impl Heap {
    pub fn intern_string(&mut self, s: String) -> ObjectId {
        if let Some(&id) = self.interned_strings.get(&s) {
            self.inc_ref(id);
            id
        } else {
            let id = self.allocate(HeapData::Str(s.clone()));
            self.interned_strings.insert(s, id);
            id
        }
    }
}
```

**Benefit**: `"hello" is "hello"` returns `True` (same interned string)

## Migration Strategy

### Compatibility Layer

During migration, support both old and new APIs:

```rust
// Old API (deprecated)
impl Object {
    #[deprecated]
    pub fn add_old(&self, other: &Object) -> Option<Object> {
        let mut heap = Heap::new();
        self.add(other, &mut heap)
    }
}
```

### Gradual Migration

1. **Phase 1-2**: Internal only, tests still pass
2. **Phase 3**: Behavior changes (reference semantics)
3. **Phase 4+**: New features enabled

### Testing Strategy

At each phase:
1. All existing tests must pass
2. Add new tests for new functionality
3. Add regression tests for Python semantics

## Examples: Before vs After

### Example 1: Shared Mutable State

**Before (Wrong)**:
```python
a = [1, 2, 3]
b = a        # b is a clone
b.append(4)
print(a)     # [1, 2, 3] - unchanged (WRONG!)
```

**After (Correct)**:
```python
a = [1, 2, 3]
b = a        # b references same list
b.append(4)
print(a)     # [1, 2, 3, 4] - correct!
```

### Example 2: Object Identity

**Before (Impossible)**:
```python
a = [1, 2, 3]
b = [1, 2, 3]
print(a is b)  # Can't implement correctly
```

**After (Correct)**:
```python
a = [1, 2, 3]  # ObjectId(0)
b = [1, 2, 3]  # ObjectId(1)
c = a          # ObjectId(0) - same as a

print(a is b)  # False - different IDs
print(a is c)  # True - same ID
print(a == b)  # True - equal values
```

### Example 3: Default Mutable Arguments

**Before (Wrong)**:
```python
def append_to(item, lst=[]):
    lst.append(item)
    return lst

print(append_to(1))  # [1]
print(append_to(2))  # [2] - WRONG! New list each time
```

**After (Correct)**:
```python
def append_to(item, lst=[]):
    lst.append(item)
    return lst

print(append_to(1))  # [1]
print(append_to(2))  # [1, 2] - correct! Same list
```

## Performance Characteristics

### Memory

**Before**: `size_of::<Object>()` = 32 bytes (largest variant)
**After**: `size_of::<Object>()` = 16 bytes (8-byte discriminant + 8-byte value/ID)

**Improvement**: 50% reduction in Object size

### Allocations

**Before**: Every operation clones
```python
x = [1, 2, 3]  # Allocation
y = x          # Full clone (allocation + copy)
z = x          # Another full clone
```

**After**: Reference counting, no clones
```python
x = [1, 2, 3]  # Allocation
y = x          # Just inc_ref (no allocation)
z = x          # Just inc_ref (no allocation)
```

### Operations

| Operation | Before | After |
|-----------|--------|-------|
| `y = x` (list) | O(n) clone | O(1) inc_ref |
| `list.append()` | O(n) clone + append | O(1) append |
| Function call | O(n) clone all args | O(1) inc_ref args |
| Comparison `==` | O(n) deep compare | O(n) deep compare |
| Identity `is` | Impossible | O(1) ID compare |

## Remaining Limitations

This design solves reference semantics but does NOT solve:

1. **Closures**: Need separate environment capture mechanism
2. **Nested scopes**: Need scope chain (separate from heap)
3. **Global/nonlocal**: Need multi-level namespace lookup
4. **Circular references**: Leak memory without cycle detector
   - This is not optional for real Python workloads; schedule at least a mark/sweep pass before enabling user-defined classes or closures.
5. **Lifetime 'c**: Still need owned AST for `eval()`
6. **Scope chains**: LEGB resolution remains flat today, so heap IDs must eventually pair with stacked namespaces and captured environments.

These require additional architectural changes beyond the heap design.

## Future Enhancements

### 1. Cycle Detection

Add mark-and-sweep GC for unreachable cycles. This must land immediately after Phase 3 so closures, classes, and default arguments do not leak per execution:

```rust
impl Heap {
    pub fn collect_garbage(&mut self, roots: &[Object]) {
        // Mark phase
        let mut marked = HashSet::new();
        self.mark_recursive(roots, &mut marked);

        // Sweep phase
        for id in 0..self.objects.len() {
            if let Some(Some(obj)) = self.objects.get(id) {
                if !marked.contains(&id) && obj.refcount > 0 {
                    // Found unreachable cycle
                    self.free_object(id);
                }
            }
        }
    }
}
```

### 2. Compacting GC

When heap becomes fragmented, compact:

```rust
impl Heap {
    pub fn compact(&mut self) -> HashMap<ObjectId, ObjectId> {
        // Move all live objects to front of array
        // Return mapping of old ID -> new ID
        // Update all references
    }
}
```

## Conclusion

The arena hybrid design provides:

✅ **Python-compatible reference semantics**
✅ **Object identity** for `is` operator
✅ **Efficient** immediate values for common cases
✅ **Safe** reference counting with clear ownership
✅ **Simple** no ID reuse eliminates entire class of bugs
✅ **Extensible** foundation for GC, closures, classes
✅ **Debuggable** can inspect entire heap state

The simplified approach (no free list, monotonic IDs per run) trades some memory efficiency for significant implementation simplicity and safety. For Monty's use case (sandboxed execution), this is an excellent trade-off, provided we land the accompanying cycle detection and scope-chain work immediately after the core heap rollout.

## Next Steps

**Core heap design is implemented.** Remaining work:

1. **Dictionary support**: Add `HeapData::Dict` variant
2. **Cycle detection**: Mark-sweep GC for circular references
3. **String interning**: Optional optimization
4. **User-defined functions**: Requires scope chains
5. **Classes**: Object model, MRO, descriptors
