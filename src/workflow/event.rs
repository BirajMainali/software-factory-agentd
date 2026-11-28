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
    SessionOpened {
        session: String,
    },
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
    SubagentFinished {
        step: Step,
        outcome: Outcome,
    },
    /// The PR is labeled ready: stop the route and wait for feedback.
    PrReady,
    /// `step` is done; [`Step::next`] runs.
    StepCompleted {
        step: Step,
        /// The step that ran next when this was recorded. Absent in logs written
        /// before the field existed, which replay with the old route instead.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<Step>,
    },
    /// The folder this worker writes artifacts to, recorded when it starts.
    WorkPath {
        path: String,
    },
    /// The task's pull request, found after the route opens it. `key` is the
    /// feedback fingerprint already present; `None` means it must be worked on.
    PullRequestFound {
        url: String,
        key: Option<String>,
    },
    /// The PR was merged or closed; the task is terminal.
    PullRequestClosed,
    /// The PR watch found feedback newer than `key`, which is the next numbered
    /// `pr-feedback` file for the developer to work on.
    PrFeedbackArrived {
        number: u32,
        key: String,
    },
    WorkerFailed {
        reason: String,
    },
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
