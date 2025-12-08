# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Monty is a sandboxed Python interpreter written in Rust. It parses Python code using Ruff's `ruff_python_parser` but implements its own runtime execution model for safety and performance. This is a work-in-progress project that currently supports a subset of Python features.

Project goals:

- **Safety**: Execute untrusted Python code safely without FFI or C dependencies, instead sandbox will call back to host to run foreign/external functions.
- **Performance**: Fast execution through compile-time optimizations and efficient memory layout
- **Simplicity**: Clean, understandable implementation focused on a Python subset
- **Snapshotting and iteration**: Plan is to allow code to be iteratively executed and snapshotted at each function call

## Build Commands

```bash
# format python and rust code
make format

# lint python and rust code
make lint

# Build the project
cargo build
```

## Exception

It's important that exceptions raised/returned by this library match those raised by Python.

Wherever you see an Exception with a repeated message, create a dedicated method to create that exception `src/exceptions.rs`.

When writing exception messages, always check `src/exceptions.rs` for existing methods to generate that message.

## Code style

Avoid local imports, unless there's a very good reason, all imports should be at the top of the file.

IMPORTANT: every struct, enum and function should be a comprehensive but concise docstring to
explain what it does and why and any considerations or potential foot-guns of using that type.

Only add examples to docstrings of public functions and structs.

The only exception is trait implementation methods where a docstring is not necessary if the method is self-explanatory.

Similarly, you should add lots of comments to code.

If you see a comment or docstring that's out of date - you MUST update it to be correct.

NOTE: COMMENTS AND DOCSTRINGS ARE EXTREMELY IMPORTANT TO THE LONG TERM HEALTH OF THE PROJECT.

## Tests

Do **NOT** write tests within modules unless explicitly prompted to do so.

Tests should live in the `tests/` directory.

Commands:

```bash
# Build the project
cargo build

# Run tests (this is the best way to run all tests as it enables the dec-ref-check feature)
make test

# Run a specific test
cargo test --features dec-ref-check str__ops

# Run the interpreter on a Python file
cargo run -- <file.py>
```

### Test File Structure

**DO NOT create many small test files.** This would be unmaintainable.

ALWAYS consolidate related tests into single files*using multiple `assert` statements. Follow `test_cases/fstring__all.py` as the gold standard pattern:

```python
# === Section name ===
# brief comment if needed
assert condition, 'descriptive message'
assert another_condition, 'another descriptive message'

# === Next section ===
x = setup_value
assert x == expected, 'test description'
```

Each `assert` should have a descriptive message.

### When to Create Separate Test Files

Only create a separate test file when you MUST use one of these special expectation formats:

- `# Raise=Exception('message')` - Test expects an exception (execution stops)
- `# ParseError=message` - Test expects a parse error
- `# ref-counts={...}` - Test checks reference counts (special mode)
- you're writing tests for a different behavior or section of the language

For everything else, **add asserts to an existing test file** or create ONE consolidated file for the feature.

### File Naming

Name files by feature, not by micro-variant:
- ✅ `str__ops.py` - all string operations (add, iadd, len, etc.)
- ✅ `list__methods.py` - all list method tests
- ❌ `str__add_basic.py`, `str__add_empty.py`, `str__add_multiple.py` - TOO GRANULAR

### Expectation Formats (use sparingly)

Only use these when `assert` won't work (on last line of file):
- `# Return=value` - Check `repr()` output (prefer assert instead)
- `# Return.str=value` - Check `str()` output (prefer assert instead)
- `# Return.type=typename` - Check `type()` output (prefer assert instead)
- `# Raise=Exception('message')` - Expect exception (REQUIRES separate file)
- `# ParseError=message` - Expect parse error (REQUIRES separate file)
- `# ref-counts={...}` - Check reference counts (REQUIRES separate file)
- No expectation comment - Assert-based test (PREFERRED)

Do NOT use `# Return=` when you could use `assert` instead

### Skip Directive

Optional, on first line of file - DO NOT use unless absolutely necessary:
- `# skip=cpython` - Skip CPython test (only run on Monty)
- `# skip=monty` - Skip Monty test (only run on CPython)

Ask for approval before using `skip`!

### Other Notes

- Prefer single quotes for strings in Python tests
- Run `make lint-py` after adding tests
- Use `make complete-tests` to fill in blank expectations
- Tests run via `datatest-stable` harness in `tests/datatest_runner.rs`

## Reference Counting

Heap-allocated values (`Value::Ref`) use manual reference counting. Key rules:

- **Cloning**: Use `clone_with_heap(heap)` which increments refcounts for `Ref` variants.
- **Dropping**: Call `drop_with_heap(heap)` when discarding an `Value` that may be a `Ref`.
- **Borrow conflicts**: When you need to read from the heap and then mutate it, use `copy_for_extend()` to copy the `Value` without incrementing refcount, then call `heap.inc_ref()` separately after the borrow ends.

Container types (`List`, `Tuple`, `Dict`) also have `clone_with_heap()` methods.

**Resource limits**: When resource limits (allocations, memory, time) are exceeded, execution terminates with a `ResourceError`. No guarantees are made about the state of the heap or reference counts after a resource limit is exceeded. The heap may contain orphaned objects with incorrect refcounts. This is acceptable because resource exhaustion is a terminal error - the execution context should be discarded.

## NOTES

ALWAYS consider code quality when adding new code, if functions are getting too complex or code is duplicated, move relevant logic to a new file.
Make sure functions are added in the most logical place, e.g. as methods on a struct where appropriate.

The code should follow the "newspaper" style where public and primary functions are at the top of the file, followed by private functions and utilities.

ALWAYS run `make lint` after making changes and fix all suggestions to maintain code quality.

ALWAYS update this file when it is out of date.

NEVER write `unsafe` code, if you think you need to write unsafe code, explicitly ask the user or leave a `todo!()` with a suggestion and explanation.
