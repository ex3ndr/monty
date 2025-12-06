use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use strum::Display;

use crate::args::ArgObjects;
use crate::callable::Callable;
use crate::exceptions::{exc_err_fmt, ExcType, SimpleException};
use crate::function::Function;
use crate::heap::HeapData;
use crate::heap::{Heap, ObjectId};
use crate::run::RunResult;
use crate::values::bytes::bytes_repr;
use crate::values::str::string_repr;
use crate::values::PyValue;

/// Primary value type representing Python objects at runtime.
///
/// This enum uses a hybrid design: small immediate values (Int, Bool, None) are stored
/// inline, while heap-allocated objects (List, Str, Dict, etc.) are stored in the arena
/// and referenced via `Ref(ObjectId)`.
///
/// NOTE: `Clone` is intentionally NOT derived. Use `clone_with_heap()` for heap objects
/// or `clone_immediate()` for immediate values only. Direct cloning via `.clone()` would
/// bypass reference counting and cause memory leaks.
#[derive(Debug)]
pub enum Object<'c, 'e> {
    // Immediate values (stored inline, no heap allocation)
    Undefined,
    Ellipsis,
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Range(i64),
    InternString(&'e str),
    InternBytes(&'e [u8]),
    /// Exception instance (e.g., result of `ValueError('msg')`).
    Exc(SimpleException<'c>),
    /// Callables, nested enum to make calling easier, allow private until Object is privates
    #[allow(private_interfaces)]
    Callable(Callable<'c>),
    /// A function defined in the module
    #[allow(private_interfaces)]
    Function(&'e Function<'c>),

    // Heap-allocated values (stored in arena)
    Ref(ObjectId),
}

impl From<bool> for Object<'_, '_> {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl<'c, 'e> PyValue<'c, 'e> for Object<'c, 'e> {
    fn py_type(&self, heap: &Heap<'c, 'e>) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Ellipsis => "ellipsis",
            Self::None => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Range(_) => "range",
            Self::InternString(_) => "str",
            Self::InternBytes(_) => "bytes",
            Self::Exc(e) => e.type_str(),
            Self::Callable(c) => c.py_type(heap),
            Self::Function(_) => "function",
            Self::Ref(id) => heap.get(*id).py_type(heap),
        }
    }

    fn py_len(&self, heap: &Heap<'c, 'e>) -> Option<usize> {
        match self {
            Self::InternString(s) => Some(s.len()),
            Self::InternBytes(b) => Some(b.len()),
            Self::Ref(id) => heap.get(*id).py_len(heap),
            _ => None,
        }
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<'c, 'e>) -> bool {
        match (self, other) {
            (Self::Undefined, _) => false,
            (_, Self::Undefined) => false,
            (Self::Int(v1), Self::Int(v2)) => v1 == v2,
            (Self::Range(v1), Self::Range(v2)) => v1 == v2,
            (Self::Bool(v1), Self::Bool(v2)) => v1 == v2,
            (Self::Bool(v1), Self::Int(v2)) => i64::from(*v1) == *v2,
            (Self::Int(v1), Self::Bool(v2)) => *v1 == i64::from(*v2),
            (Self::None, Self::None) => true,

            (Self::InternString(s1), Self::InternString(s2)) => s1 == s2,
            // for strings we need to account for the fact they might be either interned or not
            (Self::InternString(s1), Self::Ref(id2)) => {
                if let HeapData::Str(s2) = heap.get(*id2) {
                    *s1 == s2.as_str()
                } else {
                    false
                }
            }
            (Self::Ref(id1), Self::InternString(s2)) => {
                if let HeapData::Str(s1) = heap.get(*id1) {
                    s1.as_str() == *s2
                } else {
                    false
                }
            }

            (Self::InternBytes(b1), Self::InternBytes(b2)) => b1 == b2,
            // same for bytes
            (Self::InternBytes(b1), Self::Ref(id2)) => {
                if let HeapData::Bytes(b2) = heap.get(*id2) {
                    *b1 == b2.as_slice()
                } else {
                    false
                }
            }
            (Self::Ref(id1), Self::InternBytes(b2)) => {
                if let HeapData::Bytes(b1) = heap.get(*id1) {
                    b1.as_slice() == *b2
                } else {
                    false
                }
            }

            (Self::Ref(id1), Self::Ref(id2)) => {
                if *id1 == *id2 {
                    return true;
                }
                // Need to use with_two for proper borrow management
                heap.with_two(*id1, *id2, |heap, left, right| left.py_eq(right, heap))
            }
            _ => false,
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    fn py_cmp(&self, other: &Self, heap: &mut Heap<'c, 'e>) -> Option<Ordering> {
        match (self, other) {
            (Self::Int(s), Self::Int(o)) => s.partial_cmp(o),
            (Self::Float(s), Self::Float(o)) => s.partial_cmp(o),
            (Self::Int(s), Self::Float(o)) => (*s as f64).partial_cmp(o),
            (Self::Float(s), Self::Int(o)) => s.partial_cmp(&(*o as f64)),
            (Self::Bool(s), _) => Self::Int(i64::from(*s)).py_cmp(other, heap),
            (_, Self::Bool(s)) => self.py_cmp(&Self::Int(i64::from(*s)), heap),
            (Self::InternString(s1), Self::InternString(s2)) => s1.partial_cmp(s2),
            (Self::InternBytes(b1), Self::InternBytes(b2)) => b1.partial_cmp(b2),
            // Ref comparison requires heap context, not supported in PartialOrd
            (Self::Ref(_), Self::Ref(_)) => None,
            _ => None,
        }
    }

    fn py_dec_ref_ids(&self, stack: &mut Vec<ObjectId>) {
        if let Object::Ref(id) = self {
            stack.push(*id);
        }
    }

    fn py_bool(&self, heap: &Heap<'c, 'e>) -> bool {
        match self {
            Self::Undefined => false,
            Self::Ellipsis => true,
            Self::None => false,
            Self::Bool(b) => *b,
            Self::Int(v) => *v != 0,
            Self::Float(f) => *f != 0.0,
            Self::Range(v) => *v != 0,
            Self::Exc(_) => true,
            Self::Callable(_) => true,
            Self::Function(_) => true,
            Self::InternString(s) => !s.is_empty(),
            Self::InternBytes(b) => !b.is_empty(),
            Self::Ref(id) => heap.get(*id).py_bool(heap),
        }
    }

    fn py_repr<'a>(&'a self, heap: &'a Heap<'c, 'e>) -> Cow<'a, str> {
        match self {
            Self::Undefined => "Undefined".into(),
            Self::Ellipsis => "Ellipsis".into(),
            Self::None => "None".into(),
            Self::Bool(true) => "True".into(),
            Self::Bool(false) => "False".into(),
            Self::Int(v) => format!("{v}").into(),
            Self::Float(v) => {
                let s = v.to_string();
                if s.contains('.') {
                    s.into()
                } else {
                    format!("{s}.0").into()
                }
            }
            Self::Range(size) => format!("0:{size}").into(),
            Self::Exc(exc) => format!("{exc}").into(),
            Self::Callable(c) => c.py_repr(heap),
            Self::Function(f) => f.py_repr(),
            Self::InternString(s) => string_repr(s).into(),
            Self::InternBytes(b) => bytes_repr(b).into(),
            Self::Ref(id) => heap.get(*id).py_repr(heap),
        }
    }

    fn py_str<'a>(&'a self, heap: &'a Heap<'c, 'e>) -> Cow<'a, str> {
        match self {
            Self::InternString(s) => (*s).into(),
            Self::Ref(id) => heap.get(*id).py_str(heap),
            _ => self.py_repr(heap),
        }
    }

    fn py_add(&self, other: &Self, heap: &mut Heap<'c, 'e>) -> Option<Object<'c, 'e>> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Object::Int(v1 + v2)),
            (Self::Float(v1), Self::Float(v2)) => Some(Object::Float(v1 + v2)),
            (Self::Ref(id1), Self::Ref(id2)) => heap.with_two(*id1, *id2, |heap, left, right| left.py_add(right, heap)),
            (Self::InternString(s1), Self::InternString(s2)) => {
                let concat = format!("{s1}{s2}");
                Some(Object::Ref(heap.allocate(HeapData::Str(concat.into()))))
            }
            // for strings we need to account for the fact they might be either interned or not
            (Self::InternString(s1), Self::Ref(id2)) => {
                if let HeapData::Str(s2) = heap.get(*id2) {
                    let concat = format!("{}{}", s1, s2.as_str());
                    Some(Object::Ref(heap.allocate(HeapData::Str(concat.into()))))
                } else {
                    None
                }
            }
            (Self::Ref(id1), Self::InternString(s2)) => {
                if let HeapData::Str(s1) = heap.get(*id1) {
                    let concat = format!("{}{}", s1.as_str(), s2);
                    Some(Object::Ref(heap.allocate(HeapData::Str(concat.into()))))
                } else {
                    None
                }
            }
            // same for bytes
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                let mut b = Vec::with_capacity(b1.len() + b2.len());
                b.extend_from_slice(b1);
                b.extend_from_slice(b2);
                Some(Object::Ref(heap.allocate(HeapData::Bytes(b.into()))))
            }
            (Self::InternBytes(b1), Self::Ref(id2)) => {
                if let HeapData::Bytes(b2) = heap.get(*id2) {
                    let mut b = Vec::with_capacity(b1.len() + b2.len());
                    b.extend_from_slice(b1);
                    b.extend_from_slice(b2);
                    Some(Object::Ref(heap.allocate(HeapData::Bytes(b.into()))))
                } else {
                    None
                }
            }
            (Self::Ref(id1), Self::InternBytes(b2)) => {
                if let HeapData::Bytes(b1) = heap.get(*id1) {
                    let mut b = Vec::with_capacity(b1.len() + b2.len());
                    b.extend_from_slice(b1);
                    b.extend_from_slice(b2);
                    Some(Object::Ref(heap.allocate(HeapData::Bytes(b.into()))))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn py_sub(&self, other: &Self, _heap: &mut Heap<'c, 'e>) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Object::Int(v1 - v2)),
            _ => None,
        }
    }

    fn py_mod(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Object::Int(v1 % v2)),
            (Self::Float(v1), Self::Float(v2)) => Some(Object::Float(v1 % v2)),
            (Self::Float(v1), Self::Int(v2)) => Some(Object::Float(v1 % (*v2 as f64))),
            (Self::Int(v1), Self::Float(v2)) => Some(Object::Float((*v1 as f64) % v2)),
            _ => None,
        }
    }

    fn py_mod_eq(&self, other: &Self, right_value: i64) -> Option<bool> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(v1 % v2 == right_value),
            (Self::Float(v1), Self::Float(v2)) => Some(v1 % v2 == right_value as f64),
            (Self::Float(v1), Self::Int(v2)) => Some(v1 % (*v2 as f64) == right_value as f64),
            (Self::Int(v1), Self::Float(v2)) => Some((*v1 as f64) % v2 == right_value as f64),
            _ => None,
        }
    }

    fn py_iadd(&mut self, other: Self, heap: &mut Heap<'c, 'e>, _self_id: Option<ObjectId>) -> bool {
        match (&self, &other) {
            (Self::Int(v1), Self::Int(v2)) => {
                *self = Object::Int(*v1 + v2);
                true
            }
            (Self::Float(v1), Self::Float(v2)) => {
                *self = Object::Float(*v1 + *v2);
                true
            }
            (Self::InternString(s1), Self::InternString(s2)) => {
                let concat = format!("{s1}{s2}");
                *self = Object::Ref(heap.allocate(HeapData::Str(concat.into())));
                true
            }
            (Self::InternString(s1), Self::Ref(id2)) => {
                if let HeapData::Str(s2) = heap.get(*id2) {
                    let concat = format!("{}{}", s1, s2.as_str());
                    *self = Object::Ref(heap.allocate(HeapData::Str(concat.into())));
                    true
                } else {
                    false
                }
            }
            (Self::Ref(id1), Self::InternString(s2)) => {
                if let HeapData::Str(s1) = heap.get_mut(*id1) {
                    s1.as_string_mut().push_str(s2);
                    true
                } else {
                    false
                }
            }
            // same for bytes
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                let mut b = Vec::with_capacity(b1.len() + b2.len());
                b.extend_from_slice(b1);
                b.extend_from_slice(b2);
                *self = Object::Ref(heap.allocate(HeapData::Bytes(b.into())));
                true
            }
            (Self::InternBytes(b1), Self::Ref(id2)) => {
                if let HeapData::Bytes(b2) = heap.get(*id2) {
                    let mut b = Vec::with_capacity(b1.len() + b2.len());
                    b.extend_from_slice(b1);
                    b.extend_from_slice(b2);
                    *self = Object::Ref(heap.allocate(HeapData::Bytes(b.into())));
                    true
                } else {
                    false
                }
            }
            (Self::Ref(id1), Self::InternBytes(b2)) => {
                if let HeapData::Bytes(b1) = heap.get_mut(*id1) {
                    b1.as_vec_mut().extend_from_slice(b2);
                    true
                } else {
                    false
                }
            }
            (Self::Ref(id), Self::Ref(_)) => {
                heap.with_entry_mut(*id, |heap, data| data.py_iadd(other, heap, Some(*id)))
            }
            _ => false,
        }
    }

    fn py_getitem(&self, key: &Self, heap: &mut Heap<'c, 'e>) -> RunResult<'c, Self> {
        match self {
            Object::Ref(id) => {
                // Need to take entry out to allow mutable heap access
                let id = *id;
                heap.with_entry_mut(id, |heap, data| data.py_getitem(key, heap))
            }
            _ => Err(ExcType::type_error_not_sub(self.py_type(heap))),
        }
    }

    // fn py_call(&self, heap: &mut Heap<'c, 'e>, args: ArgObjects<'c, 'e>) -> Option<RunResult<'c, Self>> {
    //     match self {
    //         Self::Callable(c) => Some(c.call(heap, args)),
    //         _ => None,
    //     }
    // }
}

impl<'c, 'e> Object<'c, 'e> {
    /// Returns a stable, unique identifier for this object, boxing it to the heap if necessary.
    ///
    /// Should match Python's `id()` function.
    ///
    /// For inline values (Int, Float, Range), this method allocates them to the heap on first call
    /// and replaces `self` with an `Object::Ref` pointing to the boxed value. This ensures that
    /// subsequent calls to `id()` return the same stable heap address.
    ///
    /// Singletons (None, True, False, etc.) return IDs from a dedicated tagged range without allocation.
    /// Already heap-allocated objects (Ref) reuse their `ObjectId` inside the heap-tagged range.
    pub fn id(&mut self, heap: &mut Heap<'c, 'e>) -> usize {
        match self {
            // should not be used in practice
            Self::Undefined => singleton_id(SingletonSlot::Undefined),
            // Singletons have fixed tagged IDs
            Self::Ellipsis => singleton_id(SingletonSlot::Ellipsis),
            Self::None => singleton_id(SingletonSlot::None),
            Self::Bool(b) => {
                if *b {
                    singleton_id(SingletonSlot::True)
                } else {
                    singleton_id(SingletonSlot::False)
                }
            }
            Self::Function(f) => f.id(),
            Self::InternString(s) => {
                interned_id_from_parts(s.as_ptr() as usize, s.len(), INTERN_STR_ID_TAG, INTERN_STR_ID_MASK)
            }
            Self::InternBytes(b) => {
                interned_id_from_parts(b.as_ptr() as usize, b.len(), INTERN_BYTES_ID_TAG, INTERN_BYTES_ID_MASK)
            }
            // Already heap-allocated, return id within a dedicated tag range
            Self::Ref(id) => heap_tagged_id(*id),
            // Everything else needs to be added to the heap to get a stable address
            _ => {
                // Use clone_immediate since these are all non-Ref variants
                let new_id = heap.allocate(HeapData::Object(self.clone_immediate()));
                // Replace self with a Ref to the newly allocated heap object
                *self = Self::Ref(new_id);
                heap_tagged_id(new_id)
            }
        }
    }

    /// Equivalent of Python's `is` method.
    pub fn is(&mut self, heap: &mut Heap<'c, 'e>, other: &mut Self) -> bool {
        self.id(heap) == other.id(heap)
    }

    /// Computes the hash value for this object, used for dict keys.
    ///
    /// Returns Some(hash) for hashable types (immediate values and immutable heap types).
    /// Returns None for unhashable types (list, dict).
    ///
    /// For heap-allocated objects (Ref variant), this computes the hash lazily
    /// on first use and caches it for subsequent calls.
    pub fn py_hash_u64(&self, heap: &mut Heap<'c, 'e>) -> Option<u64> {
        match self {
            // Immediate values can be hashed directly
            Self::Undefined => Some(0),
            Self::Ellipsis => Some(1),
            Self::None => Some(2),
            Self::Bool(b) => {
                let mut hasher = DefaultHasher::new();
                b.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Int(i) => {
                let mut hasher = DefaultHasher::new();
                i.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Float(f) => {
                let mut hasher = DefaultHasher::new();
                // Hash the bit representation of float for consistency
                f.to_bits().hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Range(r) => {
                let mut hasher = DefaultHasher::new();
                r.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Exc(e) => {
                // Exceptions are rarely used as dict keys, but we provide a hash
                // based on the exception type and argument for proper distribution
                Some(e.py_hash())
            }
            Self::Callable(c) => {
                // Builtins have a fixed identity, hash based on variant discriminant
                let mut hasher = DefaultHasher::new();
                std::mem::discriminant(c).hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Function(f) => {
                let mut hasher = DefaultHasher::new();
                // TODO, this is NOT proper hashing, we should somehow hash the function properly
                f.name.name.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::InternString(s) => {
                let mut hasher = DefaultHasher::new();
                s.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::InternBytes(b) => {
                let mut hasher = DefaultHasher::new();
                b.hash(&mut hasher);
                Some(hasher.finish())
            }
            // For heap-allocated objects, compute hash lazily and cache it
            Self::Ref(id) => heap.get_or_compute_hash(*id),
        }
    }

    /// TODO maybe replace with TryFrom
    pub fn as_int(&self) -> RunResult<'static, i64> {
        match self {
            Self::Int(i) => Ok(*i),
            // TODO use self.type
            _ => exc_err_fmt!(ExcType::TypeError; "'{self:?}' object cannot be interpreted as an integer"),
        }
    }

    /// Calls an attribute method on this object (e.g., list.append()).
    ///
    /// This method requires heap access to work with heap-allocated objects and
    /// to generate accurate error messages.
    pub fn call_attr(
        &mut self,
        heap: &mut Heap<'c, 'e>,
        attr: &Attr,
        args: ArgObjects<'c, 'e>,
    ) -> RunResult<'c, Object<'c, 'e>> {
        if let Self::Ref(id) = self {
            heap.call_attr(*id, attr, args)
        } else {
            Err(ExcType::attribute_error(self.py_type(heap), attr))
        }
    }

    /// Clones an object with proper heap reference counting.
    ///
    /// For immediate values (Int, Bool, None, etc.), this performs a simple copy.
    /// For heap-allocated objects (Ref variant), this increments the reference count
    /// and returns a new reference to the same heap object.
    ///
    /// # Important
    /// This method MUST be used instead of the derived `Clone` implementation to ensure
    /// proper reference counting. Using `.clone()` directly will bypass reference counting
    /// and cause memory leaks or double-frees.
    #[must_use]
    pub fn clone_with_heap(&self, heap: &mut Heap<'c, 'e>) -> Self {
        match self {
            Self::Ref(id) => {
                heap.inc_ref(*id);
                Self::Ref(*id)
            }
            // Immediate values can be copied without heap interaction
            other => other.clone_immediate(),
        }
    }

    /// Drops an object, decrementing its heap reference count if applicable.
    ///
    /// For immediate values, this is a no-op. For heap-allocated objects (Ref variant),
    /// this decrements the reference count and frees the object (and any children) when
    /// the count reaches zero.
    ///
    /// # Important
    /// This method MUST be called before overwriting a namespace slot or discarding
    /// a value to prevent memory leaks.
    pub fn drop_with_heap(self, heap: &mut Heap<'c, 'e>) {
        if let Self::Ref(id) = self {
            heap.dec_ref(id);
        }
    }

    /// Internal helper for copying immediate values without heap interaction.
    ///
    /// This method should only be called by `clone_with_heap()` for immediate values.
    /// Attempting to clone a Ref variant will panic.
    fn clone_immediate(&self) -> Self {
        match self {
            Self::Undefined => Self::Undefined,
            Self::Ellipsis => Self::Ellipsis,
            Self::None => Self::None,
            Self::Bool(b) => Self::Bool(*b),
            Self::Int(v) => Self::Int(*v),
            Self::Float(v) => Self::Float(*v),
            Self::Range(v) => Self::Range(*v),
            Self::Exc(e) => Self::Exc(e.clone()),
            Self::Callable(c) => Self::Callable(c.clone()),
            Self::Function(f) => Self::Function(f),
            Self::InternString(s) => Self::InternString(s),
            Self::InternBytes(b) => Self::InternBytes(b),
            Self::Ref(_) => panic!("Ref clones must go through clone_with_heap to maintain refcounts"),
        }
    }

    /// Creates a shallow copy of this Object without incrementing reference counts.
    ///
    /// IMPORTANT: For Ref variants, this copies the ObjectId but does NOT increment
    /// the reference count. The caller MUST call `heap.inc_ref()` separately for any
    /// Ref variants to maintain correct reference counting.
    ///
    /// This is useful when you need to copy Objects from a borrowed heap context
    /// and will increment refcounts in a separate step.
    pub(crate) fn copy_for_extend(&self) -> Self {
        match self {
            Self::Undefined => Self::Undefined,
            Self::Ellipsis => Self::Ellipsis,
            Self::None => Self::None,
            Self::Bool(b) => Self::Bool(*b),
            Self::Int(v) => Self::Int(*v),
            Self::Float(v) => Self::Float(*v),
            Self::Range(v) => Self::Range(*v),
            Self::Exc(e) => Self::Exc(e.clone()),
            Self::Callable(c) => Self::Callable(c.clone()),
            Self::Function(f) => Self::Function(f),
            Self::InternString(s) => Self::InternString(s),
            Self::InternBytes(b) => Self::InternBytes(b),
            Self::Ref(id) => Self::Ref(*id), // Caller must increment refcount!
        }
    }
}

/// Attribute names for method calls on container types (list, dict).
///
/// Uses strum `Display` derive with lowercase serialization.
/// The `Other(String)` variant is a fallback for unknown/dynamic attribute names.
#[derive(Debug, Clone, Display)]
#[strum(serialize_all = "lowercase")]
pub enum Attr {
    Append,
    Insert,
    Get,
    Keys,
    Values,
    Items,
    Pop,
    /// Fallback for unknown attribute names. Displays as the contained string.
    #[strum(default)]
    Other(String),
}

impl From<String> for Attr {
    fn from(name: String) -> Self {
        match name.as_str() {
            "append" => Self::Append,
            "insert" => Self::Insert,
            "get" => Self::Get,
            "keys" => Self::Keys,
            "values" => Self::Values,
            "items" => Self::Items,
            "pop" => Self::Pop,
            _ => Self::Other(name),
        }
    }
}

/// High-bit tag reserved for literal singletons (None, Ellipsis, booleans).
const SINGLETON_ID_TAG: usize = 1usize << (usize::BITS - 1);
/// High-bit tag reserved for interned string `id()` values.
const INTERN_STR_ID_TAG: usize = 1usize << (usize::BITS - 2);
/// High-bit tag reserved for interned bytes `id()` values to avoid colliding with any other space.
const INTERN_BYTES_ID_TAG: usize = 1usize << (usize::BITS - 3);
/// High-bit tag reserved for heap-backed `ObjectId`s.
const HEAP_OBJECT_ID_TAG: usize = 1usize << (usize::BITS - 4);

/// Mask that keeps pointer-derived bits below the bytes tag bit.
const INTERN_BYTES_ID_MASK: usize = INTERN_BYTES_ID_TAG - 1;
/// Mask that keeps pointer-derived bits below the string tag bit.
const INTERN_STR_ID_MASK: usize = INTERN_STR_ID_TAG - 1;
/// Mask that keeps per-singleton offsets below the singleton tag bit.
const SINGLETON_ID_MASK: usize = SINGLETON_ID_TAG - 1;
/// Mask that keeps heap object IDs below the heap tag bit.
const HEAP_OBJECT_ID_MASK: usize = HEAP_OBJECT_ID_TAG - 1;

/// Rotate distance used when folding slice length into the pointer identity.
const INTERN_LEN_ROTATE: u32 = usize::BITS / 2;

/// Mixes a slice pointer and its length into a deterministic identity
/// that lives in a reserved numeric range controlled by `tag`.
///
/// This lets us use literal storage addresses for stable `id()` values without
/// ever overlapping the sequential heap `ObjectId` space.
#[inline]
fn interned_id_from_parts(ptr: usize, len: usize, tag: usize, mask: usize) -> usize {
    let mixed = (ptr ^ len.rotate_left(INTERN_LEN_ROTATE)) & mask;
    tag | mixed
}

/// Enumerates singleton literal slots so we can issue stable `id()` values without heap allocation.
#[repr(usize)]
#[derive(Copy, Clone)]
enum SingletonSlot {
    Undefined = 0,
    Ellipsis = 1,
    None = 2,
    False = 3,
    True = 4,
}

/// Returns the fully tagged `id()` value for the requested singleton literal.
#[inline]
const fn singleton_id(slot: SingletonSlot) -> usize {
    SINGLETON_ID_TAG | ((slot as usize) & SINGLETON_ID_MASK)
}

/// Converts a heap `ObjectId` into its tagged `id()` value, ensuring it never collides with other spaces.
#[inline]
pub fn heap_tagged_id(object_id: ObjectId) -> usize {
    HEAP_OBJECT_ID_TAG | (object_id & HEAP_OBJECT_ID_MASK)
}
