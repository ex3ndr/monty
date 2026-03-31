//! Python file object type implementation (`_io.TextIOWrapper`).
//!
//! Provides an eagerly-loaded file object returned by `open()`. The file content
//! is read entirely into memory via an `OsCall` when `open()` is called. Read
//! operations (`read()`, `readline()`) return data from the in-memory buffer.
//! Write operations accumulate data that is flushed to the host on `close()`.
//!
//! The `FileObject` implements the context manager protocol (`__enter__`/`__exit__`)
//! so it can be used with `with` statements.

use std::{fmt::Write, mem};

use ahash::AHashSet;

use crate::{
    ResourceTracker,
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{DropWithHeap, HeapData, HeapId, HeapItem, HeapRead},
    os::OsFunction,
    resource::ResourceError,
    types::{PyTrait, Type, str::allocate_string},
    value::{EitherStr, Value},
};

/// The file mode: read or write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum FileMode {
    /// Read mode (`'r'`). Content is loaded eagerly from the host.
    Read,
    /// Write mode (`'w'`). Written data is accumulated and flushed on close.
    Write,
}

impl FileMode {
    /// Returns the Python mode string for this file mode.
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "r",
            Self::Write => "w",
        }
    }
}

/// A file object holding eagerly-loaded content (for reads) or accumulated writes.
///
/// Created by the `open()` builtin via an `OsCall`. For read mode, the host reads
/// the entire file and passes the content back. For write mode, writes are buffered
/// in memory and flushed to the host when the file is closed.
///
/// Implements the context manager protocol: `__enter__` returns self, `__exit__`
/// calls `close()`. This ensures files opened with `with open(...) as f:` are
/// properly closed even if an exception occurs.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct FileObject {
    /// The file path (used in repr and for write-mode flush).
    path: Box<str>,
    /// The file mode (read or write).
    mode: FileMode,
    /// File content (eagerly loaded for read mode, empty for write mode).
    content: Box<str>,
    /// Current read position (byte offset into `content`).
    position: usize,
    /// Whether the file has been closed.
    closed: bool,
    /// Accumulated write data (only used in write mode).
    written: Option<String>,
}

impl FileObject {
    /// Creates a new read-mode file object with the given content.
    pub fn new_read(path: String, content: String) -> Self {
        Self {
            path: path.into_boxed_str(),
            mode: FileMode::Read,
            content: content.into_boxed_str(),
            position: 0,
            closed: false,
            written: None,
        }
    }

    /// Creates a new write-mode file object.
    pub fn new_write(path: String) -> Self {
        Self {
            path: path.into_boxed_str(),
            mode: FileMode::Write,
            content: String::new().into_boxed_str(),
            position: 0,
            closed: false,
            written: Some(String::new()),
        }
    }
}

impl HeapItem for FileObject {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.path.len() + self.content.len() + self.written.as_ref().map_or(0, String::len)
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // FileObject contains no heap references
    }
}

impl<'h> HeapRead<'h, FileObject> {
    /// Reads all remaining content from the current position.
    ///
    /// Returns the content from the current read position to the end,
    /// then advances the position to the end.
    fn read_all(&mut self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let file = self.get_mut(vm.heap);
        check_closed(file)?;
        check_readable(file)?;
        let remaining = file.content[file.position..].to_owned();
        file.position = file.content.len();
        allocate_string(remaining, vm.heap)
    }

    /// Reads the next line from the file.
    ///
    /// Returns the next line including the trailing newline (if present).
    /// Returns an empty string at EOF.
    fn readline(&mut self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let file = self.get_mut(vm.heap);
        check_closed(file)?;
        check_readable(file)?;
        let remaining = &file.content[file.position..];
        let line_end = remaining.find('\n').map_or(remaining.len(), |pos| pos + 1);
        let line = remaining[..line_end].to_owned();
        file.position += line_end;
        allocate_string(line, vm.heap)
    }

    /// Writes data to the file's write buffer.
    ///
    /// Returns the number of characters written.
    fn write_str(&mut self, vm: &mut VM<'h, '_, impl ResourceTracker>, data: &str) -> RunResult<Value> {
        let file = self.get_mut(vm.heap);
        check_closed(file)?;
        check_writable(file)?;
        let len = data.len();
        file.written
            .as_mut()
            .expect("write buffer exists in write mode")
            .push_str(data);
        Ok(Value::Int(i64::try_from(len).unwrap_or(i64::MAX)))
    }

    /// Closes the file.
    ///
    /// For read-mode files, this simply marks the file as closed.
    /// For write-mode files, this returns an `OsCall` to flush the accumulated
    /// writes to the host filesystem.
    fn close(&mut self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<CallResult> {
        let file = self.get_mut(vm.heap);
        if file.closed {
            return Ok(CallResult::Value(Value::None));
        }
        file.closed = true;
        if file.mode == FileMode::Write {
            let written = file.written.take().unwrap_or_default();
            let path_str = file.path.to_string();
            let path_value = allocate_string(path_str, vm.heap)?;
            let content_value = allocate_string(written, vm.heap)?;
            Ok(CallResult::OsCall(
                OsFunction::FileClose,
                ArgValues::Two(path_value, content_value),
            ))
        } else {
            Ok(CallResult::Value(Value::None))
        }
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, FileObject> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::TextIOWrapper
    }

    fn py_len(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, _other: &Self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        // File objects use identity comparison (same as CPython)
        Ok(false)
    }

    fn py_bool(&self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        let file = self.get(vm.heap);
        Ok(write!(
            f,
            "<_io.TextIOWrapper name='{}' mode='{}'>",
            file.path,
            file.mode.as_str()
        )?)
    }

    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let method = attr.as_str(vm.interns);
        match method {
            "read" => {
                args.check_zero_args("read", vm.heap)?;
                Ok(CallResult::Value(self.read_all(vm)?))
            }
            "readline" => {
                args.check_zero_args("readline", vm.heap)?;
                Ok(CallResult::Value(self.readline(vm)?))
            }
            "write" => {
                let data = args.get_one_arg("write", vm.heap)?;
                // Extract the string to an owned copy to avoid borrow conflict
                let s = get_str_arg(&data, vm, "write")?.to_owned();
                let result = self.write_str(vm, &s)?;
                data.drop_with_heap(vm);
                Ok(CallResult::Value(result))
            }
            "close" => {
                args.check_zero_args("close", vm.heap)?;
                self.close(vm)
            }
            "__enter__" => {
                args.check_zero_args("__enter__", vm.heap)?;
                // Increment refcount because we're returning a new reference to self
                vm.heap.inc_ref(self_id);
                Ok(CallResult::Value(Value::Ref(self_id)))
            }
            "__exit__" => {
                // __exit__ receives (exc_type, exc_val, exc_tb) — we ignore them
                // and just close the file. Always returns False (never suppresses).
                args.drop_with_heap(vm);
                let close_result = self.close(vm)?;
                // For read mode, close returns Value(None) — we return False.
                // For write mode, close returns OsCall to flush — the host returns
                // None which is falsy, matching __exit__'s "don't suppress" contract.
                match close_result {
                    CallResult::Value(_) => Ok(CallResult::Value(Value::Bool(false))),
                    other => Ok(other),
                }
            }
            "readable" => {
                args.check_zero_args("readable", vm.heap)?;
                let file = self.get(vm.heap);
                Ok(CallResult::Value(Value::Bool(file.mode == FileMode::Read)))
            }
            "writable" => {
                args.check_zero_args("writable", vm.heap)?;
                let file = self.get(vm.heap);
                Ok(CallResult::Value(Value::Bool(file.mode == FileMode::Write)))
            }
            _ => {
                args.drop_with_heap(vm);
                Err(ExcType::attribute_error(Type::TextIOWrapper, method))
            }
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        let name = attr.as_str(vm.interns);
        match name {
            "closed" => {
                let file = self.get(vm.heap);
                Ok(Some(CallResult::Value(Value::Bool(file.closed))))
            }
            "name" => {
                let file = self.get(vm.heap);
                let name_str = file.path.to_string();
                let value = allocate_string(name_str, vm.heap)?;
                Ok(Some(CallResult::Value(value)))
            }
            "mode" => {
                let file = self.get(vm.heap);
                let mode_str = file.mode.as_str().to_owned();
                let value = allocate_string(mode_str, vm.heap)?;
                Ok(Some(CallResult::Value(value)))
            }
            _ => Err(ExcType::attribute_error(Type::TextIOWrapper, name)),
        }
    }
}

/// Checks that the file is not closed, raising `ValueError` if it is.
fn check_closed(file: &FileObject) -> RunResult<()> {
    if file.closed {
        Err(SimpleException::new_msg(ExcType::ValueError, "I/O operation on closed file.").into())
    } else {
        Ok(())
    }
}

/// Checks that the file is in read mode, raising `ValueError` if not.
fn check_readable(file: &FileObject) -> RunResult<()> {
    if file.mode == FileMode::Read {
        Ok(())
    } else {
        Err(SimpleException::new_msg(ExcType::ValueError, "not readable").into())
    }
}

/// Checks that the file is in write mode, raising `ValueError` if not.
fn check_writable(file: &FileObject) -> RunResult<()> {
    if file.mode == FileMode::Write {
        Ok(())
    } else {
        Err(SimpleException::new_msg(ExcType::ValueError, "not writable").into())
    }
}

/// Extracts a `&str` from a `Value`, raising `TypeError` if it's not a string.
fn get_str_arg<'a>(value: &'a Value, vm: &'a VM<'_, '_, impl ResourceTracker>, method: &str) -> RunResult<&'a str> {
    match value {
        Value::InternString(id) => Ok(vm.interns.get_str(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => Ok(s.as_str()),
            other => Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("{method}() argument must be str, not {}", other.py_type()),
            )
            .into()),
        },
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{method}() argument must be str, not {}", value.py_type(vm)),
        )
        .into()),
    }
}
