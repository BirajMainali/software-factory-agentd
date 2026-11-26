//! What happened to a task. The log of these is the durable record; all task state
//! is derived by replaying it.

use super::sequence::Role;
use crate::opencode::Outcome;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum Event {
    WorkerStarted,
    /// The task's main opencode session.
    SessionOpened {
        session: String,
    },
    /// The router created `role`'s sub-agent session under the main session.
    SubagentStarted {
        role: Role,
        session: String,
        parent: String,
    },
    /// Recorded before prompting `session` for sequence step `step`, with the
    /// session's last finished run before it (`after`).
    TurnStarted {
        step: usize,
        role: Role,
        session: String,
        after: Option<String>,
    },
    /// opencode reported the sub-agent's run ended.
    SubagentFinished {
        role: Role,
        session: String,
        outcome: Outcome,
    },
    /// Sequence step `step` is done; the next one runs.
    StepCompleted {
        step: usize,
    },
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
