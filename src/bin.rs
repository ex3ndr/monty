use std::env;
use std::fs;
use std::process::ExitCode;
use std::time::Instant;

use monty::Executor;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let file_path = if args.len() > 1 { &args[1] } else { "monty.py" };
    let code = match read_file(file_path) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    // let input_names = vec!["foo", "bar"];
    // let inputs = vec![Object::Int(1), Object::Int(2)];
    let input_names = vec![];
    let inputs = vec![];

    let mut ex = match Executor::new(&code, file_path, &input_names) {
        Ok(ex) => ex,
        Err(err) => {
            eprintln!("Error parsing code: {err}");
            return ExitCode::FAILURE;
        }
    };

    let tic = Instant::now();
    let r = ex.run(inputs);
    let toc = Instant::now();
    eprintln!("Elapsed time: {:?}\n", toc - tic);
    match r {
        Ok(exit) => {
            println!("Exit:\n{exit}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("Error running code: {err}");
            ExitCode::FAILURE
        }
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
