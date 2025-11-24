use std::fmt;

use crate::exceptions::{exc_err_fmt, internal_err, ExcType, InternalRunError, SimpleException};
use crate::heap::Heap;
use crate::parse_error::{ParseError, ParseResult};
use crate::run::RunResult;
use crate::{HeapData, Object};

// TODO use strum
#[derive(Debug, Clone)]
pub(crate) enum FunctionTypes {
    Print,
    Len,
    Str,
    Repr,
}

#[derive(Debug, Clone)]
pub(crate) enum Types {
    BuiltinFunction(FunctionTypes),
    Exceptions(ExcType),
    Range,
}

impl fmt::Display for Types {
    // TODO replace with a strum
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuiltinFunction(FunctionTypes::Print) => write!(f, "print"),
            Self::BuiltinFunction(FunctionTypes::Len) => write!(f, "len"),
            Self::BuiltinFunction(FunctionTypes::Str) => write!(f, "str"),
            Self::BuiltinFunction(FunctionTypes::Repr) => write!(f, "repr"),
            Self::Exceptions(exc) => write!(f, "{exc}"),
            Self::Range => write!(f, "range"),
        }
    }
}

impl Types {
    // TODO replace with a strum
    pub fn find(name: &str) -> ParseResult<'static, Self> {
        match name {
            "print" => Ok(Self::BuiltinFunction(FunctionTypes::Print)),
            "len" => Ok(Self::BuiltinFunction(FunctionTypes::Len)),
            "str" => Ok(Self::BuiltinFunction(FunctionTypes::Str)),
            "repr" => Ok(Self::BuiltinFunction(FunctionTypes::Repr)),
            "ValueError" => Ok(Self::Exceptions(ExcType::ValueError)),
            "TypeError" => Ok(Self::Exceptions(ExcType::TypeError)),
            "NameError" => Ok(Self::Exceptions(ExcType::NameError)),
            "range" => Ok(Self::Range),
            _ => Err(ParseError::Internal(format!("unknown builtin: `{name}`").into())),
        }
    }

    pub fn call_function<'c>(&self, heap: &mut Heap, args: Vec<Object>) -> RunResult<'c, Object> {
        match self {
            Self::BuiltinFunction(FunctionTypes::Print) => {
                for (i, object) in args.iter().enumerate() {
                    if i == 0 {
                        print!("{}", object.str(heap));
                    } else {
                        print!(" {}", object.str(heap));
                    }
                }
                println!();
                Ok(Object::None)
            }
            Self::BuiltinFunction(FunctionTypes::Len) => {
                if args.len() != 1 {
                    return exc_err_fmt!(ExcType::TypeError; "len() takes exactly exactly one argument ({} given)", args.len());
                }
                let object = &args[0];
                match object.len(heap) {
                    Some(len) => Ok(Object::Int(len as i64)),
                    None => exc_err_fmt!(ExcType::TypeError; "Object of type {} has no len()", object.repr(heap)),
                }
            }
            Self::BuiltinFunction(FunctionTypes::Str) => {
                if args.len() != 1 {
                    return exc_err_fmt!(ExcType::TypeError; "str() takes exactly exactly one argument ({} given)", args.len());
                }
                let object = &args[0];
                let object_id = heap.allocate(HeapData::Str(object.str(heap).into_owned()));
                Ok(Object::Ref(object_id))
            }
            Self::BuiltinFunction(FunctionTypes::Repr) => {
                if args.len() != 1 {
                    return exc_err_fmt!(ExcType::TypeError; "repr() takes exactly exactly one argument ({} given)", args.len());
                }
                let object = &args[0];
                let object_id = heap.allocate(HeapData::Str(object.repr(heap).into_owned()));
                Ok(Object::Ref(object_id))
            }
            Self::Exceptions(exc_type) => {
                if let Some(first) = args.first() {
                    if args.len() == 1 {
                        if let Object::Ref(object_id) = &first {
                            if let HeapData::Str(s) = heap.get(*object_id) {
                                return Ok(Object::Exc(SimpleException::new(*exc_type, Some(s.to_owned().into()))));
                            }
                        }
                    }
                    internal_err!(InternalRunError::TodoError; "Exceptions can only be called with zero or one string argument")
                } else {
                    Ok(Object::Exc(SimpleException::new(*exc_type, None)))
                }
            }
            Self::Range => {
                if args.len() == 1 {
                    let object = &args[0];
                    let size = object.as_int()?;
                    Ok(Object::Range(size))
                } else {
                    internal_err!(InternalRunError::TodoError; "range() takes exactly one argument")
                }
            }
        }
    }
}
