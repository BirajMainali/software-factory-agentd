//! One worker per task: a router. It replays the task's event log, then spawns the
//! next sub-agent, routes its reported stage, and routes human signals, until the
//! workflow ends. What agents do, and how, is theirs. Holds the task lock throughout,
//! so two workers never run one task.

mod agents;
mod prompts;

use super::event::Event;
use super::log::{self, Log};
use super::state::TaskState;
use super::step::{Artifact, Role};
use crate::github::{self, GitHubUrl};
use crate::home;
use crate::opencode::{self, Client, Outcome};
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    GitHub(#[from] github::Error),
    #[error(transparent)]
    Home(#[from] home::Error),
    #[error(transparent)]
    Log(#[from] log::Error),
    #[error(transparent)]
    Agent(#[from] opencode::Error),
    #[error("another worker is running {0}")]
    Busy(String),
    #[error("agent {} run ended {outcome:?} in opencode", .role.agent())]
    SubagentEnded { role: Role, outcome: Outcome },
    #[error("no session for agent {}", .0.agent())]
    NoSession(Role),
    #[error("no pull request found for {0}")]
    NoPullRequest(GitHubUrl),
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot use {path} as the work folder: {source}")]
    Folder {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub(crate) struct Worker<'a> {
    issue: GitHubUrl,
    title: String,
    /// The exact folder the caller chose for artifacts.
    dir: PathBuf,
    /// `$PROJECT_PATH`: the directory the main session and agents run in.
    project: PathBuf,
    opencode: Client,
    log: &'a mut Log,
    state: TaskState,
}

impl Worker<'_> {
    /// Route the issue at `url` until its workflow ends, resuming from the
    /// event log. Artifacts go directly in `path`. Failures are recorded in the
    /// log before returning.
    pub(crate) fn run(url: &str, path: &Path) -> Result<(), Error> {
        let issue: GitHubUrl = url.parse()?;
        issue.expect_issue()?;
        let home = home::home()?;
        let dir = home::task_dir(&home, &issue);
        let Some(_lock) = home::try_lock(&home::task_lock_path(&dir))? else {
            return Err(Error::Busy(issue.to_string()));
        };

        let events = home::events_path(&dir);
        let mut state = TaskState::replay(&log::read(&events)?);
        emit(&json!({
            "event": "progress",
            "issue": issue.to_string(),
            "completed": state.completed(),
            "next": state.current(),
        }));
        if state.is_finished() {
            return Ok(());
        }
        let mut log = Log::open(&events)?;
        let started = log.append(Event::WorkerStarted)?;
        emit(&started);
        state.apply(&started);

        let result = Self::drive(issue, &mut log, state, path);
        if let Err(error) = &result {
            // The worker error is what matters; failing to log it must not hide it.
            if let Ok(failed) = log.append(Event::WorkerFailed {
                reason: error.to_string(),
            }) {
                emit(&failed);
            }
        }
        result
    }

    fn drive(issue: GitHubUrl, log: &mut Log, state: TaskState, path: &Path) -> Result<(), Error> {
        let dir = std::path::absolute(path).map_err(|source| Error::Folder {
            path: path.to_path_buf(),
            source,
        })?;
        // The caller names the work root; this issue gets its own numbered folder.
        let dir = dir.join(format!("#{}", issue.number));
        fs::create_dir_all(&dir).map_err(|source| Error::Folder {
            path: dir.clone(),
            source,
        })?;
        let project = home::project_path()?;
        let opencode = Client::connect()?;
        let fetched = github::issue::fetch(&issue)?;

        let mut worker = Worker {
            issue,
            title: fetched.title,
            dir,
            project,
            opencode,
            log,
            state,
        };
        if worker.state.title.as_deref() != Some(worker.title.as_str()) {
            worker.record(Event::IssueFetched {
                title: worker.title.clone(),
            })?;
        }
        worker.record(Event::WorkPath {
            path: worker.dir.display().to_string(),
        })?;
        // The issue text is written once, so it needs no number.
        let issue_file = prompts::file(&worker.dir, Artifact::Issue, 1);
        write_file(&issue_file, &fetched.markdown)?;
        worker.run_workflow()
    }

    fn run_workflow(&mut self) -> Result<(), Error> {
        while let Some(step) = self.state.current() {
            self.run_step(step)?;
        }
        Ok(())
    }

    fn record(&mut self, event: Event) -> Result<(), Error> {
        let record = self.log.append(event)?;
        emit(&record);
        self.state.apply(&record);
        Ok(())
    }
}

fn write_file(path: &Path, text: &str) -> Result<(), Error> {
    let write_error = |source| Error::Write {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(write_error)?;
    }
    fs::write(path, text).map_err(write_error)
}

/// Create `path` if missing, so a prompt can name a file that is already there.
fn create_file(path: &Path) -> Result<(), Error> {
    if !path.exists() {
        write_file(path, "")?;
    }
    Ok(())
}

/// One machine-readable progress line on stdout.
fn emit(value: &impl Serialize) {
    if let Ok(line) = serde_json::to_string(value) {
        // A closed stdout must not stop the worker.
        let _ = writeln!(io::stdout(), "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_file_makes_folders_and_keeps_content() {
        let dir = std::env::temp_dir().join(format!("agentd-create-{}", std::process::id()));
        let path = dir.join("review/1-code.md");
        create_file(&path).unwrap();
        assert!(path.exists());
        fs::write(&path, "the review").unwrap();

        // A crash-resumed run finds its own file instead of a blank one.
        create_file(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "the review");
        let _ = fs::remove_dir_all(&dir);
    }
}
