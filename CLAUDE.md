# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Monty is a sandboxed Python interpreter written in Rust. It parses Python code using RustPython's parser but implements its own runtime execution model for safety and performance. This is a work-in-progress project that currently supports a subset of Python features.

Project goals:

- **Safety**: Execute untrusted Python code safely without FFI or C dependencies, instead sandbox will call back to host to run foreign/external functions.
- **Performance**: Fast execution through compile-time optimizations and efficient memory layout
- **Simplicity**: Clean, understandable implementation focused on a Python subset
- **Snapshotting and iteration**: Plan is to allow code to be iteratively executed and snapshotted at each function call

## Build and Test Commands

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

## NOTES

ALWAYS run `make lint` after making changes and fix all suggestions to maintain code quality.
