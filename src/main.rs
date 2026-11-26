mod github;
mod opencode;
mod workflow;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match run(&args) {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    match args {
        [command, url, dir] if command == "fetch-issue" => {
            github::issue::fetch_issue(url, Path::new(dir))?;
            Ok(ExitCode::SUCCESS)
        }
        [command, url, dir] if command == "fetch-pr" => {
            github::pr::fetch_pr(url, Path::new(dir))?;
            Ok(ExitCode::SUCCESS)
        }
        [command, rest @ ..] if command == "spawn" => cmd_spawn(rest),
        [command, ..] if matches!(command.as_str(), "help" | "--help" | "-h") => {
            usage();
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            usage();
            Err("expected an issue, pull request, or agent command".into())
        }
    }
}

fn cmd_spawn(args: &[String]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let mut flags = parse_flags(args)?;
    let prompt = take(&mut flags, "prompt")?;
    let session = take_optional(&mut flags, "session");
    let title = take_optional(&mut flags, "title");
    ensure_no_unknown(&flags)?;

    let opencode = opencode::OpenCode;
    let session = opencode.spawn(session.as_deref(), &prompt, title.as_deref())?;
    let _ = writeln!(std::io::stdout(), "session {session}");
    Ok(ExitCode::SUCCESS)
}

fn parse_flags(args: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut flags = BTreeMap::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let Some(rest) = arg.strip_prefix("--") else {
            return Err(format!("unexpected argument: {arg}"));
        };
        let (name, value) = if let Some((name, value)) = rest.split_once('=') {
            if name.is_empty() {
                return Err("empty flag name".into());
            }
            (name.to_string(), value.to_string())
        } else {
            if rest.is_empty() {
                return Err("empty flag name".into());
            }
            let value = iter
                .next()
                .ok_or_else(|| format!("missing value for --{rest}"))?;
            (rest.to_string(), value.clone())
        };
        if flags.insert(name.clone(), value).is_some() {
            return Err(format!("duplicate flag --{name}"));
        }
    }
    Ok(flags)
}

fn take(flags: &mut BTreeMap<String, String>, name: &str) -> Result<String, String> {
    match flags.remove(name) {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) => Err(format!("--{name} cannot be empty")),
        None => Err(format!("missing --{name}")),
    }
}

fn take_optional(flags: &mut BTreeMap<String, String>, name: &str) -> Option<String> {
    flags.remove(name)
}

fn ensure_no_unknown(flags: &BTreeMap<String, String>) -> Result<(), String> {
    match flags.keys().next() {
        Some(name) => Err(format!("unknown flag --{name}")),
        None => Ok(()),
    }
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
    let _ = writeln!(
        out,
        "  software-factory-runtime spawn --prompt <text> [--session <id>] [--title <text>]"
    );
}
