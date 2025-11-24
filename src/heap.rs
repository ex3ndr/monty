use crate::object::Object;

/// Unique identifier for objects stored inside the heap arena.
pub type ObjectId = usize;

/// HeapData captures every runtime object that must live in the arena.
#[derive(Debug, Clone)]
pub enum HeapData {
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<Object>),
    Tuple(Vec<Object>),
    // TODO: support arbitrary classes
}

impl HeapData {
    /// Debug representation of the data type
    #[must_use]
    pub fn type_str(&self) -> &'static str {
        match self {
            Self::Str(_) => "str",
            Self::Bytes(_) => "bytes",
            Self::List(_) => "list",
            Self::Tuple(_) => "tuple",
        }
    }

    /// different name to avoid confusion with `PartialEq::eq`
    #[must_use]
    pub fn py_eq(&self, other: &Self, heap: &Heap) -> bool {
        match (self, other) {
            (Self::Str(s1), Self::Str(s2)) => s1 == s2,
            (Self::Bytes(b1), Self::Bytes(b2)) => b1 == b2,
            (Self::List(elements1), Self::List(elements2)) => {
                elements1.len() == elements2.len() && elements1.iter().zip(elements2).all(|(i1, i2)| i1.py_eq(i2, heap))
            }
            (Self::Tuple(elements1), Self::Tuple(elements2)) => {
                elements1.len() == elements2.len() && elements1.iter().zip(elements2).all(|(i1, i2)| i1.py_eq(i2, heap))
            }
            _ => false,
        }
    }
}

/// A single entry inside the heap arena, storing refcount and payload.
#[derive(Debug)]
struct HeapObject {
    refcount: usize,
    data: HeapData,
}

/// Reference-counted arena that backs all heap-only runtime objects.
///
/// The heap never reuses IDs during a single execution; instead it appends new
/// entries and relies on `clear()` between runs.  This keeps identity checks
/// simple and avoids the need for generation counters while we're still
/// building out semantics.
#[derive(Debug, Default)]
pub struct Heap {
    objects: Vec<Option<HeapObject>>,
}

impl Heap {
    /// Allocates a new heap object, returning the fresh identifier.
    pub fn allocate(&mut self, data: HeapData) -> ObjectId {
        let id = self.objects.len();
        self.objects.push(Some(HeapObject { refcount: 1, data }));
        id
    }

    /// Increments the reference count for an existing heap object.
    ///
    /// # Panics
    /// Panics if the object ID is invalid or the object has already been freed.
    pub fn inc_ref(&mut self, id: ObjectId) {
        let object = self
            .objects
            .get_mut(id)
            .expect("Heap::inc_ref: slot missing")
            .as_mut()
            .expect("Heap::inc_ref: object already freed");
        object.refcount += 1;
    }

    /// Decrements the reference count and frees the object (plus children) once it hits zero.
    ///
    /// # Panics
    /// Panics if the object ID is invalid or the object has already been freed.
    pub fn dec_ref(&mut self, id: ObjectId) {
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let slot = self.objects.get_mut(current).expect("Heap::dec_ref: slot missing");
            let entry = slot.as_mut().expect("Heap::dec_ref: object already freed");
            if entry.refcount > 1 {
                entry.refcount -= 1;
                continue;
            }

            if let Some(object) = slot.take() {
                enqueue_children(&object.data, &mut stack);
            }
        }
    }

    /// Returns an immutable reference to the heap data stored at the given ID.
    ///
    /// # Panics
    /// Panics if the object ID is invalid or the object has already been freed.
    #[must_use]
    pub fn get(&self, id: ObjectId) -> &HeapData {
        &self
            .objects
            .get(id)
            .expect("Heap::get: slot missing")
            .as_ref()
            .expect("Heap::get: object already freed")
            .data
    }

    /// Returns a mutable reference to the heap data stored at the given ID.
    ///
    /// # Panics
    /// Panics if the object ID is invalid or the object has already been freed.
    pub fn get_mut(&mut self, id: ObjectId) -> &mut HeapData {
        &mut self
            .objects
            .get_mut(id)
            .expect("Heap::get_mut: slot missing")
            .as_mut()
            .expect("Heap::get_mut: object already freed")
            .data
    }

    /// Removes all objects and resets the ID counter, used between executor runs.
    pub fn clear(&mut self) {
        self.objects.clear();
    }
}

/// Pushes any child object IDs referenced by `data` onto the provided stack so
/// `dec_ref` can recursively drop entire object graphs without recursion.
fn enqueue_children(data: &HeapData, stack: &mut Vec<ObjectId>) {
    match data {
        HeapData::List(items) | HeapData::Tuple(items) => {
            // Walk through all items and enqueue any heap-allocated objects
            for obj in items {
                if let Object::Ref(id) = obj {
                    stack.push(*id);
                }
            }
        }
        HeapData::Str(_) | HeapData::Bytes(_) => {
            // Strings and bytes don't contain nested objects
        }
    }
}
