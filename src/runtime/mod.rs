//! The runtime, the only scheduler. Polls repos for labelled open issues and runs one
//! worker process per task, at most `MAX_WORKERS` at a time, retrying failed workers
//! with backoff. Workers resume from their event log, so a retry or a runtime restart
//! never repeats finished work.

use crate::github::{self, GitHubUrl, gh_json, is_valid_name};
use crate::home;
use crate::workflow::{self, Event, Log, TaskState};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::env;
use std::io::{self, Write as _};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const ISSUE_LABEL: &str = "factory";
const MAX_WORKERS: usize = 4;
const POLL_INTERVAL: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("expected a repository as owner/repo, got {0}")]
    InvalidRepo(String),
    #[error("give at least one owner/repo to watch")]
    NoRepos,
    #[error(transparent)]
    Home(#[from] home::Error),
    #[error(transparent)]
    Log(#[from] workflow::LogError),
    #[error("cannot locate this executable: {0}")]
    CurrentExe(#[source] io::Error),
}

#[derive(Deserialize)]
struct IssueRef {
    url: String,
}

struct Running {
    issue: GitHubUrl,
    child: Child,
}

pub(crate) fn run(repos: &[String]) -> Result<(), Error> {
    if repos.is_empty() {
        return Err(Error::NoRepos);
    }
    for repo in repos {
        validate_repo(repo)?;
    }
    let exe = env::current_exe().map_err(Error::CurrentExe)?;
    let home = home::home()?;
    home::project_path()?;
    let mut running: BTreeMap<String, Running> = BTreeMap::new();
    let mut next_poll = Instant::now();

    loop {
        reap(&home, &mut running);
        if Instant::now() >= next_poll {
            next_poll = Instant::now() + POLL_INTERVAL;
            for repo in repos {
                match open_issues(repo) {
                    Ok(issues) => {
                        for issue in issues {
                            watch_feedback(&home, &issue);
                            if running.len() < MAX_WORKERS {
                                schedule(&home, &exe, issue, &mut running);
                            }
                        }
                    }
                    Err(error) => emit(&json!({
                        "event": "poll_failed",
                        "repo": repo,
                        "reason": error.to_string(),
                    })),
                }
            }
        }
        thread::sleep(TICK);
    }
}

fn validate_repo(repo: &str) -> Result<(), Error> {
    match repo.split_once('/') {
        Some((owner, name)) if is_valid_name(owner) && is_valid_name(name) => Ok(()),
        _ => Err(Error::InvalidRepo(repo.to_string())),
    }
}

fn open_issues(repo: &str) -> Result<Vec<GitHubUrl>, github::Error> {
    let issues: Vec<IssueRef> = gh_json(
        "issue list",
        &[
            "issue",
            "list",
            "-R",
            repo,
            "--label",
            ISSUE_LABEL,
            "--state",
            "open",
            "--json",
            "url",
            "--limit",
            "200",
        ],
    )?;
    issues.iter().map(|issue| issue.url.parse()).collect()
}

/// New feedback on an open PR that this task has not seen yet.
fn new_feedback(state: &TaskState, snapshot: &github::pr::Snapshot) -> bool {
    snapshot.open && state.feedback_key() != Some(snapshot.key.as_str())
}

/// If `issue` is finished and its PR has feedback that was never applied, record
/// it and let the next [`schedule`] run the developer on it. A task is finished
/// when it waits after the PR opened, so this is the only thing that wakes it.
fn watch_feedback(home: &Path, issue: &GitHubUrl) {
    let dir = home::task_dir(home, issue);
    let Ok(records) = workflow::read_log(&home::events_path(&dir)) else {
        return;
    };
    let state = TaskState::replay(&records);
    if !state.is_finished() {
        return;
    }
    let Ok(Some(pr)) = github::pr::for_issue(issue) else {
        return;
    };
    let Ok(snapshot) = github::pr::fetch(&pr) else {
        return;
    };
    if !new_feedback(&state, &snapshot) {
        return;
    }

    // A worker may have appended between the read and now; hold the task lock and
    // replay again so the log this decision is based on cannot move.
    let Ok(Some(_lock)) = home::try_lock(&home::task_lock_path(&dir)) else {
        return;
    };
    let Ok(records) = workflow::read_log(&home::events_path(&dir)) else {
        return;
    };
    let state = TaskState::replay(&records);
    if !state.is_finished() || !new_feedback(&state, &snapshot) {
        return;
    }
    let number = state.next_pr_feedback();
    let key = snapshot.key;
    match workflow::Log::open(&home::events_path(&dir))
        .and_then(|mut log| log.append(Event::PrFeedbackArrived { number, key }))
    {
        Ok(_) => emit(&json!({
            "event": "pr_feedback",
            "issue": issue.to_string(),
            "number": number,
        })),
        Err(error) => emit(&json!({
            "event": "pr_feedback_failed",
            "issue": issue.to_string(),
            "reason": error.to_string(),
        })),
    }
}

/// Start a worker for `issue` unless it is running, done, out of retries, or backing off.
fn schedule(home: &Path, exe: &Path, issue: GitHubUrl, running: &mut BTreeMap<String, Running>) {
    let key = issue.to_string();
    if running.contains_key(&key) {
        return;
    }
    let dir = home::task_dir(home, &issue);
    let state = match workflow::read_log(&home::events_path(&dir)) {
        Ok(records) => TaskState::replay(&records),
        Err(error) => {
            emit(&json!({ "event": "task_unreadable", "issue": key, "reason": error.to_string() }));
            return;
        }
    };
    let backing_off = state.retry_at().is_some_and(|at| workflow::now() < at);
    if state.is_finished() || state.is_stuck() || backing_off {
        return;
    }
    // A worker this runtime did not start (manual run, previous runtime) may hold it.
    match home::try_lock(&home::task_lock_path(&dir)) {
        Ok(Some(_free)) => {}
        Ok(None) => return,
        Err(error) => {
            emit(
                &json!({ "event": "task_lock_failed", "issue": key, "reason": error.to_string() }),
            );
            return;
        }
    }

    let path = home::workspace_dir(&dir);
    let spawned = Command::new(exe)
        .args(["work", "--issue", key.as_str(), "--path"])
        .arg(path)
        .arg("--foreground") // the scheduler tracks this process
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(child) => {
            emit(&json!({ "event": "worker_started", "issue": key, "pid": child.id() }));
            running.insert(key, Running { issue, child });
        }
        Err(error) => emit(&json!({
            "event": "worker_spawn_failed",
            "issue": key,
            "reason": error.to_string(),
        })),
    }
}

/// Collect exited workers and record each exit in its task log.
fn reap(home: &Path, running: &mut BTreeMap<String, Running>) {
    let mut exited = Vec::new();
    for (key, worker) in running.iter_mut() {
        match worker.child.try_wait() {
            Ok(Some(status)) => exited.push((key.clone(), status.code())),
            Ok(None) => {}
            Err(error) => {
                emit(&json!({ "event": "worker_lost", "issue": key, "reason": error.to_string() }));
                exited.push((key.clone(), None));
            }
        }
    }
    for (key, code) in exited {
        let Some(worker) = running.remove(&key) else {
            continue;
        };
        emit(&json!({ "event": "worker_exited", "issue": key, "code": code }));
        if let Err(error) = record_exit(home, &worker.issue, code) {
            emit(&json!({ "event": "record_failed", "issue": key, "reason": error.to_string() }));
        }
    }
}

fn record_exit(home: &Path, issue: &GitHubUrl, code: Option<i32>) -> Result<(), Error> {
    let dir = home::task_dir(home, issue);
    // Someone else already runs the task again; it owns the log now.
    let Some(_lock) = home::try_lock(&home::task_lock_path(&dir))? else {
        return Ok(());
    };
    Log::open(&home::events_path(&dir))?.append(Event::WorkerExited { code })?;
    Ok(())
}

/// One machine-readable runtime event on stdout.
fn emit(event: &Value) {
    // A closed stdout must not stop scheduling.
    let _ = writeln!(io::stdout(), "{event}");
}
