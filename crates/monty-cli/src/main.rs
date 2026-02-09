use std::{
    env, fs,
    io::{self, BufRead, Write},
    process::ExitCode,
    time::Instant,
};

use monty::{MontyObject, MontyRepl, MontyRun, NoLimitTracker, RunProgress, StdPrint};
// disabled due to format failing on https://github.com/pydantic/monty/pull/75 where CI and local wanted imports ordered differently
// TODO re-enabled soon!
#[rustfmt::skip]
use monty_type_checking::{SourceFile, type_check};

/// Monty — a sandboxed Python interpreter written in Rust.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Python file to execute.
    file: String,
}

const EXT_FUNCTIONS: bool = false;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let repl_mode = matches!(args.get(1).map(String::as_str), Some("--repl" | "-r"));

    if repl_mode {
        let file_path = args.get(2).map_or("repl.py", String::as_str);
        let code = if args.len() > 2 {
            match read_file(file_path) {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("error: {err}");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            String::new()
        };
        return run_repl(file_path, code);
    }

    let file_path = args.get(1).map_or("example.py", String::as_str);
    let code = match read_file(file_path) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    run_script(file_path, code)
}

fn run_script(file_path: &str, code: String) -> ExitCode {
    let start = Instant::now();
    if let Some(failure) = type_check(&SourceFile::new(&code, file_path), None).unwrap() {
        eprintln!("type checking failed:\n{failure}");
    } else {
        eprintln!("type checking succeeded");
    }
    let elapsed = start.elapsed();
    println!("time taken to run typing: {elapsed:?}");

    let input_names = vec![];
    let inputs = vec![];
    let ext_functions = vec!["add_ints".to_owned()];

    let runner = match MontyRun::new(code, file_path, input_names, ext_functions) {
        Ok(ex) => ex,
        Err(err) => {
            eprintln!("error:\n{err}");
            return ExitCode::FAILURE;
        }
    };

    if EXT_FUNCTIONS {
        let start = Instant::now();
        let progress = match runner.start(inputs, NoLimitTracker, &mut StdPrint) {
            Ok(p) => p,
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!("error after: {elapsed:?}\n{err}");
                return ExitCode::FAILURE;
            }
        };

        match run_until_complete(progress) {
            Ok(value) => {
                let elapsed = start.elapsed();
                eprintln!("success after: {elapsed:?}\n{value}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!("error after: {elapsed:?}\n{err}");
                ExitCode::FAILURE
            }
        }
    } else {
        let start = Instant::now();
        let value = match runner.run_no_limits(inputs) {
            Ok(p) => p,
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!("error after: {elapsed:?}\n{err}");
                return ExitCode::FAILURE;
            }
        };
        let elapsed = start.elapsed();
        eprintln!("success after: {elapsed:?}\n{value}");
        ExitCode::SUCCESS
    }
}

fn run_repl(file_path: &str, code: String) -> ExitCode {
    let input_names = vec![];
    let inputs = vec![];
    let ext_functions = vec!["add_ints".to_owned()];

    let (mut repl, init_output) = match MontyRepl::new(
        code,
        file_path,
        input_names,
        ext_functions,
        inputs,
        NoLimitTracker,
        &mut StdPrint,
    ) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("error initializing repl:\n{err}");
            return ExitCode::FAILURE;
        }
    };

    if init_output != MontyObject::None {
        println!("{init_output}");
    }

    eprintln!("Monty REPL mode. Enter Python snippets line-by-line. Use :quit to exit.");
    let stdin = io::stdin();
    let mut stdin = stdin.lock();

    loop {
        print!(">>> ");
        if io::stdout().flush().is_err() {
            eprintln!("error: failed to flush stdout");
            return ExitCode::FAILURE;
        }

        let mut line = String::new();
        let read = match stdin.read_line(&mut line) {
            Ok(n) => n,
            Err(err) => {
                eprintln!("error reading input: {err}");
                return ExitCode::FAILURE;
            }
        };

        if read == 0 {
            return ExitCode::SUCCESS;
        }

        let snippet = line.trim_end();
        if snippet.is_empty() {
            continue;
        }
        if snippet == ":quit" {
            return ExitCode::SUCCESS;
        }

        match repl.feed_no_print(snippet) {
            Ok(output) => {
                if output != MontyObject::None {
                    println!("{output}");
                }
            }
            Err(err) => eprintln!("error:\n{err}"),
        }
    }
}

fn run_until_complete(mut progress: RunProgress<NoLimitTracker>) -> Result<MontyObject, String> {
    loop {
        match progress {
            RunProgress::Complete(value) => return Ok(value),
            RunProgress::FunctionCall {
                function_name,
                args,
                state,
                ..
            } => {
                let return_value = resolve_external_call(&function_name, &args)?;
                progress = state.run(return_value, &mut StdPrint).map_err(|err| format!("{err}"))?;
            }
            RunProgress::ResolveFutures(state) => {
                return Err(format!(
                    "async futures not supported in CLI: {:?}",
                    state.pending_call_ids()
                ));
            }
            RunProgress::OsCall { function, args, .. } => {
                return Err(format!("OS calls not supported in CLI: {function:?}({args:?})"));
            }
        }
    }
}

fn resolve_external_call(function_name: &str, args: &[MontyObject]) -> Result<MontyObject, String> {
    if function_name != "add_ints" {
        return Err(format!("unknown external function: {function_name}({args:?})"));
    }

    if args.len() != 2 {
        return Err(format!("add_ints requires exactly 2 arguments, got {}", args.len()));
    }

    if let (MontyObject::Int(a), MontyObject::Int(b)) = (&args[0], &args[1]) {
        Ok(MontyObject::Int(a + b))
    } else {
        Err(format!("add_ints requires integer arguments, got {args:?}"))
    }
}

fn read_file(file_path: &str) -> Result<String, String> {
    eprintln!("Reading file: {file_path}");
    match fs::metadata(file_path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(format!("Error: {file_path} is not a file"));
            }
        }
        Err(err) => {
            return Err(format!("Error reading {file_path}: {err}"));
        }
    }
    match fs::read_to_string(file_path) {
        Ok(contents) => Ok(contents),
        Err(err) => Err(format!("Error reading file: {err}")),
    }
}
