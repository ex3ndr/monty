use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;

use crate::exceptions::{exc_err_fmt, ExcType, SimpleException};
use crate::heap::HeapData;
use crate::heap::{Heap, ObjectId};
use crate::run::RunResult;
use crate::{ParseError, ParseResult};

/// Primary value type representing Python objects at runtime.
///
/// This enum uses a hybrid design: small immediate values (Int, Bool, None) are stored
/// inline, while heap-allocated objects (List, Str, Dict, etc.) are stored in the arena
/// and referenced via `Ref(ObjectId)`.
///
/// NOTE: We intentionally keep `Clone` and `PartialEq` derives temporarily during
/// migration, but these will be removed once all code uses `clone_with_heap()` and
/// heap-aware comparisons. Direct cloning bypasses reference counting and will cause leaks.
#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    // Immediate values (stored inline, no heap allocation)
    Undefined,
    Ellipsis,
    None,
    True,
    False,
    Int(i64),
    Float(f64),
    Range(i64),
    Exc(SimpleException),

    // Heap-allocated values (stored in arena)
    Ref(ObjectId),
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.cow_str())
    }
}

impl PartialOrd for Object {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Int(s), Self::Int(o)) => s.partial_cmp(o),
            (Self::Float(s), Self::Float(o)) => s.partial_cmp(o),
            (Self::Int(s), Self::Float(o)) => (*s as f64).partial_cmp(o),
            (Self::Float(s), Self::Int(o)) => s.partial_cmp(&(*o as f64)),
            (Self::True, _) => Self::Int(1).partial_cmp(other),
            (Self::False, _) => Self::Int(0).partial_cmp(other),
            (_, Self::True) => self.partial_cmp(&Self::Int(1)),
            (_, Self::False) => self.partial_cmp(&Self::Int(0)),
            // Ref comparison requires heap context, not supported in PartialOrd
            (Self::Ref(_), Self::Ref(_)) => None,
            _ => None,
        }
    }
}

impl From<bool> for Object {
    fn from(v: bool) -> Self {
        if v {
            Self::True
        } else {
            Self::False
        }
    }
}

impl Object {
    /// Performs addition on two objects, allocating result on heap if necessary.
    ///
    /// For heap-allocated objects (Ref variant), this method accesses the heap to perform
    /// the operation and allocates the result on the heap with refcount=1.
    #[must_use]
    pub fn add(&self, other: &Self, heap: &mut Heap) -> Option<Self> {
        use crate::heap::HeapData;

        match (self, other) {
            // Immediate value addition
            (Self::Int(v1), Self::Int(v2)) => Some(Self::Int(v1 + v2)),

            // Heap-allocated object addition
            (Self::Ref(id1), Self::Ref(id2)) => {
                let data1 = heap.get(*id1);
                let data2 = heap.get(*id2);
                match (data1, data2) {
                    (HeapData::Str(s1), HeapData::Str(s2)) => {
                        let result = format!("{s1}{s2}");
                        let id = heap.allocate(HeapData::Str(result));
                        Some(Self::Ref(id))
                    }
                    (HeapData::List(list1), HeapData::List(list2)) => {
                        // Clone the first list's items and extend with second list
                        let mut result = list1.clone();
                        result.extend_from_slice(list2);
                        // Inc ref for all items in result (they're now referenced twice)
                        for obj in &result {
                            if let Self::Ref(id) = obj {
                                heap.inc_ref(*id);
                            }
                        }
                        let id = heap.allocate(HeapData::List(result));
                        Some(Self::Ref(id))
                    }
                    _ => None,
                }
            }

            _ => None,
        }
    }

    /// Performs in-place addition, mutating the left operand.
    ///
    /// For heap-allocated objects, this modifies the heap data directly.
    /// Returns Ok(()) on success, or Err(other) if the operation is not supported.
    pub fn add_mut(&mut self, other: Self, heap: &mut Heap) -> Result<(), Self> {
        use crate::heap::HeapData;

        match (self, other) {
            // Immediate value mutation
            (Self::Int(v1), Self::Int(v2)) => {
                *v1 += v2;
            }

            // Heap-allocated object mutation
            (Self::Ref(id1), Self::Ref(id2)) => {
                // Clone the second object's data before mutating the first
                let data2: HeapData = heap.get(id2).clone();

                match heap.get_mut(*id1) {
                    HeapData::Str(s1) => {
                        if let HeapData::Str(s2) = data2 {
                            s1.push_str(&s2);
                        } else {
                            return Err(Self::Ref(id2));
                        }
                    }
                    HeapData::List(list1) => {
                        if let HeapData::List(mut list2) = data2 {
                            // Collect IDs to inc_ref after releasing the borrow
                            let ids_to_inc: Vec<ObjectId> = list2
                                .iter()
                                .filter_map(|obj| if let Self::Ref(id) = obj { Some(*id) } else { None })
                                .collect();
                            // Extend list1 with list2 items by appending them individually
                            list1.append(&mut list2);
                            // Release the mutable borrow
                            let _ = list1;
                            // Now inc_ref for all heap objects
                            for id in ids_to_inc {
                                heap.inc_ref(id);
                            }
                        } else {
                            return Err(Self::Ref(id2));
                        }
                    }
                    _ => return Err(Self::Ref(id2)),
                }
            }

            (_, other) => return Err(other),
        }
        Ok(())
    }

    #[must_use]
    pub fn sub(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Self::Int(v1 - v2)),
            _ => None,
        }
    }

    /// different name to avoid confusion with `PartialEq::eq`
    #[must_use]
    pub fn py_eq(&self, other: &Self, heap: &Heap) -> bool {
        match (self, other) {
            (Self::Undefined, _) => false,
            (_, Self::Undefined) => false,
            (Self::Int(v1), Self::Int(v2)) => v1 == v2,
            (Self::Range(v1), Self::Range(v2)) => v1 == v2,
            (Self::True, Self::True) => true,
            (Self::True, Self::Int(v2)) => 1 == *v2,
            (Self::Int(v1), Self::True) => *v1 == 1,
            (Self::False, Self::False) => true,
            (Self::False, Self::Int(v2)) => 0 == *v2,
            (Self::Int(v1), Self::False) => *v1 == 0,
            (Self::None, Self::None) => true,
            (Self::Ref(id1), Self::Ref(id2)) => (*id1 == *id2) || heap.get(*id1).py_eq(heap.get(*id2), heap),
            _ => false,
        }
    }

    /// Returns the truthiness of this object in Python semantics.
    ///
    /// For heap-allocated objects, this method requires heap access to check
    /// if containers are empty.
    #[must_use]
    pub fn bool(&self, heap: &Heap) -> bool {
        use crate::heap::HeapData;

        match self {
            // Immediate values
            Self::Undefined => false,
            Self::Ellipsis => true,
            Self::None => false,
            Self::True => true,
            Self::False => false,
            Self::Int(v) => *v != 0,
            Self::Float(f) => *f != 0.0,
            Self::Range(v) => *v != 0,
            Self::Exc(_) => true,
            Self::Ref(id) => match heap.get(*id) {
                HeapData::Str(s) => !s.is_empty(),
                HeapData::Bytes(b) => !b.is_empty(),
                HeapData::List(items) => !items.is_empty(),
                HeapData::Tuple(items) => !items.is_empty(),
            },
        }
    }

    #[must_use]
    pub fn modulus(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Self::Int(v1 % v2)),
            (Self::Float(v1), Self::Float(v2)) => Some(Self::Float(v1 % v2)),
            (Self::Float(v1), Self::Int(v2)) => Some(Self::Float(v1 % (*v2 as f64))),
            (Self::Int(v1), Self::Float(v2)) => Some(Self::Float((*v1 as f64) % v2)),
            _ => None,
        }
    }

    #[must_use]
    pub fn modulus_eq(&self, other: &Self, right_value: i64) -> Option<bool> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(v1 % v2 == right_value),
            (Self::Float(v1), Self::Float(v2)) => Some(v1 % v2 == right_value as f64),
            (Self::Float(v1), Self::Int(v2)) => Some(v1 % (*v2 as f64) == right_value as f64),
            (Self::Int(v1), Self::Float(v2)) => Some((*v1 as f64) % v2 == right_value as f64),
            _ => None,
        }
    }

    /// Returns the length of this object if it has one.
    ///
    /// For heap-allocated objects, this method requires heap access to retrieve
    /// the actual length.
    #[allow(clippy::len_without_is_empty)]
    #[must_use]
    pub fn len(&self, heap: &Heap) -> Option<usize> {
        use crate::heap::HeapData;

        match self {
            Self::Ref(id) => match heap.get(*id) {
                HeapData::Str(s) => Some(s.len()),
                HeapData::Bytes(b) => Some(b.len()),
                HeapData::List(items) => Some(items.len()),
                HeapData::Tuple(items) => Some(items.len()),
            },
            _ => None,
        }
    }

    /// Returns a Python-style repr string for this object, e.g. `__repr__` / `repr`
    ///
    /// For heap-allocated objects, this method requires heap access to retrieve
    /// and format the actual data.
    #[must_use]
    pub fn repr<'h>(&self, heap: &'h Heap) -> Cow<'h, str> {
        match self {
            Self::Ref(id) => match heap.get(*id) {
                HeapData::Str(s) => string_repr(s).into(),
                HeapData::Bytes(b) => format!("b'{b:?}'").into(),
                HeapData::List(items) => repr_sequence('[', ']', items, heap).into(),
                HeapData::Tuple(items) => repr_sequence('(', ')', items, heap).into(),
            },
            _ => self.cow_str(),
        }
    }

    /// Implementation of Python's `__str__` / `str`.
    ///
    /// For heap-allocated objects, this method requires heap access to retrieve
    /// and format the actual data.
    #[must_use]
    pub fn str<'h>(&self, heap: &'h Heap) -> Cow<'h, str> {
        if let Self::Ref(id) = self {
            if let HeapData::Str(s) = heap.get(*id) {
                return Cow::Borrowed(s.as_str());
            }
        }
        self.repr(heap)
    }

    /// TODO maybe replace with TryFrom
    pub fn as_int(&self) -> RunResult<'static, i64> {
        match self {
            Self::Int(i) => Ok(*i),
            // TODO use self.type
            _ => exc_err_fmt!(ExcType::TypeError; "'{self:?}' object cannot be interpreted as an integer"),
        }
    }

    /// Returns the Python type name for this object.
    ///
    /// For heap-allocated objects (Ref variant), this method requires heap access
    /// to determine the type.
    #[must_use]
    pub fn type_str(&self, heap: &Heap) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Ellipsis => "ellipsis",
            Self::None => "NoneType",
            Self::True => "bool",
            Self::False => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Range(_) => "range",
            Self::Exc(e) => e.type_str(),
            Self::Ref(id) => heap.get(*id).type_str(),
        }
    }

    /// Calls an attribute method on this object (e.g., list.append()).
    ///
    /// This method requires heap access to work with heap-allocated objects and
    /// to generate accurate error messages.
    pub(crate) fn attr_call<'c>(&mut self, heap: &mut Heap, attr: &Attr, args: Vec<Self>) -> RunResult<'c, Object> {
        use crate::heap::HeapData;

        match (self, attr) {
            // Heap-allocated list support
            (Self::Ref(id), Attr::Append) => {
                let obj_id = *id; // Copy the ID to avoid borrow issues
                if let HeapData::List(list) = heap.get_mut(obj_id) {
                    if args.len() == 1 {
                        let item = args[0].clone();
                        // Store the item_id if it's a Ref, to inc_ref after releasing the borrow
                        let item_id_opt = if let Self::Ref(item_id) = &item {
                            Some(*item_id)
                        } else {
                            None
                        };
                        list.push(item);
                        // Release the mutable borrow before calling inc_ref
                        let _ = list;
                        if let Some(item_id) = item_id_opt {
                            heap.inc_ref(item_id);
                        }
                        Ok(Self::None)
                    } else {
                        exc_err_fmt!(ExcType::TypeError; "{attr} takes exactly exactly one argument ({} given)", args.len())
                    }
                } else {
                    let type_str = Self::Ref(obj_id).type_str(heap);
                    exc_err_fmt!(ExcType::AttributeError; "'{}' object has no attribute '{attr}'", type_str)
                }
            }
            (Self::Ref(id), Attr::Insert) => {
                let obj_id = *id; // Copy the ID to avoid borrow issues
                if let HeapData::List(list) = heap.get_mut(obj_id) {
                    if args.len() == 2 {
                        let index = args[0].as_int()? as usize;
                        let item = args[1].clone();
                        // Store the item_id if it's a Ref, to inc_ref after releasing the borrow
                        let item_id_opt = if let Self::Ref(item_id) = &item {
                            Some(*item_id)
                        } else {
                            None
                        };
                        list.insert(index, item);
                        // Release the mutable borrow before calling inc_ref
                        let _ = list;
                        if let Some(item_id) = item_id_opt {
                            heap.inc_ref(item_id);
                        }
                        Ok(Self::None)
                    } else {
                        exc_err_fmt!(ExcType::TypeError; "{attr} expected 2 arguments, got {}", args.len())
                    }
                } else {
                    let type_str = Self::Ref(obj_id).type_str(heap);
                    exc_err_fmt!(ExcType::AttributeError; "'{}' object has no attribute '{attr}'", type_str)
                }
            }
            (Self::Ref(id), Attr::Foobar) => {
                let id = *id;
                if let HeapData::List(list) = heap.get(id) {
                    Ok(Object::Int(list.len() as i64))
                } else {
                    let type_str = Self::Ref(id).type_str(heap);
                    exc_err_fmt!(ExcType::AttributeError; "'{}' object has no attribute '{attr}'", type_str)
                }
            }

            (s, _) => {
                let type_str = s.type_str(heap);
                exc_err_fmt!(ExcType::AttributeError; "'{}' object has no attribute '{attr}'", type_str)
            }
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
    pub fn clone_with_heap(&self, heap: &mut Heap) -> Self {
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
    pub fn drop_with_heap(self, heap: &mut Heap) {
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
            Self::True => Self::True,
            Self::False => Self::False,
            Self::Int(v) => Self::Int(*v),
            Self::Float(v) => Self::Float(*v),
            Self::Range(v) => Self::Range(*v),
            Self::Exc(e) => Self::Exc(e.clone()),
            Self::Ref(_) => unreachable!("Ref clones must go through clone_with_heap to maintain refcounts"),
        }
    }

    fn cow_str(&self) -> Cow<'static, str> {
        match self {
            Self::Undefined => "Undefined".into(),
            Self::Ellipsis => "...".into(),
            Self::None => "None".into(),
            Self::True => "True".into(),
            Self::False => "False".into(),
            Self::Int(v) => format!("{v}").into(),
            Self::Float(v) => format!("{v}").into(),
            Self::Range(size) => format!("0:{size}").into(),
            Self::Exc(exc) => format!("{exc}").into(),
            Self::Ref(id) => format!("<Ref({id})>").into(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Attr {
    Append,
    Insert,
    Foobar,
}

impl fmt::Display for Attr {
    // TODO replace with a strum
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Append => write!(f, "append"),
            Self::Insert => write!(f, "insert"),
            Self::Foobar => write!(f, "foobar"),
        }
    }
}

impl Attr {
    // TODO replace with a strum
    pub fn find(name: &str) -> ParseResult<'static, Self> {
        match name {
            "append" => Ok(Self::Append),
            "insert" => Ok(Self::Insert),
            "foobar" => Ok(Self::Foobar),
            _ => Err(ParseError::Internal(format!("unknown attribute: `{name}`").into())),
        }
    }
}

macro_rules! string_replace_common {
    ($s:expr) => {
        $s.replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
            .replace('\r', "\\r")
    };
}

pub(crate) fn string_repr(s: &str) -> String {
    // Check if the string contains single quotes but not double quotes
    if s.contains('\'') && !s.contains('"') {
        // Use double quotes if string contains only single quotes
        format!("\"{}\"", string_replace_common!(s))
    } else {
        // Use single quotes by default, escape any single quotes in the string
        format!("'{}'", string_replace_common!(s.replace('\'', "\\'")))
    }
}

fn repr_sequence(start: char, end: char, items: &[Object], heap: &Heap) -> String {
    let mut s = String::from(start);
    let mut iter = items.iter();
    if let Some(first) = iter.next() {
        s.push_str(&first.repr(heap));
        for item in iter {
            s.push_str(", ");
            s.push_str(&item.repr(heap));
        }
    }
    s.push(end);
    s
}
