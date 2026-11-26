//! What happened to a task. The log of these is the durable record; all task state
//! is derived by replaying it.

use super::step::Step;
use crate::opencode::Outcome;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum Event {
    WorkerStarted,
    /// The issue's title, recorded when first seen or changed.
    IssueFetched {
        title: String,
    },
    /// The task's main opencode session.
    SessionOpened {
        session: String,
    },
    /// The router created step `step`'s sub-agent session under the main session.
    SubagentStarted {
        step: Step,
        session: String,
        parent: String,
    },
    /// Recorded before prompting `session` for `step`, with the
    /// session's last finished run before it (`after`).
    TurnStarted {
        step: Step,
        session: String,
        after: Option<String>,
    },
    /// opencode reported step `step`'s run ended.
    SubagentFinished {
        step: Step,
        outcome: Outcome,
    },
    /// `step` is done; [`Step::next`] runs.
    StepCompleted {
        step: Step,
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
