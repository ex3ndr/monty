use monty::{Executor, Exit};
use pyo3::prelude::*;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::Path;

/// Specifies which interpreters a test should run on.
///
/// Parsed from an optional `# test=monty,cpython` comment at the start of a test file.
/// If not present, defaults to running on both interpreters.
#[derive(Debug, Clone)]
struct TestTargets {
    monty: bool,
    cpython: bool,
}

impl Default for TestTargets {
    fn default() -> Self {
        Self {
            monty: true,
            cpython: true,
        }
    }
}

/// Represents the expected outcome of a test fixture
#[derive(Debug, Clone)]
enum Expectation {
    /// Expect exception with specific message
    Raise(String),
    /// Expect parse error containing message
    ParseError(String),
    /// Expect successful execution, check py_str() output
    ReturnStr(String),
    /// Expect successful execution, check py_repr() output
    Return(String),
    /// Expect successful execution, check py_type() output
    ReturnType(String),
    /// Expect successful execution, check ref counts of named variables.
    /// Only used when `ref-counting` feature is enabled; skipped otherwise.
    RefCounts(#[cfg_attr(not(feature = "ref-counting"), allow(dead_code))] HashMap<String, usize>),
}

impl Expectation {
    /// Returns the expected value string
    fn expected_value(&self) -> &str {
        match self {
            Expectation::Raise(s)
            | Expectation::ParseError(s)
            | Expectation::ReturnStr(s)
            | Expectation::Return(s)
            | Expectation::ReturnType(s) => s,
            Expectation::RefCounts(_) => "",
        }
    }
}

/// Parse a Python fixture file into code, expected outcome, and test targets.
///
/// The file may optionally start with a `# test=monty,cpython` comment to specify
/// which interpreters to run on. If not present, defaults to both.
///
/// The file MUST have an expectation comment as the LAST line:
/// - `# Raise=ExceptionType('message')` - Exception format
/// - `# ParseError=message` - Parse error format
/// - `# Return.str=value` - Check py_str() output
/// - `# Return=value` - Check py_repr() output
/// - `# Return.type=typename` - Check py_type() output
/// - `# ref-counts={'var': count, ...}` - Check ref counts of named heap variables
fn parse_fixture(content: &str) -> (String, Expectation, TestTargets) {
    let lines: Vec<&str> = content.lines().collect();

    assert!(!lines.is_empty(), "Empty fixture file");

    // Check for test targets comment at the start of the file
    let (targets, code_start_idx) = if let Some(first_line) = lines.first() {
        if let Some(targets_str) = first_line.strip_prefix("# test=") {
            let targets = TestTargets {
                monty: targets_str.contains("monty"),
                cpython: targets_str.contains("cpython"),
            };
            (targets, 1)
        } else {
            (TestTargets::default(), 0)
        }
    } else {
        (TestTargets::default(), 0)
    };

    // Check if first code line has an expectation (this is an error)
    if let Some(first_code_line) = lines.get(code_start_idx) {
        if first_code_line.starts_with("# Return")
            || first_code_line.starts_with("# Raise")
            || first_code_line.starts_with("# ParseError")
        {
            panic!("Expectation comment must be on the LAST line, not the first line");
        }
    }

    // Get the last line (must be the expectation)
    let last_line = lines.last().unwrap();
    let expectation_line = last_line;
    let code_lines = &lines[code_start_idx..lines.len() - 1];

    // Parse expectation from comment line
    // Note: Check more specific patterns first (Return.str, Return.type, ref-counts) before general Return
    let expectation = if let Some(expected) = expectation_line.strip_prefix("# ref-counts=") {
        Expectation::RefCounts(parse_ref_counts(expected))
    } else if let Some(expected) = expectation_line.strip_prefix("# Return.str=") {
        Expectation::ReturnStr(expected.to_string())
    } else if let Some(expected) = expectation_line.strip_prefix("# Return.type=") {
        Expectation::ReturnType(expected.to_string())
    } else if let Some(expected) = expectation_line.strip_prefix("# Return=") {
        Expectation::Return(expected.to_string())
    } else if let Some(expected) = expectation_line.strip_prefix("# Raise=") {
        Expectation::Raise(expected.to_string())
    } else if let Some(expected) = expectation_line.strip_prefix("# ParseError=") {
        Expectation::ParseError(expected.to_string())
    } else {
        panic!("Invalid expectation format in comment line: {expectation_line}");
    };

    // Code is everything except the test targets comment and expectation comment
    let code = code_lines.join("\n");

    (code, expectation, targets)
}

/// Parses the ref-counts format: {'var': count, 'var2': count2}
///
/// Supports both single and double quotes for variable names.
/// Example: {'x': 2, 'y': 1} or {"x": 2, "y": 1}
fn parse_ref_counts(s: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    let trimmed = s.trim().trim_start_matches('{').trim_end_matches('}');
    for pair in trimmed.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let parts: Vec<&str> = pair.split(':').collect();
        assert!(
            parts.len() == 2,
            "Invalid ref-counts pair format: {pair}. Expected 'name': count"
        );
        let name = parts[0].trim().trim_matches('\'').trim_matches('"');
        let count: usize = parts[1]
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("Invalid ref count value: {}", parts[1]));
        counts.insert(name.to_string(), count);
    }
    counts
}

/// Run a test with the given code and expectation
///
/// This function executes Python code via the Executor and validates the result
/// against the expected outcome specified in the fixture.
fn run_test(path: &Path, code: &str, expectation: Expectation) {
    let test_name = path.strip_prefix("test_cases/").unwrap_or(path).display().to_string();

    // Handle ref-counting tests separately since they need run_ref_counts()
    #[cfg(feature = "ref-counting")]
    if let Expectation::RefCounts(expected) = &expectation {
        match Executor::new(code, "test.py", &[]) {
            Ok(ex) => {
                let result = ex.run_ref_counts(vec![]);
                match result {
                    Ok((Exit::Return(_), (actual, unique_refs, heap_count))) => {
                        // Strict matching: verify all heap objects are accounted for by variables
                        assert_eq!(
                            unique_refs, heap_count,
                            "[{test_name}] Strict matching failed: {heap_count} heap objects exist, \
                             but only {unique_refs} are referenced by variables.\n\
                             Actual ref counts: {actual:?}"
                        );
                        assert_eq!(&actual, expected, "[{test_name}] ref-counts mismatch");
                    }
                    Ok((Exit::Raise(exc), _)) => {
                        panic!("[{test_name}] Unexpected exception: {exc:?}");
                    }
                    Err(e) => panic!("[{test_name}] Runtime error: {e:?}"),
                }
            }
            Err(parse_err) => {
                panic!("[{test_name}] Unexpected parse error: {parse_err:?}");
            }
        }
        return;
    }

    match Executor::new(code, "test.py", &[]) {
        Ok(ex) => {
            let result = ex.run(vec![]);
            match result {
                Ok(Exit::Return(obj)) => match expectation {
                    Expectation::ReturnStr(expected) => {
                        let output = obj.py_str();
                        assert_eq!(output.as_ref(), expected, "[{test_name}] py_str() mismatch");
                    }
                    Expectation::Return(expected) => {
                        let output = obj.py_repr();
                        assert_eq!(output.as_ref(), expected, "[{test_name}] py_repr() mismatch");
                    }
                    Expectation::ReturnType(expected) => {
                        let output = obj.py_type();
                        assert_eq!(output, expected, "[{test_name}] py_type() mismatch");
                    }
                    #[cfg(not(feature = "ref-counting"))]
                    Expectation::RefCounts(_) => {
                        // Skip ref-count tests when feature is disabled
                    }
                    _ => panic!("[{test_name}] Expected return, got different expectation type"),
                },
                Ok(Exit::Raise(exc)) => {
                    if let Expectation::Raise(expected) = expectation {
                        let output = format!("{}", exc.exc);
                        assert_eq!(output, expected, "[{test_name}] Exception mismatch");
                    } else {
                        panic!("[{test_name}] Unexpected exception: {exc:?}");
                    }
                }
                Err(e) => panic!("[{test_name}] Runtime error: {e:?}"),
            }
        }
        Err(parse_err) => {
            if let Expectation::ParseError(expected) = expectation {
                let err_msg = parse_err.summary();
                assert_eq!(err_msg, expected, "[{test_name}] Parse error mismatch");
            } else {
                panic!("[{test_name}] Unexpected parse error: {parse_err:?}");
            }
        }
    }
}

/// Wrap Python code in a function that returns the last expression's value.
///
/// The last non-empty line is treated as an expression whose value should be returned.
/// All other lines are executed as statements.
///
/// If `add_return` is false, all lines are wrapped as-is (for code that raises exceptions).
fn wrap_code_in_function(code: &str, add_return: bool) -> String {
    let lines: Vec<&str> = code.lines().collect();

    // Find the last non-empty line
    let last_idx = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .expect("Empty code");

    let mut result = String::from("def __test__():\n");

    if add_return {
        // Add all lines except the last, indented
        for line in &lines[..last_idx] {
            if line.trim().is_empty() {
                result.push('\n');
            } else {
                result.push_str("    ");
                result.push_str(line);
                result.push('\n');
            }
        }

        // Add the last line as a return statement
        result.push_str("    return ");
        result.push_str(lines[last_idx]);
        result.push('\n');
    } else {
        // Add all lines as-is (for exception tests)
        for line in &lines[..=last_idx] {
            if line.trim().is_empty() {
                result.push('\n');
            } else {
                result.push_str("    ");
                result.push_str(line);
                result.push('\n');
            }
        }
    }

    result
}

/// Run a test through CPython to verify Monty produces the same output
///
/// This function executes the same Python code via CPython (using pyo3) and
/// compares the result with the expected value. This ensures Monty behaves
/// identically to CPython.
///
/// ParseError tests are skipped since Monty uses a different parser (ruff).
fn run_cpython_test(path: &Path, code: &str, expectation: &Expectation) {
    // Skip ParseError tests - Monty uses ruff parser which has different error messages
    if matches!(expectation, Expectation::ParseError(_) | Expectation::RefCounts(_)) {
        return;
    }

    let test_name = path.strip_prefix("test_cases/").unwrap_or(path).display().to_string();
    let add_return = !matches!(expectation, Expectation::Raise(_));
    let wrapped = wrap_code_in_function(code, add_return);

    let result = Python::with_gil(|py| {
        // Execute the wrapped code to define __test__
        let globals = pyo3::types::PyDict::new(py);
        py.run(&wrapped, Some(globals), None)
            .unwrap_or_else(|e| panic!("[{test_name}] CPython failed to define __test__: {e}"));

        // Get and call the __test__ function
        let test_fn = globals.get_item("__test__").expect("__test__ not defined");

        match test_fn.call0() {
            Ok(result) => {
                // Code returned successfully - format based on expectation type
                match expectation {
                    Expectation::Return(_) => result.repr().unwrap().to_string(),
                    Expectation::ReturnStr(_) => result.str().unwrap().to_string(),
                    Expectation::ReturnType(_) => result.get_type().name().unwrap().to_string(),
                    Expectation::Raise(_) => {
                        panic!("[{test_name}] Expected exception but code completed normally")
                    }
                    Expectation::ParseError(_) => unreachable!(),
                    Expectation::RefCounts(_) => unreachable!(),
                }
            }
            Err(e) => {
                // Code raised an exception
                let exc_type = e.get_type(py).name().unwrap();
                let exc_message: String = e
                    .value(py)
                    .getattr("args")
                    .and_then(|args| args.get_item(0))
                    .and_then(pyo3::PyAny::extract)
                    .unwrap_or_default();

                // Normalize function names: CPython shows "__test__.<locals>.f()" for functions
                // defined inside the test wrapper, but Monty shows just "f()". Strip the prefix
                // so both can use the same expectation.
                let exc_message = exc_message.replace("__test__.<locals>.", "");

                if exc_message.is_empty() {
                    format!("{exc_type}()")
                } else if exc_message.contains('\'') {
                    // Use double quotes when message contains single quotes (like Python's repr)
                    format!("{exc_type}(\"{exc_message}\")")
                } else {
                    // Use single quotes (default Python repr format)
                    format!("{exc_type}('{exc_message}')")
                }
            }
        }
    });

    assert_eq!(
        result,
        expectation.expected_value(),
        "[{test_name}] CPython result mismatch"
    );
}

/// Test function that runs each fixture through Monty
fn run_test_cases_monty(path: &Path) -> Result<(), Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let (code, expectation, targets) = parse_fixture(&content);
    if targets.monty {
        run_test(path, &code, expectation);
    }
    Ok(())
}

/// Test function that runs each fixture through CPython
fn run_test_cases_cpython(path: &Path) -> Result<(), Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let (code, expectation, targets) = parse_fixture(&content);
    if targets.cpython {
        run_cpython_test(path, &code, &expectation);
    }
    Ok(())
}

// Generate tests for all fixture files using datatest-stable harness macro
datatest_stable::harness!(
    run_test_cases_monty,
    "test_cases",
    r"^.*\.py$",
    run_test_cases_cpython,
    "test_cases",
    r"^.*\.py$",
);
