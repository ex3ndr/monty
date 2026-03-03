use std::fmt::Write;

use ahash::AHashSet;

use super::{Dict, PyTrait};
use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId, HeapRead, HeapReadOutput, HeapReader},
    intern::Interns,
    resource::{ResourceError, ResourceTracker},
    types::{AttrCallResult, Type},
    value::{EitherStr, Value},
};

/// Python dataclass instance type.
///
/// Represents an instance of a dataclass with a class name, field values, and
/// frozen/mutable semantics. Method calls on dataclasses are detected lazily:
/// when `call_attr` is invoked on a dataclass and the attribute name is not found
/// in `attrs`, it is dispatched as a `MethodCall` to the host (provided the name
/// is public — no leading underscore).
///
/// # Fields
/// - `name`: The class name (e.g., "Point", "User")
/// - `field_names`: Declared field names in definition order (used for repr)
/// - `attrs`: All attributes including declared fields and dynamically added ones
/// - `frozen`: Whether the dataclass instance is immutable
///
/// # Hashability
/// When `frozen` is true, the dataclass is immutable and hashable. The hash
/// is computed from the class name and declared field values only.
/// When `frozen` is false, the dataclass is mutable and unhashable.
///
/// # Reference Counting
/// The `attrs` Dict contains Values that may be heap-allocated. The
/// `py_dec_ref_ids` method properly handles decrementing refcounts for
/// all attribute values when the dataclass instance is freed.
///
/// # Attribute Access
/// - Getting: Looks up the attribute name in the attrs Dict
/// - Setting: Updates or adds the attribute in attrs (only if not frozen)
/// - Method calls: If the attribute is a public name not found in attrs, dispatched to host
/// - repr: Only shows declared fields (from field_names), not extra attributes
#[derive(Debug)]
pub(crate) struct Dataclass {
    /// The class name (e.g., "Point", "User")
    name: EitherStr,
    /// Identifier of the type, from `id(type(dc))` in python.
    type_id: u64,
    /// Declared field names in definition order (for repr and hashing)
    field_names: Vec<String>,
    /// All attributes (both declared fields and dynamically added)
    /// Dict wrapped as value for heap management
    attrs: HeapId,
    /// Whether this dataclass instance is immutable (affects hashability)
    frozen: bool,
}

impl Dataclass {
    /// Creates a new dataclass instance.
    ///
    /// # Arguments
    /// * `name` - The class name
    /// * `type_id` - The type ID of the dataclass
    /// * `field_names` - Declared field names in definition order
    /// * `attrs` - Dict of attribute name -> value pairs (ownership transferred)
    /// * `frozen` - Whether this dataclass instance is immutable (affects hashability)
    pub fn new(
        name: impl Into<EitherStr>,
        type_id: u64,
        field_names: Vec<String>,
        attrs: Dict,
        frozen: bool,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Self, ResourceError> {
        Ok(Self {
            name: name.into(),
            type_id,
            field_names,
            attrs: heap.allocate(HeapData::Dict(attrs))?,
            frozen,
        })
    }

    /// Returns the class name.
    #[must_use]
    pub fn name<'a>(&'a self, interns: &'a Interns) -> &'a str {
        self.name.as_str(interns)
    }

    /// Returns the type ID of the dataclass.
    #[must_use]
    pub fn type_id(&self) -> u64 {
        self.type_id
    }

    /// Returns a reference to the declared field names.
    #[must_use]
    pub fn field_names(&self) -> &[String] {
        &self.field_names
    }

    /// Returns whether this dataclass contains any heap references (`Value::Ref`).
    #[inline]
    #[expect(clippy::unused_self)]
    #[must_use]
    pub fn has_refs(&self) -> bool {
        // contains a dict
        true
    }

    /// Returns whether this dataclass instance is frozen (immutable).
    #[must_use]
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Sets an attribute value.
    ///
    /// The caller transfers ownership of both `name` and `value`. Returns the
    /// old value if the attribute existed (caller must drop it), or None if this
    /// is a new attribute.
    ///
    /// Returns `FrozenInstanceError` if the dataclass is frozen.
    pub fn set_attr<'a>(
        this: &mut HeapRead<'a, Self>,
        name: Value,
        value: Value,
        reader: &mut HeapReader<'a, Heap<impl ResourceTracker>>,
        interns: &Interns,
    ) -> RunResult<Option<Value>> {
        if this.get(reader).frozen {
            // Get attribute name for error message
            let attr_name = match &name {
                Value::InternString(id) => interns.get_str(*id).to_string(),
                _ => "<unknown>".to_string(),
            };
            // Drop the values we were given ownership of
            name.drop_with_heap(reader.heap);
            value.drop_with_heap(reader.heap);
            return Err(ExcType::frozen_instance_error(&attr_name));
        }
        Dict::set_via_reader(&mut Self::attrs_reader(this, reader), name, value, reader, interns)
    }

    /// Computes the hash for this dataclass if it's frozen.
    ///
    /// Returns `Ok(Some(hash))` for frozen (immutable) dataclasses, `Ok(None)` for mutable ones.
    /// Returns `Err(ResourceError::Recursion)` if the recursion limit is exceeded.
    /// The hash is computed from the class name and declared field values only.
    pub fn py_hash<'a>(
        this: &HeapRead<'a, Self>,
        reader: &mut HeapReader<'a, Heap<impl ResourceTracker>>,
        interns: &Interns,
    ) -> Result<Option<u64>, ResourceError> {
        use std::{
            collections::hash_map::DefaultHasher,
            hash::{Hash, Hasher},
        };

        // Only frozen (immutable) dataclasses are hashable
        if !this.get(reader).frozen {
            return Ok(None);
        }

        let token = reader.heap.incr_recursion_depth()?;
        defer_drop!(token, reader);
        let mut hasher = DefaultHasher::new();
        // Hash the class name
        this.get(reader).name.hash(&mut hasher);
        // Hash each declared field (name, value) pair in order
        let field_count = this.get(reader).field_names.len();

        let attrs = Self::attrs_reader(this, reader);

        for i in 0..field_count {
            let field_name = &this.get(reader).field_names[i];
            field_name.hash(&mut hasher);
            let Some(value) = attrs
                .get(reader)
                .get_by_str(field_name, reader.heap, interns)
                .map(|v| v.clone_with_heap(reader.heap))
            else {
                continue; // Missing field value - TODO should this be an error?
            };
            defer_drop!(value, reader);
            match value.py_hash(reader.heap, interns)? {
                Some(h) => h.hash(&mut hasher),
                None => return Ok(None),
            }
        }
        Ok(Some(hasher.finish()))
    }

    pub fn attrs_dict(&self) -> HeapId {
        self.attrs
    }

    pub fn traverse(&self, work_list: &mut Vec<HeapId>) {
        work_list.push(self.attrs);
    }

    fn attrs_reader<'a>(
        this: &HeapRead<'a, Self>,
        reader: &mut HeapReader<'a, Heap<impl ResourceTracker>>,
    ) -> HeapRead<'a, Dict> {
        let HeapReadOutput::Dict(attrs) = reader.read(this.get(reader).attrs) else {
            panic!("Dataclass attrs is not a Dict");
        };
        attrs
    }
}

impl PyTrait for Dataclass {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Dataclass
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.name.py_estimate_size()
            + self.field_names.iter().map(String::len).sum::<usize>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        // Dataclasses don't have a length
        None
    }

    fn py_eq<'a>(
        this: &HeapRead<'a, Self>,
        other: &HeapRead<'a, Self>,
        reader: &mut HeapReader<'a, Heap<impl ResourceTracker>>,
        interns: &Interns,
    ) -> Result<bool, ResourceError> {
        // Dataclasses are equal if they have the same name and equal attrs
        let self_attrs = Self::attrs_reader(this, reader);
        let other_attrs = Self::attrs_reader(other, reader);
        Ok(this.get(reader).name == other.get(reader).name && Dict::py_eq(&self_attrs, &other_attrs, reader, interns)?)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.traverse(stack);
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        // Dataclass instances are always truthy (like Python objects)
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        // Check depth limit before recursing
        let Some(token) = heap.incr_recursion_depth_for_repr() else {
            return f.write_str("...");
        };
        crate::defer_drop_immutable_heap!(token, heap);

        // Format: ClassName(field1=value1, field2=value2, ...)
        // Only declared fields are shown, not dynamically added attributes
        f.write_str(self.name(interns))?;
        f.write_char('(')?;

        let mut first = true;
        for field_name in &self.field_names {
            if !first {
                f.write_str(", ")?;
            }
            first = false;

            // Write field name
            f.write_str(field_name)?;
            f.write_char('=')?;

            let HeapData::Dict(attrs) = heap.get(self.attrs) else {
                panic!("Dataclass attrs is not a Dict");
            };

            // Look up value in attrs
            if let Some(value) = attrs.get_by_str(field_name, heap, interns) {
                value.py_repr_fmt(f, heap, heap_ids, interns)?;
            } else {
                // Field not found - shouldn't happen for well-formed dataclasses
                f.write_str("<?>")?;
            }
        }

        f.write_char(')')?;
        Ok(())
    }

    /// Performs lazy method detection for dataclass instances.
    ///
    /// If the attribute is a public name (no leading underscore) not found in the
    /// dataclass's attrs dict, returns `MethodCall` so the VM yields to the host.
    /// Otherwise handles the call directly:
    /// - Attributes that exist in attrs but aren't callable produce `TypeError`
    /// - Private/dunder attributes that aren't in attrs produce `AttributeError`
    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<AttrCallResult> {
        let heap = &mut *vm.heap;
        let interns = vm.interns;
        let attr_str = attr.as_str(interns);
        // Only public methods (no underscore prefix = no dunders, no private)
        let is_public_method = !attr_str.starts_with('_') && {
            let HeapData::Dict(attrs) = heap.get(self.attrs) else {
                panic!("Dataclass attrs is not a Dict");
            };
            attrs.get_by_str(attr_str, heap, interns).is_none()
        };
        if is_public_method {
            // Clone self and prepend to args for the method call
            // inc_ref works even when data is taken out (refcount metadata is separate)
            heap.inc_ref(self_id);
            let self_arg = Value::Ref(self_id);
            let args_with_self = args.prepend(self_arg);
            Ok(AttrCallResult::MethodCall(attr.clone(), args_with_self))
        } else {
            // Not a method call — handle directly
            let method_name = attr.as_str(interns);
            defer_drop!(args, heap);

            let HeapData::Dict(attrs) = heap.get(self.attrs) else {
                panic!("Dataclass attrs is not a Dict");
            };
            // If the attribute exists in attrs, it's a data value (not callable)
            if let Some(value) = attrs.get_by_str(method_name, heap, interns) {
                let type_name = value.py_type(heap);
                Err(ExcType::type_error_not_callable_object(type_name))
            } else {
                // Attribute doesn't exist — use the class name (e.g., "Point") not "Dataclass"
                Err(ExcType::attribute_error(self.name(interns), method_name))
            }
        }
    }

    fn py_getattr<'a>(
        this: &HeapRead<'a, Self>,
        attr: &EitherStr,
        reader: &mut HeapReader<'a, Heap<impl ResourceTracker>>,
        interns: &Interns,
    ) -> RunResult<Option<AttrCallResult>> {
        let attr_name = attr.as_str(interns);
        let attrs = Self::attrs_reader(this, reader);
        match attrs.get(reader).get_by_str(attr_name, reader.heap, interns) {
            Some(value) => Ok(Some(AttrCallResult::Value(value.clone_with_heap(reader.heap)))),
            // we use name here, not `self.py_type(heap)` hence returning a Ok(None)
            None => Err(ExcType::attribute_error(this.get(reader).name(interns), attr_name)),
        }
    }
}

// Custom serde implementation for Dataclass.
// Serializes all five fields.
impl serde::Serialize for Dataclass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Dataclass", 5)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("type_id", &self.type_id)?;
        state.serialize_field("field_names", &self.field_names)?;
        state.serialize_field("attrs", &self.attrs)?;
        state.serialize_field("frozen", &self.frozen)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for Dataclass {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct DataclassData {
            name: EitherStr,
            type_id: u64,
            field_names: Vec<String>,
            attrs: HeapId,
            frozen: bool,
        }
        let dc = DataclassData::deserialize(deserializer)?;
        Ok(Self {
            name: dc.name,
            type_id: dc.type_id,
            field_names: dc.field_names,
            attrs: dc.attrs,
            frozen: dc.frozen,
        })
    }
}
