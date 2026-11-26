//! What happened to a task. The log of these is the durable record; all task state
//! is derived by replaying it.

use super::machine::{Role, Signal};
use super::stage::Stage;
use super::watcher::Cursor;
use crate::github::pulls::PullRequest;
use crate::opencode::Outcome;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum Event {
    WorkerStarted,
    WorktreeReady {
        path: PathBuf,
        branch: String,
    },
    /// The task's main opencode session, recorded as soon as its id is known.
    SessionOpened {
        session: String,
    },
    /// Recorded before prompting `session` for `role`: the commit the turn starts
    /// from, and the session's last finished run before it (`after`).
    TurnStarted {
        role: Role,
        head: String,
        session: String,
        after: Option<String>,
    },
    /// The runtime created `role`'s sub-agent session under the main session.
    SubagentStarted {
        role: Role,
        session: String,
        parent: String,
    },
    /// opencode reported the sub-agent's run ended.
    SubagentFinished {
        role: Role,
        session: String,
        outcome: Outcome,
    },
    StageReported {
        role: Role,
        session: String,
        head: String,
        stage: Stage,
    },
    /// The developer's answer to a human question, not yet on the pull request.
    AnswerWritten {
        text: String,
    },
    AnswerPosted,
    Pushed {
        head: String,
    },
    PullRequestOpened {
        pull_request: PullRequest,
    },
    /// Started watching the pull request; only signals after `cursor` count.
    Watching {
        cursor: Cursor,
    },
    SignalReceived {
        signal: Signal,
        cursor: Cursor,
    },
    Merged,
    WorkerFailed {
        reason: String,
    },
    /// The runtime saw the worker process exit.
    WorkerExited {
        code: Option<i32>,
    },
}

/// One log line: an event and when it was recorded (Unix seconds).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Record {
    pub(crate) ts: u64,
    #[serde(flatten)]
    pub(crate) event: Event,
}
