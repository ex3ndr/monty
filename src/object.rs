use std::cmp::Ordering;
use std::fmt;

use crate::exceptions::{exc_err, Exception};
use crate::run::RunResult;

#[derive(Debug, Clone, PartialEq)]
pub enum Object {
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
    Exc(Exception),
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
            Self::Exc(exc) => write!(f, "0:{exc}"),
        }
    }
}

fn format_iterable(start: char, end: char, items: &[Object], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{start}")?;
    let mut items_iter = items.iter();
    if let Some(first) = items_iter.next() {
        write!(f, "{first}")?;
        for item in items_iter {
            write!(f, ", {item}")?;
        }
    }
    write!(f, "{end}")
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
            (Self::Str(v1), Self::Str(v2)) => Some(Self::Str(format!("{v1}{v2}"))),
            (Self::List(v1), Self::List(v2)) => {
                let mut v = v1.clone();
                v.extend(v2.clone());
                Some(Self::List(v))
            }
            _ => None,
        }
    }

    pub fn add_mut(&mut self, other: Self) -> Result<(), Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => {
                *v1 += v2;
            }
            (Self::Str(v1), Self::Str(v2)) => {
                v1.push_str(&v2);
            }
            (Self::List(v1), Self::List(v2)) => {
                v1.extend(v2);
            }
            (_, other) => return Err(other),
        }
        Ok(())
    }

    pub fn sub(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Self::Int(v1 - v2)),
            _ => None,
        }
    }

    /// different name to avoid confusion with `PartialEq::eq`
    pub fn py_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Undefined, _) => false,
            (_, Self::Undefined) => false,
            (Self::Int(v1), Self::Int(v2)) => v1 == v2,
            (Self::Str(v1), Self::Str(v2)) => v1 == v2,
            (Self::List(v1), Self::List(v2)) => vecs_equal(v1, v2),
            (Self::Tuple(v1), Self::Tuple(v2)) => vecs_equal(v1, v2),
            (Self::Range(v1), Self::Range(v2)) => v1 == v2,
            (Self::True, Self::True) => true,
            (Self::True, Self::Int(v2)) => 1 == *v2,
            (Self::Int(v1), Self::True) => *v1 == 1,
            (Self::False, Self::False) => true,
            (Self::False, Self::Int(v2)) => 0 == *v2,
            (Self::Int(v1), Self::False) => *v1 == 0,
            (Self::None, Self::None) => true,
            _ => false,
        }
    }

    pub fn bool(&self) -> bool {
        match self {
            Self::Undefined => false,
            Self::Ellipsis => true,
            Self::None => false,
            Self::True => true,
            Self::False => false,
            Self::Int(v) => *v != 0,
            Self::Float(f) => *f != 0.0,
            Self::Str(v) => !v.is_empty(),
            Self::Bytes(v) => !v.is_empty(),
            Self::List(v) => !v.is_empty(),
            Self::Tuple(v) => !v.is_empty(),
            Self::Range(v) => *v != 0,
            Self::Exc(_) => true,
        }
    }

    pub fn modulus(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Self::Int(v1 % v2)),
            (Self::Float(v1), Self::Float(v2)) => Some(Self::Float(v1 % v2)),
            (Self::Float(v1), Self::Int(v2)) => Some(Self::Float(v1 % (*v2 as f64))),
            (Self::Int(v1), Self::Float(v2)) => Some(Self::Float((*v1 as f64) % v2)),
            _ => None,
        }
    }

    pub fn modulus_eq(&self, other: &Self, right_value: i64) -> Option<bool> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(v1 % v2 == right_value),
            (Self::Float(v1), Self::Float(v2)) => Some(v1 % v2 == right_value as f64),
            (Self::Float(v1), Self::Int(v2)) => Some(v1 % (*v2 as f64) == right_value as f64),
            (Self::Int(v1), Self::Float(v2)) => Some((*v1 as f64) % v2 == right_value as f64),
            _ => None,
        }
    }

    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> Option<usize> {
        match self {
            Self::Str(v) => Some(v.len()),
            Self::Bytes(v) => Some(v.len()),
            Self::List(v) => Some(v.len()),
            Self::Tuple(v) => Some(v.len()),
            _ => None,
        }
    }

    pub fn repr(&self) -> String {
        // TODO these need to match python escaping
        match self {
            Self::Str(v) => format!("\"{v}\""),
            Self::Bytes(v) => format!("b\"{v:?}\""),
            _ => self.to_string(),
        }
    }

    pub fn as_int(&self) -> RunResult<'static, i64> {
        match self {
            Self::Int(i) => Ok(*i),
            // TODO use self.type
            _ => exc_err!(Exception::TypeError; "'{self:?}' object cannot be interpreted as an integer"),
        }
    }

    // TODO this should be replaced by a proper ObjectType enum
    pub fn type_str(&self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Ellipsis => "ellipsis",
            Self::None => "NoneType",
            Self::True => "bool",
            Self::False => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "str",
            Self::Bytes(_) => "bytes",
            Self::List(_) => "list",
            Self::Tuple(_) => "tuple",
            Self::Range(_) => "range",
            Self::Exc(e) => e.type_str(),
        }
    }
}

fn vecs_equal(v1: &[Object], v2: &[Object]) -> bool {
    if v1.len() != v2.len() {
        false
    } else {
        for (v1, v2) in v1.iter().zip(v2.iter()) {
            if !v1.py_eq(v2) {
                return false;
            }
        }
        true
    }
}
