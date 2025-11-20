use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt;
use std::rc::Rc;

use crate::exceptions::{exc_err, ExcType, Exception};
use crate::run::RunResult;
use crate::{ParseError, ParseResult};

/// TODO: Move heap-worthy variants (List/Tuple/Str/Bytes/Exceptions/etc.) into the
/// arena (`HeapData`) once the new runtime object model lands. Keeping them inline
/// is a temporary bridge so existing code keeps working during the migration.
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
    List(Rc<RefCell<Vec<Object>>>),
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
            Self::List(v) => format_list_display(v, f),
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

fn format_list_display(list: &Rc<RefCell<Vec<Object>>>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let borrow = list.borrow();
    format_iterable('[', ']', &borrow, f)
}

fn format_list_repr(list: &Rc<RefCell<Vec<Object>>>) -> String {
    let borrow = list.borrow();
    let mut s = String::from("[");
    let mut iter = borrow.iter();
    if let Some(first) = iter.next() {
        s.push_str(&first.repr());
        for item in iter {
            s.push_str(", ");
            s.push_str(&item.repr());
        }
    }
    s.push(']');
    s
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
            (Self::List(s), Self::List(o)) => s.borrow().partial_cmp(&o.borrow()),
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
    #[must_use]
    pub fn add(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Self::Int(v1 + v2)),
            (Self::Str(v1), Self::Str(v2)) => Some(Self::Str(format!("{v1}{v2}"))),
            (Self::List(v1), Self::List(v2)) => {
                let mut data = v1.borrow().clone();
                data.extend(v2.borrow().iter().cloned());
                Some(Self::List(Rc::new(RefCell::new(data))))
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
                v1.borrow_mut().extend(v2.borrow().iter().cloned());
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
    pub fn py_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Undefined, _) => false,
            (_, Self::Undefined) => false,
            (Self::Int(v1), Self::Int(v2)) => v1 == v2,
            (Self::Str(v1), Self::Str(v2)) => v1 == v2,
            (Self::List(v1), Self::List(v2)) => vecs_equal(&v1.borrow(), &v2.borrow()),
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

    #[must_use]
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
            Self::List(v) => !v.borrow().is_empty(),
            Self::Tuple(v) => !v.is_empty(),
            Self::Range(v) => *v != 0,
            Self::Exc(_) => true,
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

    #[allow(clippy::len_without_is_empty)]
    #[must_use]
    pub fn len(&self) -> Option<usize> {
        match self {
            Self::Str(v) => Some(v.len()),
            Self::Bytes(v) => Some(v.len()),
            Self::List(v) => Some(v.borrow().len()),
            Self::Tuple(v) => Some(v.len()),
            _ => None,
        }
    }

    #[must_use]
    pub fn repr(&self) -> String {
        // TODO these need to match python escaping
        match self {
            Self::Str(v) => format!("'{v}'"),
            Self::Bytes(v) => format!("b'{v:?}'"),
            Self::List(v) => format_list_repr(v),
            Self::Exc(exc) => exc.repr(),
            _ => self.to_string(),
        }
    }

    /// TODO maybe replace with TryFrom
    pub fn as_int(&self) -> RunResult<'static, i64> {
        match self {
            Self::Int(i) => Ok(*i),
            // TODO use self.type
            _ => exc_err!(ExcType::TypeError; "'{self:?}' object cannot be interpreted as an integer"),
        }
    }

    // TODO this should be replaced by a proper ObjectType enum
    #[must_use]
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

    pub(crate) fn attr_call<'c, 'd>(
        &mut self,
        attr: &Attr,
        args: Vec<Cow<'d, Self>>,
    ) -> RunResult<'c, Cow<'d, Object>> {
        match (self, attr) {
            (Self::List(v), Attr::Foobar) => Ok(Cow::Owned(Object::Int(v.borrow().len() as i64))),
            (Self::List(v), Attr::Append) => {
                if args.len() == 1 {
                    v.borrow_mut().push(args[0].clone().into_owned());
                    Ok(Cow::Owned(Self::None))
                } else {
                    exc_err!(ExcType::TypeError; "{attr} takes exactly exactly one argument ({} given)", args.len())
                }
            }
            (Self::List(v), Attr::Insert) => {
                if args.len() == 2 {
                    let index = args[0].as_int()? as usize;
                    v.borrow_mut().insert(index, args[1].clone().into_owned());
                    Ok(Cow::Owned(Self::None))
                } else {
                    exc_err!(ExcType::TypeError; "{attr} expected 2 arguments, got {}", args.len())
                }
            }
            (s, _) => exc_err!(ExcType::AttributeError; "'{}' object has no attribute '{attr}'", s.type_str()),
        }
    }
}

fn vecs_equal(v1: &[Object], v2: &[Object]) -> bool {
    if v1.len() == v2.len() {
        for (v1, v2) in v1.iter().zip(v2.iter()) {
            if !v1.py_eq(v2) {
                return false;
            }
        }
        true
    } else {
        false
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
