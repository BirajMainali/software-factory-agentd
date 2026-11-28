//! Owns one work item from submit until its pull request closes. The route runs
//! first; then this process watches the PR, resumes the developer on new feedback,
//! and exits when the PR merges or closes. Issue discovery and priority belong to
//! the caller: `work --issue` is the only entry point.
//!
//! Holds the task lock for the whole lifetime, so a second `work` for the same
//! issue is refused while the first is routing or watching.

use crate::github::{self, GitHubUrl};
use crate::home;
use crate::workflow::{self, Event, Log, TaskState, Worker};
use std::path::Path;
use std::thread;
use std::time::Duration;

const WATCH_INTERVAL: Duration = Duration::from_secs(60);
/// Wait for GitHub to show a PR the developer just opened.
const PR_RETRY: Duration = Duration::from_secs(10);
const PR_TRIES: u32 = 30;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    GitHub(#[from] github::Error),
    #[error(transparent)]
    Home(#[from] home::Error),
    #[error(transparent)]
    Log(#[from] workflow::LogError),
    #[error(transparent)]
    Task(#[from] workflow::WorkerError),
    #[error("another process already owns {0}")]
    Busy(String),
    #[error("{0} is stuck after repeated failures")]
    Stuck(String),
    #[error("no pull request found for {0}")]
    NoPullRequest(String),
}

enum Watch {
    /// New feedback was recorded; run the developer.
    Feedback,
    /// The PR merged or closed; the task is terminal.
    Closed,
}

pub(crate) fn supervise(issue: &str, path: &Path) -> Result<(), Error> {
    let url: GitHubUrl = issue.parse()?;
    url.expect_issue()?;
    let home = home::home()?;
    let dir = home::task_dir(&home, &url);
    let _lock =
        home::try_lock(&home::task_lock_path(&dir))?.ok_or_else(|| Error::Busy(url.to_string()))?;

    loop {
        let state = replay_state(&home, &url)?;
        if state.pr_closed {
            return Ok(());
        }
        if state.is_finished() {
            let pr = ensure_pr(&home, &url, &state)?;
            // Replay so the just-recorded PR and its baseline key are visible.
            let state = replay_state(&home, &url)?;
            match watch(&home, &url, &pr, &state)? {
                Watch::Feedback => continue,
                Watch::Closed => return Ok(()),
            }
        }
        if state.is_stuck() {
            return Err(Error::Stuck(url.to_string()));
        }

        match Worker::run(issue, path) {
            Ok(()) => append(&home, &url, Event::WorkerExited { code: Some(0) })?,
            Err(error) => {
                append(&home, &url, Event::WorkerExited { code: Some(1) })?;
                let state = replay_state(&home, &url)?;
                if state.is_stuck() {
                    return Err(Error::Task(error));
                }
                let now = workflow::now();
                if let Some(at) = state.retry_at() {
                    thread::sleep(Duration::from_secs(at.saturating_sub(now)));
                }
            }
        }
    }
}

/// The task's PR: from the log when known, otherwise found through the issue
/// timeline. The baseline key is seeded only when the PR has no feedback yet, so
/// comments that arrived before we found it are still worked on.
fn ensure_pr(home: &Path, issue: &GitHubUrl, state: &TaskState) -> Result<GitHubUrl, Error> {
    if let Some(url) = state.pr() {
        return Ok(url.parse()?);
    }
    for _ in 0..PR_TRIES {
        // Transient gh failures keep the supervisor alive; only a missing PR gives up.
        if let Some(pr) = github::pr::for_issue(issue).ok().flatten()
            && let Ok(snapshot) = github::pr::fetch(&pr)
        {
            let key = (snapshot.feedback == 0).then_some(snapshot.key);
            append(
                home,
                issue,
                Event::PullRequestFound {
                    url: pr.to_string(),
                    key,
                },
            )?;
            return Ok(pr);
        }
        thread::sleep(PR_RETRY);
    }
    Err(Error::NoPullRequest(issue.to_string()))
}

/// Poll the PR until it closes or has feedback the task has not seen.
fn watch(
    home: &Path,
    issue: &GitHubUrl,
    pr: &GitHubUrl,
    state: &TaskState,
) -> Result<Watch, Error> {
    loop {
        // A transient gh failure must not end the watch; try again next tick.
        let Ok(snapshot) = github::pr::fetch(pr) else {
            thread::sleep(WATCH_INTERVAL);
            continue;
        };
        if snapshot.has_label(github::pr::APPROVED_LABEL) {
            if snapshot.mergeable == Some(true) && github::pr::merge(pr).is_ok() {
                append(home, issue, Event::PullRequestClosed)?;
                return Ok(Watch::Closed);
            }
            if snapshot.mergeable.is_some() {
                // Conflicts, or a merge GitHub refused: the developer fixes it.
                let _ = github::pr::label(pr, github::pr::APPROVED_LABEL, false);
                let _ = github::pr::label(pr, github::pr::READY_LABEL, false);
                let number = state.next_pr_feedback();
                append(
                    home,
                    issue,
                    Event::PrFeedbackArrived {
                        number,
                        key: snapshot.key,
                    },
                )?;
                return Ok(Watch::Feedback);
            }
            // GitHub has not computed mergeability yet; retry next tick.
        }
        if !snapshot.open {
            append(home, issue, Event::PullRequestClosed)?;
            return Ok(Watch::Closed);
        }
        if state.feedback_key() != Some(snapshot.key.as_str()) {
            // This round is being worked on again: it is no longer ready to review.
            if snapshot.has_label(github::pr::READY_LABEL) {
                let _ = github::pr::label(pr, github::pr::READY_LABEL, false);
            }
            let number = state.next_pr_feedback();
            append(
                home,
                issue,
                Event::PrFeedbackArrived {
                    number,
                    key: snapshot.key,
                },
            )?;
            return Ok(Watch::Feedback);
        }
        thread::sleep(WATCH_INTERVAL);
    }
}

fn replay_state(home: &Path, issue: &GitHubUrl) -> Result<TaskState, Error> {
    let dir = home::task_dir(home, issue);
    Ok(TaskState::replay(&workflow::read_log(&home::events_path(
        &dir,
    ))?))
}

fn append(home: &Path, issue: &GitHubUrl, event: Event) -> Result<(), Error> {
    let dir = home::task_dir(home, issue);
    Log::open(&home::events_path(&dir))?.append(event)?;
    Ok(())
}
