mod github;
mod issue;
mod pr;

use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match args {
        [command, url, dir] if command == "fetch-issue" => {
            issue::fetch_issue(url, Path::new(dir))?;
        }
        [command, url, dir] if command == "fetch-pr" => {
            pr::fetch_pr(url, Path::new(dir))?;
        }
        [command, ..] if matches!(command.as_str(), "help" | "--help" | "-h") => usage(),
        _ => {
            usage();
            return Err(
                "expected: fetch-issue <issue-url> <output-dir> or fetch-pr <pr-url> <output-dir>"
                    .into(),
            );
        }
    }

    Ok(())
}

fn usage() {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "usage:");
    let _ = writeln!(
        out,
        "  software-factory-runtime fetch-issue <issue-url> <output-dir>"
    );
    let _ = writeln!(
        out,
        "  software-factory-runtime fetch-pr <pr-url> <output-dir>"
    );
}
