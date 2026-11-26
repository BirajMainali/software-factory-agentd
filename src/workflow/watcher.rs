//! Pull request watcher: turns human actions on the PR into workflow signals.
//!
//! - `factory-approved` label added → approve the current PR head.
//! - Comment `/factory feedback <text>` → feedback; `/factory question <text>` → question.
//!
//! Comments count only from authors with write access to the repo; others are ignored,
//! since comment text reaches agents.

use super::machine::Signal;
use crate::github::pulls::{self, PullState};
use crate::github::{self, GitHubUrl, Label, gh_json};
use serde::{Deserialize, Serialize};

const APPROVAL_LABEL: &str = "factory-approved";
const COMMAND_PREFIX: &str = "/factory";
const TRUSTED_AUTHORS: [&str; 3] = ["OWNER", "MEMBER", "COLLABORATOR"];

/// Last comment and label event already consumed. GitHub ids only grow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cursor {
    pub(crate) comments_after: u64,
    pub(crate) labels_after: u64,
}

pub(crate) enum Poll {
    Signal { signal: Signal, cursor: Cursor },
    Merged,
    Closed,
    Waiting,
}

#[derive(Deserialize)]
struct Comment {
    id: u64,
    body: Option<String>,
    author_association: String,
}

#[derive(Deserialize)]
struct IssueEvent {
    id: u64,
    event: String,
    label: Option<Label>,
}

/// Cursor that ignores every label event so far: approvals must come after the
/// reviewed commit is pushed. Comments keep their position.
pub(crate) fn start(
    issue: &GitHubUrl,
    number: u64,
    cursor: Cursor,
) -> Result<Cursor, github::Error> {
    let labels_after = events(issue, number)?
        .iter()
        .map(|event| event.id)
        .max()
        .unwrap_or(cursor.labels_after);
    Ok(Cursor {
        labels_after,
        ..cursor
    })
}

/// The oldest unconsumed human signal, if any. Commands before approval, so
/// feedback left alongside an approval is never skipped.
pub(crate) fn poll(issue: &GitHubUrl, number: u64, cursor: Cursor) -> Result<Poll, github::Error> {
    let head = match pulls::state(issue, number)? {
        PullState::Open { head } => head,
        PullState::Merged => return Ok(Poll::Merged),
        PullState::Closed => return Ok(Poll::Closed),
    };

    let path = issue.repo_path(&format!("issues/{number}/comments"));
    let pages: Vec<Vec<Comment>> = gh_json("api", &["api", "--paginate", "--slurp", &path])?;
    let mut comments: Vec<Comment> = pages
        .into_iter()
        .flatten()
        .filter(|comment| comment.id > cursor.comments_after)
        .collect();
    comments.sort_by_key(|comment| comment.id);
    for comment in comments {
        if !TRUSTED_AUTHORS.contains(&comment.author_association.as_str()) {
            continue;
        }
        if let Some(signal) = comment.body.as_deref().and_then(parse_command) {
            let cursor = Cursor {
                comments_after: comment.id,
                ..cursor
            };
            return Ok(Poll::Signal { signal, cursor });
        }
    }

    let approval = events(issue, number)?
        .into_iter()
        .filter(|event| event.id > cursor.labels_after && event.event == "labeled")
        .filter(|event| {
            event
                .label
                .as_ref()
                .is_some_and(|label| label.name == APPROVAL_LABEL)
        })
        .map(|event| event.id)
        .max();
    Ok(match approval {
        Some(labels_after) => Poll::Signal {
            signal: Signal::Approved { head },
            cursor: Cursor {
                labels_after,
                ..cursor
            },
        },
        None => Poll::Waiting,
    })
}

fn events(issue: &GitHubUrl, number: u64) -> Result<Vec<IssueEvent>, github::Error> {
    let path = issue.repo_path(&format!("issues/{number}/events"));
    let pages: Vec<Vec<IssueEvent>> = gh_json("api", &["api", "--paginate", "--slurp", &path])?;
    Ok(pages.into_iter().flatten().collect())
}

fn parse_command(body: &str) -> Option<Signal> {
    let rest = body.trim().strip_prefix(COMMAND_PREFIX)?;
    let (verb, text) = rest.trim_start().split_once(char::is_whitespace)?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    match verb {
        "feedback" => Some(Signal::Feedback { text }),
        "question" => Some(Signal::Question { text }),
        _ => None,
    }
}
