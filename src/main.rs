mod issue;

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let result = match args.as_slice() {
        [command, url, dir] if command == "fetch-issue" => issue::fetch_issue(url, Path::new(dir)),
        [command, ..] if matches!(command.as_str(), "help" | "--help" | "-h") => {
            usage();
            Ok(())
        }
        _ => {
            usage();
            Err("expected: fetch-issue <issue-url> <output-dir>".to_string())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    println!("usage: software-factory-runtime fetch-issue <issue-url> <output-dir>");
}
