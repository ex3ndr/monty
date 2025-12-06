use std::fmt::{self, Write};

use crate::{
    exceptions::ExcType,
    expressions::{ExprLoc, Identifier},
    object::Object,
    run::RunResult,
    ParseError,
};

/// Type for method call arguments.
///
/// Uses specific variants for common cases (0-2 arguments).
/// Most Python method calls have at most 2 arguments, so this optimization
/// eliminates the Vec heap allocation overhead for the vast majority of calls.
#[derive(Debug)]
pub enum ArgObjects<'c, 'e> {
    Zero,
    One(Object<'c, 'e>),
    Two(Object<'c, 'e>, Object<'c, 'e>),
    Many(Vec<Object<'c, 'e>>),
    // TODO kwarg types
}

impl<'c, 'e> ArgObjects<'c, 'e> {
    /// Checks that zero arguments were passed.
    pub fn check_zero_args(&self, name: &str) -> RunResult<'static, ()> {
        match self {
            Self::Zero => Ok(()),
            _ => Err(ExcType::type_error_no_args(name, self.count())),
        }
    }

    /// Checks that exactly one argument was passed, returning it.
    pub fn get_one_arg(self, name: &str) -> RunResult<'static, Object<'c, 'e>> {
        match self {
            Self::One(a) => Ok(a),
            _ => Err(ExcType::type_error_arg_count(name, 1, self.count())),
        }
    }

    /// Checks that exactly two arguments were passed, returning them as a tuple.
    pub fn get_two_args(self, name: &str) -> RunResult<'static, (Object<'c, 'e>, Object<'c, 'e>)> {
        match self {
            Self::Two(a1, a2) => Ok((a1, a2)),
            _ => Err(ExcType::type_error_arg_count(name, 2, self.count())),
        }
    }

    /// Checks that one or two arguments were passed, returning them as a tuple.
    pub fn get_one_two_args(self, name: &str) -> RunResult<'static, (Object<'c, 'e>, Option<Object<'c, 'e>>)> {
        match self {
            Self::One(a) => Ok((a, None)),
            Self::Two(a1, a2) => Ok((a1, Some(a2))),
            Self::Zero => Err(ExcType::type_error_at_least(name, 1, self.count())),
            Self::Many(_) => Err(ExcType::type_error_at_most(name, 2, self.count())),
        }
    }

    /// Create a new namespace for a function arguments
    pub fn inject_into_namespace(self, namespace: &mut Vec<Object<'c, 'e>>) {
        match self {
            Self::Zero => (),
            Self::One(a) => {
                namespace.push(a);
            }
            Self::Two(a1, a2) => {
                namespace.push(a1);
                namespace.push(a2);
            }
            Self::Many(v) => {
                namespace.extend(v);
            }
        }
    }

    /// Returns the number of arguments.
    fn count(&self) -> usize {
        match self {
            Self::Zero => 0,
            Self::One(_) => 1,
            Self::Two(_, _) => 2,
            Self::Many(v) => v.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Kwarg<'c> {
    pub key: Identifier<'c>,
    pub value: ExprLoc<'c>,
}

/// Expressions that make up a function call's arguments.
#[derive(Debug, Clone)]
pub enum ArgExprs<'c> {
    Zero,
    One(Box<ExprLoc<'c>>),
    Two(Box<ExprLoc<'c>>, Box<ExprLoc<'c>>),
    Args(Vec<ExprLoc<'c>>),
    Kwargs(Vec<Kwarg<'c>>),
    ArgsKargs {
        args: Vec<ExprLoc<'c>>,
        kwargs: Vec<Kwarg<'c>>,
    },
}

impl<'c> ArgExprs<'c> {
    pub fn new(args: Vec<ExprLoc<'c>>, kwargs: Vec<Kwarg<'c>>) -> Self {
        if !kwargs.is_empty() {
            if args.is_empty() {
                Self::Kwargs(kwargs)
            } else {
                Self::ArgsKargs { args, kwargs }
            }
        } else if args.len() > 2 {
            Self::Args(args)
        } else {
            let mut iter = args.into_iter();
            if let Some(first) = iter.next() {
                if let Some(second) = iter.next() {
                    Self::Two(Box::new(first), Box::new(second))
                } else {
                    Self::One(Box::new(first))
                }
            } else {
                Self::Zero
            }
        }
    }

    /// Applies a transformation function to all `ExprLoc` elements in the args.
    ///
    /// This is used during the preparation phase to recursively prepare all
    /// argument expressions before execution.
    pub fn prepare_args(
        &mut self,
        mut f: impl FnMut(ExprLoc<'c>) -> Result<ExprLoc<'c>, ParseError<'c>>,
    ) -> Result<(), ParseError<'c>> {
        // Swap self with Empty to take ownership, then rebuild
        let taken = std::mem::replace(self, Self::Zero);
        *self = match taken {
            Self::Zero => Self::Zero,
            Self::One(arg) => Self::One(Box::new(f(*arg)?)),
            Self::Two(arg1, arg2) => Self::Two(Box::new(f(*arg1)?), Box::new(f(*arg2)?)),
            Self::Args(args) => Self::Args(args.into_iter().map(&mut f).collect::<Result<Vec<_>, _>>()?),
            Self::Kwargs(kwargs) => Self::Kwargs(
                kwargs
                    .into_iter()
                    .map(|kwarg| {
                        Ok(Kwarg {
                            key: kwarg.key,
                            value: f(kwarg.value)?,
                        })
                    })
                    .collect::<Result<Vec<_>, ParseError<'c>>>()?,
            ),
            Self::ArgsKargs { args, kwargs } => {
                let args = args
                    .into_iter()
                    .map(&mut f)
                    .collect::<Result<Vec<_>, ParseError<'c>>>()?;
                let kwargs = kwargs
                    .into_iter()
                    .map(|kwarg| {
                        Ok(Kwarg {
                            key: kwarg.key,
                            value: f(kwarg.value)?,
                        })
                    })
                    .collect::<Result<Vec<_>, ParseError<'c>>>()?;
                Self::ArgsKargs { args, kwargs }
            }
        };
        Ok(())
    }
}

impl fmt::Display for ArgExprs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('(')?;
        match self {
            Self::Zero => {}
            Self::One(arg) => write!(f, "{arg}")?,
            Self::Two(arg1, arg2) => write!(f, "{arg1}, {arg2}")?,
            Self::Args(args) => {
                for (index, arg) in args.iter().enumerate() {
                    if index == 0 {
                        write!(f, "{arg}")?;
                    } else {
                        write!(f, ", {arg}")?;
                    }
                }
            }
            Self::Kwargs(kwargs) => {
                for (index, kwarg) in kwargs.iter().enumerate() {
                    if index == 0 {
                        write!(f, "{}={}", kwarg.key.name, kwarg.value)?;
                    } else {
                        write!(f, ", {}={}", kwarg.key.name, kwarg.value)?;
                    }
                }
            }
            Self::ArgsKargs { args, kwargs } => {
                for (index, arg) in args.iter().enumerate() {
                    if index == 0 {
                        write!(f, "{arg}")?;
                    } else {
                        write!(f, ", {arg}")?;
                    }
                }
                for kwarg in kwargs {
                    write!(f, ", {}={}", kwarg.key.name, kwarg.value)?;
                }
            }
        }
        f.write_char(')')
    }
}
