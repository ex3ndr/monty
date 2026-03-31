//! Implementation of the `open()` builtin function.
//!
//! `open()` yields an `OsCall(FileOpen)` to the host, which reads or prepares the file
//! and returns `MontyObject::FileData`. The VM then creates a `FileObject` on the heap.

use crate::{
    ResourceTracker,
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult, SimpleException},
    heap::HeapData,
    os::OsFunction,
    types::str::allocate_string,
    value::Value,
};

/// Implementation of the `open()` builtin function.
///
/// Accepts `open(path)` or `open(path, mode)`. The mode defaults to `'r'` (read).
/// Yields `OsCall(FileOpen)` with path and mode arguments for the host to resolve.
pub fn builtin_open(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
    let (path_value, mode_value) = args.get_one_two_args("open", vm.heap)?;

    // Extract strings to owned copies before dropping values
    let path_str = extract_str(&path_value, vm, "open", "file")?.to_owned();
    let mode_str = if let Some(ref mode_val) = mode_value {
        extract_str(mode_val, vm, "open", "mode")?.to_owned()
    } else {
        "r".to_owned()
    };

    // Clean up the original arguments
    path_value.drop_with_heap(vm);
    if let Some(m) = mode_value {
        m.drop_with_heap(vm);
    }

    // Validate mode
    if mode_str != "r" && mode_str != "w" {
        return Err(SimpleException::new_msg(ExcType::ValueError, format!("invalid mode: '{mode_str}'")).into());
    }

    // Build the OsCall arguments: path string and mode string as Values
    let os_path = allocate_string(path_str, vm.heap)?;
    let os_mode = allocate_string(mode_str, vm.heap)?;

    Ok(CallResult::OsCall(
        OsFunction::FileOpen,
        ArgValues::Two(os_path, os_mode),
    ))
}

/// Extracts a `&str` from a `Value`, raising `TypeError` if it's not a string.
fn extract_str<'a>(
    value: &'a Value,
    vm: &'a VM<'_, '_, impl ResourceTracker>,
    func_name: &str,
    arg_name: &str,
) -> RunResult<&'a str> {
    match value {
        Value::InternString(id) => Ok(vm.interns.get_str(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => Ok(s.as_str()),
            _ => Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("{func_name}() {arg_name} argument must be str"),
            )
            .into()),
        },
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{func_name}() {arg_name} argument must be str"),
        )
        .into()),
    }
}
