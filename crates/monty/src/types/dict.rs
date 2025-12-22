use std::fmt::Write;

use ahash::AHashSet;
use indexmap::IndexMap;

use crate::args::ArgValues;
use crate::exceptions::ExcType;
use crate::types::Type;

use super::{List, PyTrait, Tuple};
use crate::heap::{Heap, HeapData, HeapId};
use crate::intern::Interns;
use crate::resource::ResourceTracker;
use crate::run_frame::RunResult;
use crate::value::{Attr, Value};

/// Python dict type, wrapping an IndexMap to preserve insertion order.
///
/// This type provides Python dict semantics including dynamic key-value namespaces,
/// reference counting for heap values, and standard dict methods like get, keys,
/// values, items, and pop.
///
/// # Storage Strategy
/// Uses `IndexMap<u64, Vec<(Value, Value)>>` to preserve insertion order (matching
/// Python 3.7+ behavior). The key is the hash of the dict key. The Vec handles hash
/// collisions by storing multiple (key, value) pairs with the same hash, allowing
/// proper equality checking for collisions.
///
/// # Reference Counting
/// When values are added via `set()`, their reference counts are incremented.
/// When using `from_pairs()`, ownership is transferred without incrementing refcounts
/// (caller must ensure values' refcounts account for the dict's reference).
#[derive(Debug, Default)]
pub struct Dict {
    /// Maps hash -> list of (key, value) pairs with that hash
    /// The Vec handles hash collisions. IndexMap preserves insertion order.
    map: IndexMap<u64, Vec<(Value, Value)>>,
}

impl Dict {
    /// Creates a new empty dict.
    #[must_use]
    pub fn new() -> Self {
        Self { map: IndexMap::new() }
    }

    pub fn as_index_map(&self) -> &IndexMap<u64, Vec<(Value, Value)>> {
        &self.map
    }

    /// Creates a dict from a vector of (key, value) pairs.
    ///
    /// Assumes the caller is transferring ownership of all keys and values in the pairs.
    /// Does NOT increment reference counts since ownership is being transferred.
    /// Returns Err if any key is unhashable (e.g., list, dict).
    pub fn from_pairs(
        pairs: Vec<(Value, Value)>,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Self> {
        let mut dict = Self::new();
        let mut pairs_iter = pairs.into_iter();
        for (key, value) in pairs_iter.by_ref() {
            if let Err(err) = dict.set_transfer_ownership(key, value, heap, interns) {
                for (k, v) in pairs_iter {
                    k.drop_with_heap(heap);
                    v.drop_with_heap(heap);
                }
                dict.drop_all_entries(heap);
                return Err(err);
            }
        }
        Ok(dict)
    }

    /// Internal method to set a key-value pair without incrementing refcounts.
    ///
    /// Used when ownership is being transferred (e.g., from_pairs) rather than shared.
    /// The caller must ensure the values' refcounts already account for this dict's reference.
    fn set_transfer_ownership(
        &mut self,
        key: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<Value>> {
        let Some(hash) = key.py_hash_u64(heap, interns) else {
            // Key is unhashable - clean up before returning error
            let err = ExcType::type_error_unhashable(key.py_type(Some(heap)));
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            return Err(err);
        };

        let bucket = self.map.entry(hash).or_default();

        // Check if key already exists in bucket
        for (i, (k, _v)) in bucket.iter().enumerate() {
            if k.py_eq(&key, heap, interns) {
                // Key exists, replace in place to preserve insertion order
                // Note: we don't decrement old value's refcount since this is a transfer
                // and we don't increment new value's refcount either
                let (_old_key, old_value) = std::mem::replace(&mut bucket[i], (key, value));
                return Ok(Some(old_value));
            }
        }

        // Key doesn't exist, add new pair
        bucket.push((key, value));
        Ok(None)
    }

    fn drop_all_entries(&mut self, heap: &mut Heap<impl ResourceTracker>) {
        for bucket in self.map.values_mut() {
            for (key, value) in bucket.drain(..) {
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
            }
        }
    }

    /// Gets a value from the dict by key.
    ///
    /// Returns Ok(Some(value)) if key exists, Ok(None) if key doesn't exist.
    /// Returns Err if key is unhashable.
    pub fn get(
        &self,
        key: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<&Value>> {
        let hash = key
            .py_hash_u64(heap, interns)
            .ok_or_else(|| ExcType::type_error_unhashable(key.py_type(Some(heap))))?;
        if let Some(bucket) = self.map.get(&hash) {
            for (k, v) in bucket {
                if k.py_eq(key, heap, interns) {
                    return Ok(Some(v));
                }
            }
        }
        Ok(None)
    }

    /// Sets a key-value pair in the dict.
    ///
    /// The caller transfers ownership of `key` and `value` to the dict. Their refcounts
    /// are NOT incremented here - the caller is responsible for ensuring the refcounts
    /// were already incremented (e.g., via `clone_with_heap` or `evaluate_use`).
    ///
    /// If the key already exists, replaces the old value and returns it (caller now
    /// owns the old value and is responsible for its refcount).
    /// Returns Err if key is unhashable.
    pub fn set(
        &mut self,
        key: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<Value>> {
        let hash = key
            .py_hash_u64(heap, interns)
            .ok_or_else(|| ExcType::type_error_unhashable(key.py_type(Some(heap))))?;

        let bucket = self.map.entry(hash).or_default();

        // Check if key already exists in bucket
        for (i, (k, _v)) in bucket.iter().enumerate() {
            if k.py_eq(&key, heap, interns) {
                // Key exists, replace in place to preserve insertion order within the bucket
                let (old_key, old_value) = std::mem::replace(&mut bucket[i], (key, value));

                // Decrement refcount for old key (we're discarding it)
                old_key.drop_with_heap(heap);
                // Transfer ownership of old_value to caller (no clone needed)
                return Ok(Some(old_value));
            }
        }

        // Key doesn't exist, add new pair (ownership transfer)
        bucket.push((key, value));
        Ok(None)
    }

    /// Removes and returns a key-value pair from the dict.
    ///
    /// Returns Ok(Some((key, value))) if key exists, Ok(None) if key doesn't exist.
    /// Returns Err if key is unhashable.
    ///
    /// Reference counting: does not decrement refcounts for removed key and value;
    /// caller assumes ownership and is responsible for managing their refcounts.
    pub fn pop(
        &mut self,
        key: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<(Value, Value)>> {
        let hash = key
            .py_hash_u64(heap, interns)
            .ok_or_else(|| ExcType::type_error_unhashable(key.py_type(Some(heap))))?;

        if let Some(bucket) = self.map.get_mut(&hash) {
            for (i, (k, _v)) in bucket.iter().enumerate() {
                if k.py_eq(key, heap, interns) {
                    let (old_key, old_value) = bucket.swap_remove(i);
                    if bucket.is_empty() {
                        self.map.shift_remove(&hash);
                    }
                    // Don't decrement refcounts - caller now owns the values
                    return Ok(Some((old_key, old_value)));
                }
            }
        }
        Ok(None)
    }

    /// Returns a vector of all keys in the dict with proper reference counting.
    ///
    /// Each key's reference count is incremented since the returned vector
    /// now holds additional references to these values.
    #[must_use]
    pub fn keys(&self, heap: &mut Heap<impl ResourceTracker>) -> Vec<Value> {
        let mut result = Vec::new();
        for bucket in self.map.values() {
            for (k, _v) in bucket {
                result.push(k.clone_with_heap(heap));
            }
        }
        result
    }

    /// Returns a vector of all values in the dict with proper reference counting.
    ///
    /// Each value's reference count is incremented since the returned vector
    /// now holds additional references to these values.
    #[must_use]
    pub fn values_list(&self, heap: &mut Heap<impl ResourceTracker>) -> Vec<Value> {
        let mut result = Vec::new();
        for bucket in self.map.values() {
            for (_k, v) in bucket {
                result.push(v.clone_with_heap(heap));
            }
        }
        result
    }

    /// Returns a vector of all (key, value) pairs in the dict with proper reference counting.
    ///
    /// Each key and value's reference count is incremented since the returned vector
    /// now holds additional references to these values.
    #[must_use]
    pub fn items(&self, heap: &mut Heap<impl ResourceTracker>) -> Vec<(Value, Value)> {
        let mut result = Vec::new();
        for bucket in self.map.values() {
            for (k, v) in bucket {
                result.push((k.clone_with_heap(heap), v.clone_with_heap(heap)));
            }
        }
        result
    }

    /// Returns the number of key-value pairs in the dict.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.values().map(Vec::len).sum()
    }

    /// Returns true if the dict is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns an iterator over references to all (key, value) pairs in the dict.
    ///
    /// The iteration order follows insertion order (IndexMap semantics).
    /// This does NOT increment reference counts - use `copy_for_extend` pattern if needed.
    pub fn iter_pairs(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.map.values().flatten().map(|(k, v)| (k, v))
    }

    /// Creates a deep clone of this dict with proper reference counting.
    ///
    /// All heap-allocated keys and values have their reference counts
    /// incremented. This should be used instead of `.clone()` which would
    /// bypass reference counting.
    #[must_use]
    pub fn clone_with_heap(&self, heap: &mut Heap<impl ResourceTracker>) -> Self {
        let mut new_map = IndexMap::new();
        for (hash, bucket) in &self.map {
            let new_bucket: Vec<(Value, Value)> = bucket
                .iter()
                .map(|(k, v)| (k.clone_with_heap(heap), v.clone_with_heap(heap)))
                .collect();
            new_map.insert(*hash, new_bucket);
        }
        Self { map: new_map }
    }

    /// Consumes the dict and returns a vector of all (key, value) pairs.
    ///
    /// Since ownership is just transferred, there should be no need to increment reference counts etc.
    ///
    /// TODO we should replace this with `into_iter` method to avoid intermediate allocation.
    #[must_use]
    pub fn into_vec(self) -> Vec<(Value, Value)> {
        let mut result = Vec::new();
        for (_, bucket) in self.map {
            for (k, v) in bucket {
                result.push((k, v));
            }
        }
        result
    }

    /// Creates a dict from the `dict()` constructor call.
    ///
    /// - `dict()` with no args returns an empty dict
    /// - `dict(dict)` returns a shallow copy of the dict
    ///
    /// Note: Full Python semantics also support dict(iterable) where iterable
    /// yields (key, value) pairs, and dict(**kwargs) for keyword arguments.
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("dict")?;
        match value {
            None => {
                let heap_id = heap.allocate(HeapData::Dict(Dict::new()))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let Value::Ref(id) = &v else {
                    let err = ExcType::type_error_not_iterable(v.py_type(Some(heap)));
                    v.drop_with_heap(heap);
                    return Err(err);
                };
                let id = *id;

                // Check if it's a dict and get key-value pairs
                let HeapData::Dict(dict) = heap.get(id) else {
                    let err = ExcType::type_error_not_iterable(v.py_type(Some(heap)));
                    v.drop_with_heap(heap);
                    return Err(err);
                };

                // Copy all key-value pairs first (without incrementing refcounts)
                let pairs: Vec<(Value, Value)> = dict
                    .iter_pairs()
                    .map(|(k, v)| (k.copy_for_extend(), v.copy_for_extend()))
                    .collect();

                // Now we can drop the borrow and increment refcounts
                for (k, v) in &pairs {
                    if let Value::Ref(key_id) = k {
                        heap.inc_ref(*key_id);
                    }
                    if let Value::Ref(val_id) = v {
                        heap.inc_ref(*val_id);
                    }
                }
                v.drop_with_heap(heap);

                let new_dict = Dict::from_pairs(pairs, heap, interns)?;
                let result = heap.allocate(HeapData::Dict(new_dict))?;
                Ok(Value::Ref(result))
            }
        }
    }
}

impl PyTrait for Dict {
    fn py_type(&self, _heap: Option<&Heap<impl ResourceTracker>>) -> Type {
        Type::Dict
    }

    fn py_estimate_size(&self) -> usize {
        // Dict size: struct overhead + entries (2 Values per entry for key+value)
        std::mem::size_of::<Self>() + self.len() * 2 * std::mem::size_of::<Value>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.len())
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        if self.len() != other.len() {
            return false;
        }

        // Check that all keys in self exist in other with equal values
        for bucket in self.map.values() {
            for (k, v) in bucket {
                match other.get(k, heap, interns) {
                    Ok(Some(other_v)) => {
                        if !v.py_eq(other_v, heap, interns) {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
        }
        true
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for bucket in self.map.values_mut() {
            for (key, value) in bucket {
                if let Value::Ref(id) = key {
                    stack.push(*id);
                    #[cfg(feature = "dec-ref-check")]
                    key.dec_ref_forget();
                }
                if let Value::Ref(id) = value {
                    stack.push(*id);
                    #[cfg(feature = "dec-ref-check")]
                    value.dec_ref_forget();
                }
            }
        }
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        if self.is_empty() {
            return f.write_str("{}");
        }

        f.write_char('{')?;
        let mut first = true;
        for bucket in self.map.values() {
            for (k, v) in bucket {
                if !first {
                    f.write_str(", ")?;
                }
                first = false;
                k.py_repr_fmt(f, heap, heap_ids, interns)?;
                f.write_str(": ")?;
                v.py_repr_fmt(f, heap, heap_ids, interns)?;
            }
        }
        f.write_char('}')
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
        // Use copy_for_extend to avoid borrow conflict, then increment refcount
        let result = self.get(key, heap, interns)?.map(Value::copy_for_extend);
        match result {
            Some(value) => {
                if let Value::Ref(id) = &value {
                    heap.inc_ref(*id);
                }
                Ok(value)
            }
            None => Err(ExcType::key_error(key, heap, interns)),
        }
    }

    fn py_setitem(
        &mut self,
        key: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<()> {
        // Drop the old value if one was replaced
        if let Some(old_value) = self.set(key, value, heap, interns)? {
            old_value.drop_with_heap(heap);
        }
        Ok(())
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &Attr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        match attr {
            #[allow(clippy::manual_let_else)]
            Attr::Get => {
                // dict.get() accepts 1 or 2 arguments
                let (key, default) = args.get_one_two_args("get")?;
                let default = default.unwrap_or(Value::None);
                // Handle the lookup - may fail for unhashable keys
                let result = match self.get(&key, heap, interns) {
                    Ok(r) => r,
                    Err(e) => {
                        // Drop key and default before returning error
                        key.drop_with_heap(heap);
                        default.drop_with_heap(heap);
                        return Err(e);
                    }
                };
                let value = match result {
                    Some(v) => v.clone_with_heap(heap),
                    None => default.clone_with_heap(heap),
                };
                // Drop the key and default arguments
                key.drop_with_heap(heap);
                default.drop_with_heap(heap);
                Ok(value)
            }
            Attr::Keys => {
                args.check_zero_args("dict.keys")?;
                let keys = self.keys(heap);
                let list_id = heap.allocate(HeapData::List(List::new(keys)))?;
                Ok(Value::Ref(list_id))
            }
            Attr::Values => {
                args.check_zero_args("dict.values")?;
                let values = self.values_list(heap);
                let list_id = heap.allocate(HeapData::List(List::new(values)))?;
                Ok(Value::Ref(list_id))
            }
            Attr::Items => {
                args.check_zero_args("dict.items")?;
                // Return list of tuples
                let items = self.items(heap);
                let mut tuples: Vec<Value> = Vec::with_capacity(items.len());
                for (k, v) in items {
                    let tuple_id = heap.allocate(HeapData::Tuple(Tuple::new(vec![k, v])))?;
                    tuples.push(Value::Ref(tuple_id));
                }
                let list_id = heap.allocate(HeapData::List(List::new(tuples)))?;
                Ok(Value::Ref(list_id))
            }
            #[allow(clippy::manual_let_else, clippy::single_match_else)]
            Attr::Pop => {
                // dict.pop() accepts 1 or 2 arguments (key, optional default)
                let (key, default) = args.get_one_two_args("pop")?;
                let result = match self.pop(&key, heap, interns) {
                    Ok(r) => r,
                    Err(e) => {
                        // Clean up key and default before returning error
                        key.drop_with_heap(heap);
                        if let Some(d) = default {
                            d.drop_with_heap(heap);
                        }
                        return Err(e);
                    }
                };
                if let Some((old_key, value)) = result {
                    // Drop the old key - we don't need it
                    old_key.drop_with_heap(heap);
                    // Drop the lookup key and default arguments
                    key.drop_with_heap(heap);
                    if let Some(d) = default {
                        d.drop_with_heap(heap);
                    }
                    Ok(value)
                } else {
                    // No matching key - return default if provided, else KeyError
                    if let Some(d) = default {
                        key.drop_with_heap(heap);
                        Ok(d)
                    } else {
                        let err = ExcType::key_error(&key, heap, interns);
                        key.drop_with_heap(heap);
                        Err(err)
                    }
                }
            }
            _ => Err(ExcType::attribute_error(Type::Dict, attr)),
        }
    }
}
