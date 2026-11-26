//! Pure lifecycle: stage events and external signals in, next action out.
//! No spawning, polling, or persistence here.
//!
//! The action in flight is the only state: it says which session runs now.

use super::{CodeReview, Phase, Review, Stage};

/// The role a session plays in the workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    SolutionArchitect,
    Developer,
    Reviewer(ReviewKind),
}

/// Sequential reviewer order: security, then code, then ui/ux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewKind {
    Security,
    Code,
    UiUx,
}

/// What the runner should do next. Session ids stay with the caller.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Spawn a new session.
    Start(Role),
    /// Continue an existing session with feedback.
    Resume(Role, String),
    /// Hand the PR to a human and wait.
    AwaitHumanReview,
    /// Resume development, tell it to merge; the workflow ends.
    Merge,
}

impl Default for Action {
    fn default() -> Self {
        Self::Start(Role::SolutionArchitect)
    }
}

/// External input from the PR watcher.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    /// Human or PR feedback; both restart planning.
    Feedback(String),
    /// A review question for the developer to answer.
    Question(String),
    /// The `factory-approved` label.
    Approved,
}

#[derive(Debug, Default)]
pub(crate) struct Workflow {
    action: Action,
}

impl Workflow {
    /// Apply a stage event. `None` when the event is stale or duplicated.
    pub(crate) fn advance(&mut self, stage: Stage) -> Option<&Action> {
        let next = match (&self.action, stage) {
            (
                Action::Start(Role::SolutionArchitect) | Action::Resume(Role::SolutionArchitect, _),
                Stage::Planning(Phase::Completed),
            ) => Action::Start(Role::Developer),
            (
                Action::Start(Role::Developer) | Action::Resume(Role::Developer, _),
                Stage::Development(Phase::Completed),
            ) => Action::Start(Role::Reviewer(ReviewKind::Security)),
            (
                Action::Start(Role::Reviewer(ReviewKind::Security)),
                Stage::Review(Review::Security(Phase::Completed)),
            ) => Action::Start(Role::Reviewer(ReviewKind::Code)),
            (
                Action::Start(Role::Reviewer(ReviewKind::Code)),
                Stage::Review(Review::Code(CodeReview::Completed)),
            ) => Action::Start(Role::Reviewer(ReviewKind::UiUx)),
            (
                Action::Start(Role::Reviewer(ReviewKind::UiUx)),
                Stage::Review(Review::UiUx(CodeReview::Completed)),
            ) => Action::AwaitHumanReview,
            (
                Action::Start(Role::Reviewer(_)),
                Stage::Review(Review::Code(change) | Review::UiUx(change)),
            ) => Action::Resume(Role::Developer, change_text(change)?),
            _ => return None,
        };
        self.action = next;
        Some(&self.action)
    }

    /// Apply a PR watcher signal. Ignored once the workflow is done.
    pub(crate) fn signal(&mut self, signal: Signal) -> Option<&Action> {
        if self.action == Action::Merge {
            return None;
        }
        self.action = match signal {
            Signal::Feedback(text) => Action::Resume(Role::SolutionArchitect, text),
            Signal::Question(text) => Action::Resume(Role::Developer, text),
            Signal::Approved => Action::Merge,
        };
        Some(&self.action)
    }
}

fn change_text(change: CodeReview) -> Option<String> {
    match change {
        CodeReview::ChangeRequested(text) | CodeReview::NeedMoreEvidence(text) => Some(text),
        CodeReview::Started | CodeReview::Completed => None,
    }
}
