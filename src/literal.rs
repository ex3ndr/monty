use crate::heap::{Heap, HeapData};
use crate::object::Object;

/// Represents values that can be produced purely from the parser/prepare pipeline.
///
/// Literal values are intentionally detached from the runtime heap so we can keep
/// parse-time transformations (constant folding, namespace seeding, etc.) free from
/// reference-count semantics.  Only once execution begins are these literals turned
/// into real `Object`s that participate in the interpreter's runtime rules.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Undefined,
    Ellipsis,
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Tuple(Vec<Literal>),
}

impl Literal {
    /// Converts the literal into its runtime `Object` counterpart.
    ///
    /// This is the only place parse-time data crosses the boundary into runtime
    /// semantics, ensuring every literal follows the same conversion path (helpful
    /// for keeping later heap/refcount logic centralized).
    ///
    /// Heap-allocated types (Str, Bytes, Tuple) will be allocated on the heap and
    /// returned as `Object::Ref` variants. Immediate values are returned inline.
    pub fn to_object(&self, heap: &mut Heap) -> Object {
        match self {
            Self::Undefined => Object::Undefined,
            Self::Ellipsis => Object::Ellipsis,
            Self::None => Object::None,
            Self::Bool(true) => Object::True,
            Self::Bool(false) => Object::False,
            Self::Int(v) => Object::Int(*v),
            Self::Float(v) => Object::Float(*v),
            Self::Str(s) => {
                let id = heap.allocate(HeapData::Str(s.clone()));
                Object::Ref(id)
            }
            Self::Bytes(b) => {
                let id = heap.allocate(HeapData::Bytes(b.clone()));
                Object::Ref(id)
            }
            Self::Tuple(items) => {
                // Convert all tuple items to runtime objects
                let converted: Vec<Object> = items.iter().map(|lit| lit.to_object(heap)).collect();
                // Allocate tuple on heap
                let id = heap.allocate(HeapData::Tuple(converted));
                Object::Ref(id)
            }
        }
    }

    /// Returns a Python-esque string representation for logging/debugging.
    ///
    /// This avoids the need to import runtime formatting helpers into parser code
    /// while still giving enough fidelity to display constants in errors/traces.
    pub fn repr(&self) -> String {
        match self {
            Self::Undefined => "Undefined".to_string(),
            Self::Ellipsis => "...".to_string(),
            Self::None => "None".to_string(),
            Self::Bool(true) => "True".to_string(),
            Self::Bool(false) => "False".to_string(),
            Self::Int(v) => v.to_string(),
            Self::Float(v) => v.to_string(),
            Self::Str(v) => format!("'{v}'"),
            Self::Bytes(v) => format!("b'{v:?}'"),
            Self::Tuple(items) => format_literal_iterable('(', ')', items),
        }
    }
}

/// Utility used by [`Literal::repr`] to format tuple-like literals in a compact form.
///
/// Lives outside the impl to emphasize that it has no runtime dependencies and
/// purely manipulates literal data.
fn format_literal_iterable(start: char, end: char, items: &[Literal]) -> String {
    let mut s = String::new();
    s.push(start);
    let mut iter = items.iter();
    if let Some(first) = iter.next() {
        s.push_str(&first.repr());
        for item in iter {
            s.push_str(", ");
            s.push_str(&item.repr());
        }
    }
    s.push(end);
    s
}
