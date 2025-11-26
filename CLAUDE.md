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
# format code and run clippy
make lint

# Build the project
cargo build
```

## Tests

Do **NOT** write tests within modules unless explicitly prompted to do so.

Tests should live in the `tests/` directory.

Commands:

```bash
# Build the project
cargo build

# Run tests
cargo test

# Run a specific test
cargo test execute_ok_add_ints

# Run the interpreter on a Python file
cargo run -- <file.py>
```

## Code style

Avoid local imports, unless there's a very good reason, all imports should be at the top of the file.

IMPORTANT: every struct, enum and function should be a comprehensive but concise docstring to
explain what it does and why and any considerations or potential foot-guns of using that type.

The only exception is trait implementation methods where a docstring is not necessary if the method is self-explanatory.

Similarly, you should add lots of comments to code.

If you see a comment or docstring that's out of date - you MUST update it to be correct.

NOTE: COMMENTS AND DOCSTRINGS ARE EXTREMELY IMPORTANT TO THE LONG TERM HEALTH OF THE PROJECT.

## Tests

Tests should always be as concise as possible while covering all possible cases.

All Python execution behavior tests use file-based fixtures in `test_cases/`. File names: `<group_name>__<test_name>.py`.

**Expectation formats** (on last line of file):
- `# Return=value` - Check `repr()` output
- `# Return.str=value` - Check `str()` output
- `# Return.type=typename` - Check `type()` output
- `# Raise=Exception('message')` - Expect exception
- `# ParseError=message` - Expect parse error

These tests are run via `datatest-stable` harness in `tests/datatest_runner.rs`.

## NOTES

ALWAYS run `make lint` after making changes and fix all suggestions to maintain code quality.

ALWAYS update this file when it is out of date.
