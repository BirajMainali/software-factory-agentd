//! Workflow events. Every stage runs in its own session under the main session.
//! Not wired into the runtime yet.
#![allow(dead_code)]

mod machine;

pub(crate) struct Event {
    pub(crate) session_id: String,
    pub(crate) parent_session_id: String,
    pub(crate) stage: Stage,
}

pub(crate) enum Stage {
    Planning(Phase),
    Development(Phase),
    Review(Review),
}

pub(crate) enum Review {
    Security(Phase),
    Code(CodeReview),
    UiUx(CodeReview),
}

pub(crate) enum Phase {
    Started,
    Completed,
}

pub(crate) enum CodeReview {
    Started,
    Completed,
    ChangeRequested(String),
    NeedMoreEvidence(String),
}
