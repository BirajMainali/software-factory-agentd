mod dashboard;
mod github;
mod home;
mod opencode;
mod process;
mod supervisor;
mod workflow;

use std::collections::BTreeMap;
use std::error::Error;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

const DEFAULT_DASHBOARD_ADDR: &str = "127.0.0.1:7878";
const FOREGROUND: &str = "--foreground";
const USAGE: &str = "usage:
  agentd work --issue <issue-url> --path <dir>
  agentd fetch-issue <issue-url> <output-dir>
  agentd fetch-pr <pr-url> <output-dir>
  agentd spawn --prompt <text> [--session <id>] [--agent <id>] [--title <text>]
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
        [command, rest @ ..] if command == "work" => work(rest)?,
        [command, rest @ ..] if command == "spawn" => spawn(rest)?,
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

/// Route one issue into `--path`. Starts the worker in the background and returns
/// its pid, so a loop can submit many issues at once. `--foreground` (used by the
/// scheduler) runs it in this process instead.
fn work(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (foreground, args) = take_foreground(args);
    let mut flags = parse_flags(&args)?;
    let issue = flags.remove("issue").ok_or("missing --issue")?;
    let path = flags.remove("path").ok_or("missing --path")?;
    if let Some(name) = flags.keys().next() {
        return Err(format!("unknown flag --{name}").into());
    }
    if foreground {
        supervisor::supervise(&issue, Path::new(&path))?;
    } else {
        submit(&issue, &path)?;
    }
    Ok(())
}

/// Split the bare `--foreground` switch from the `--name value` flags.
fn take_foreground(args: &[String]) -> (bool, Vec<String>) {
    let mut foreground = false;
    let mut flags = Vec::with_capacity(args.len());
    for arg in args {
        if arg.as_str() == FOREGROUND {
            foreground = true;
        } else {
            flags.push(arg.clone());
        }
    }
    (foreground, flags)
}

/// Start the worker in a child process and return at once.
fn submit(issue: &str, path: &str) -> Result<(), Box<dyn Error>> {
    let url: github::GitHubUrl = issue.parse()?;
    url.expect_issue()?;
    let home = home::home()?;
    let dir = home::task_dir(&home, &url);
    // Fail here, not silently in a detached child, if the task is already running.
    if home::try_lock(&home::task_lock_path(&dir))?.is_none() {
        return Err(format!("another process already owns {issue}").into());
    }

    let child = Command::new(std::env::current_exe()?)
        .args(["work", "--issue", issue, "--path", path, FOREGROUND])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let _ = writeln!(
        std::io::stdout(),
        "{}",
        serde_json::json!({ "issue": issue, "pid": child.id() })
    );
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

#[cfg(test)]
mod tests {
    use super::take_foreground;

    #[test]
    fn foreground_switch_is_stripped_from_the_flags() {
        let args: Vec<String> = ["--issue", "u", "--foreground", "--path", "/x"]
            .map(String::from)
            .to_vec();
        let (foreground, flags) = take_foreground(&args);
        assert!(foreground);
        assert_eq!(flags, ["--issue", "u", "--path", "/x"].map(String::from));
    }
}
