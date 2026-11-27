mod dashboard;
mod github;
mod home;
mod opencode;
mod process;
mod runtime;
mod workflow;

use std::collections::BTreeMap;
use std::error::Error;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

const DEFAULT_DASHBOARD_ADDR: &str = "127.0.0.1:7878";
const USAGE: &str = "usage:
  agentd fetch-issue <issue-url> <output-dir>
  agentd fetch-pr <pr-url> <output-dir>
  agentd spawn --prompt <text> [--session <id>] [--agent <id>] [--title <text>]
  agentd work <issue-url>
  agentd run <owner/repo>...
  agentd dashboard [<addr>]   (default 127.0.0.1:7878)";

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

fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    match args {
        [command, url, dir] if command == "fetch-issue" => {
            github::issue::fetch_issue(url, Path::new(dir))?;
        }
        [command, url, dir] if command == "fetch-pr" => github::pr::fetch_pr(url, Path::new(dir))?,
        [command, rest @ ..] if command == "spawn" => spawn(rest)?,
        [command, url] if command == "work" => workflow::Worker::run(url)?,
        [command, repos @ ..] if command == "run" => runtime::run(repos)?,
        [command, addr @ ..] if command == "dashboard" && addr.len() <= 1 => {
            let addr = addr.first().map_or(DEFAULT_DASHBOARD_ADDR, String::as_str);
            dashboard::serve(addr)?;
        }
        [command, ..] if matches!(command.as_str(), "help" | "--help" | "-h") => {
            let _ = writeln!(std::io::stdout(), "{USAGE}");
        }
        _ => return Err(format!("unknown command\n{USAGE}").into()),
    }
    Ok(())
}

/// Prompt an opencode session (new, or `--session`) and print how its run ended.
fn spawn(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut flags = parse_flags(args)?;
    let prompt = flags.remove("prompt").ok_or("missing --prompt")?;
    let session = flags.remove("session");
    let agent = flags.remove("agent");
    let title = flags.remove("title");
    if let Some(name) = flags.keys().next() {
        return Err(format!("unknown flag --{name}").into());
    }

    let client = opencode::Client::connect()?;
    let session = if let Some(session) = session {
        session
    } else {
        let directory = std::env::current_dir()?;
        client.create_session(&opencode::NewSession {
            title: title.as_deref().unwrap_or("spawn"),
            directory: Some(&directory),
            parent: None,
            agent: agent.as_deref(),
        })?
    };
    let after = client.last_run(&session)?;
    client.prompt(&session, &prompt)?;
    let outcome = client.wait_for_run(&session, after.as_deref())?;
    let _ = writeln!(
        std::io::stdout(),
        "{}",
        serde_json::json!({ "session": session, "outcome": outcome })
    );
    Ok(())
}

/// `--name value` pairs.
fn parse_flags(args: &[String]) -> Result<BTreeMap<&str, String>, String> {
    let mut flags = BTreeMap::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let name = arg
            .strip_prefix("--")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("unexpected argument: {arg}"))?;
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for --{name}"))?;
        if flags.insert(name, value.clone()).is_some() {
            return Err(format!("duplicate flag --{name}"));
        }
    }
    Ok(flags)
}
