//! One worker per task. Replays the task's event log, then drives agents, the pull
//! request, and the merge until the workflow ends. Holds the task lock throughout,
//! so two workers never share a task, worktree, or branch.

mod agents;
mod prompts;
mod pull_request;

use super::event::Event;
use super::log::{self, Log};
use super::machine::{Action, Role};
use super::report;
use super::state::TaskState;
use crate::github::{self, GitHubUrl};
use crate::home;
use crate::opencode::{self, Client, Outcome};
use crate::project::{self, Worktree};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    GitHub(#[from] github::Error),
    #[error(transparent)]
    Home(#[from] home::Error),
    #[error(transparent)]
    Project(#[from] project::Error),
    #[error(transparent)]
    Log(#[from] log::Error),
    #[error(transparent)]
    Agent(#[from] opencode::Error),
    #[error("another worker is running {0}")]
    Busy(String),
    #[error(transparent)]
    Report(#[from] report::Error),
    #[error("the developer's report has no answer text for the question")]
    NoAnswer,
    #[error("agent {} reported a stage that does not follow the current action", .0.agent())]
    StaleStage(Role),
    #[error("agent {} run ended {outcome:?} in opencode", .role.agent())]
    SubagentEnded { role: Role, outcome: Outcome },
    #[error("no session for agent {}", .0.agent())]
    NoSession(Role),
    #[error("agent {} changed the worktree during review", .0.agent())]
    ReviewerChangedCode(Role),
    #[error("the workflow needs a pull request but none was opened")]
    NoPullRequest,
    #[error("merge requested but no commit passed review")]
    NothingReviewed,
    #[error("the pull request was closed without merging")]
    PullRequestClosed,
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub(crate) struct Worker<'a> {
    issue: GitHubUrl,
    title: String,
    worktree: Worktree,
    opencode: Client,
    log: &'a mut Log,
    state: TaskState,
}

impl Worker<'_> {
    /// Run the issue at `url` until its pull request merges, resuming from the
    /// event log. Failures are recorded in the log before returning.
    pub(crate) fn run(url: &str) -> Result<(), Error> {
        let issue: GitHubUrl = url.parse()?;
        issue.expect_issue()?;
        let home = home::home()?;
        let dir = home::task_dir(&home, &issue);
        let Some(_lock) = home::try_lock(&home::task_lock_path(&dir))? else {
            return Err(Error::Busy(issue.to_string()));
        };

        let path = home::events_path(&dir);
        let mut state = TaskState::replay(&log::read(&path)?);
        if state.merged {
            return Ok(());
        }
        let mut log = Log::open(&path)?;
        state.apply(&log.append(Event::WorkerStarted)?);

        let result = Self::drive(issue, &home, &mut log, state);
        if let Err(error) = &result {
            // The worker error is what matters; failing to log it must not hide it.
            let _ = log.append(Event::WorkerFailed {
                reason: error.to_string(),
            });
        }
        result
    }

    fn drive(issue: GitHubUrl, home: &Path, log: &mut Log, state: TaskState) -> Result<(), Error> {
        let opencode = Client::connect()?;
        let worktree = Worktree::prepare(home, &issue)?;
        let fetched = github::issue::fetch(&issue)?;
        write_file(&worktree.factory_file("issue.md"), &fetched.markdown)?;

        let mut worker = Worker {
            issue,
            title: fetched.title,
            worktree,
            opencode,
            log,
            state,
        };
        if worker.state.branch.as_deref() != Some(worker.worktree.branch.as_str()) {
            worker.record(Event::WorktreeReady {
                path: worker.worktree.path.clone(),
                branch: worker.worktree.branch.clone(),
            })?;
        }
        worker.run_workflow()
    }

    fn run_workflow(&mut self) -> Result<(), Error> {
        while !self.state.merged {
            if let Some(answer) = self.state.pending_answer.clone() {
                self.post_answer(&answer)?;
                continue;
            }
            match self.state.workflow.next().clone() {
                Action::Start(role) => self.start_subagent(role)?,
                Action::Resume(role, text) => {
                    self.resume_subagent(role, &prompts::resume(role, &text))?;
                }
                Action::Answer(text) => {
                    self.resume_subagent(Role::Developer, &prompts::answer(&text))?;
                }
                Action::AwaitHumanReview => self.await_human()?,
                Action::Merge => self.merge()?,
            }
        }
        Ok(())
    }

    fn record(&mut self, event: Event) -> Result<(), Error> {
        let record = self.log.append(event)?;
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
