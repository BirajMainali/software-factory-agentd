mod dashboard;
mod github;
mod home;
mod opencode;
mod process;
mod project;
mod runtime;
mod workflow;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

const DEFAULT_DASHBOARD_ADDR: &str = "127.0.0.1:7878";

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
        [command, url] if command == "work" => {
            workflow::Worker::run(url)?;
            Ok(ExitCode::SUCCESS)
        }
        [command, repos @ ..] if command == "run" => {
            runtime::run(repos)?;
            Ok(ExitCode::SUCCESS)
        }
        [command] if command == "dashboard" => {
            dashboard::serve(DEFAULT_DASHBOARD_ADDR)?;
            Ok(ExitCode::SUCCESS)
        }
        [command, addr] if command == "dashboard" => {
            dashboard::serve(addr)?;
            Ok(ExitCode::SUCCESS)
        }
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
    let agent = take_optional(&mut flags, "agent");
    let title = take_optional(&mut flags, "title");
    ensure_no_unknown(&flags)?;

    let client = opencode::Client::connect()?;
    let session = match session {
        Some(session) => session,
        None => {
            let directory = std::env::current_dir()?;
            client.create_session(&opencode::NewSession {
                title: title.as_deref().unwrap_or("spawn"),
                directory: Some(&directory),
                parent: None,
                agent: agent.as_deref(),
            })?
        }
    };
    let after = client.last_run(&session)?;
    client.prompt(&session, &prompt)?;
    let outcome = client.wait_for_run(&session, after.as_deref())?;
    let _ = writeln!(
        std::io::stdout(),
        "{}",
        serde_json::json!({ "session": session, "outcome": outcome })
    );
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
        "  software-factory-runtime spawn --prompt <text> [--session <id>] [--agent <id>] [--title <text>]"
    );
    let _ = writeln!(out, "  software-factory-runtime work <issue-url>");
    let _ = writeln!(out, "  software-factory-runtime run <owner/repo>...");
    let _ = writeln!(
        out,
        "  software-factory-runtime dashboard [<addr>]   (default {DEFAULT_DASHBOARD_ADDR})"
    );
}
