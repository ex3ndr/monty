use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Object {
    Undefined,
    Ellipsis,
    None,
    True,
    False,
    Int(i64),
    Bytes(Vec<u8>),
    Float(f64),
    Str(String),
    List(Vec<Object>),
    Tuple(Vec<Object>),
    Range(i64),
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undefined => write!(f, "Undefined"),
            Self::Ellipsis => write!(f, "..."),
            Self::None => write!(f, "None"),
            Self::True => write!(f, "True"),
            Self::False => write!(f, "False"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Str(v) => write!(f, "{v}"),
            Self::Bytes(v) => write!(f, "{v:?}"), // TODO: format bytes
            Self::List(v) => format_iterable('[', ']', v, f),
            Self::Tuple(v) => format_iterable('(', ')', v, f),
            Self::Range(size) => write!(f, "0:{size}"),
        }
    }
}

fn format_iterable(start: char, end: char, items: &[Object], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}", start)?;
    let mut items_iter = items.iter();
    if let Some(first) = items_iter.next() {
        write!(f, "{first}")?;
        for item in items_iter {
            write!(f, ", {item}")?;
        }
    }
    write!(f, "{}", end)
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
            (Self::Str(s), Self::Str(o)) => s.partial_cmp(o),
            (Self::Bytes(s), Self::Bytes(o)) => s.partial_cmp(o),
            (Self::List(s), Self::List(o)) => s.partial_cmp(o),
            (Self::Tuple(s), Self::Tuple(o)) => s.partial_cmp(o),
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
    pub fn add(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Self::Int(v1 + v2)),
            (Self::Str(v1), Self::Str(v2)) => {
                Some(Self::Str(format!("{}{}", v1, v2)))
            }
            (Self::List(v1), Self::List(v2)) => {
                let mut v = v1.clone();
                v.extend(v2.clone());
                Some(Self::List(v))
            }
            _ => None,
        }
    }

    pub fn add_mut(&mut self, other: Self) -> bool {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => {
                *v1 += v2;
            },
            (Self::Str(v1), Self::Str(v2)) => {
                v1.push_str(&v2);
            }
            (Self::List(v1), Self::List(v2)) => {
                v1.extend(v2);
            }
            _ => return false,
        }
        true
    }

    pub fn sub(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Self::Int(v1 - v2)),
            _ => None,
        }
    }

    pub fn eq(&self, other: &Self) -> Option<bool> {
        match (self, other) {
            (Self::Undefined, _) => None,
            (_, Self::Undefined) => None,
            (Self::Int(v1), Self::Int(v2)) => Some(v1 == v2),
            (Self::Str(v1), Self::Str(v2)) => Some(v1 == v2),
            (Self::List(v1), Self::List(v2)) => vecs_equal(v1, v2),
            (Self::Tuple(v1), Self::Tuple(v2)) => vecs_equal(v1, v2),
            (Self::Range(v1), Self::Range(v2)) => Some(v1 == v2),
            (Self::True, Self::True) => Some(true),
            (Self::True, Self::Int(v2)) => Some(1 == *v2),
            (Self::Int(v1), Self::True) => Some(*v1 == 1),
            (Self::False, Self::False) => Some(true),
            (Self::False, Self::Int(v2)) => Some(0 == *v2),
            (Self::Int(v1), Self::False) => Some(*v1 == 0),
            (Self::None, Self::None) => Some(true),
            _ => Some(false),
        }
    }

    pub fn bool(&self) -> Option<bool> {
        match self {
            Self::Undefined => None,
            Self::Ellipsis => Some(true),
            Self::None => Some(false),
            Self::True => Some(true),
            Self::False => Some(false),
            Self::Int(v) => Some(*v != 0),
            Self::Float(f) => Some(*f != 0.0),
            Self::Str(v) => Some(!v.is_empty()),
            Self::Bytes(v) => Some(!v.is_empty()),
            Self::List(v) => Some(!v.is_empty()),
            Self::Tuple(v) => Some(!v.is_empty()),
            Self::Range(v) => Some(*v != 0),
        }
    }

    pub fn modulo(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Self::Int(v1 % v2)),
            (Self::Float(v1), Self::Float(v2)) => Some(Self::Float(v1 % v2)),
            (Self::Float(v1), Self::Int(v2)) => Some(Self::Float(v1 % (*v2 as f64))),
            (Self::Int(v1), Self::Float(v2)) => Some(Self::Float((*v1 as f64) % v2)),
            _ => None,
        }
    }

    pub fn len(&self) -> Option<usize> {
        match self {
            Self::Str(v) => Some(v.len()),
            Self::Bytes(v) => Some(v.len()),
            Self::List(v) => Some(v.len()),
            Self::Tuple(v) => Some(v.len()),
            _ => None,
        }
    }
}

fn vecs_equal(v1: &[Object], v2: &[Object]) -> Option<bool> {
    if v1.len() != v2.len() {
        Some(false)
    } else {
        for (v1, v2) in v1.into_iter().zip(v2.into_iter()) {
            if let Some(v) = v1.eq(v2) {
                if !v {
                    return Some(false);
                }
            } else {
                return None;
            }
        }
        Some(true)
    }
}
