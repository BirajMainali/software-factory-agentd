//! Pure lifecycle: stage reports and human signals in, next action out.
//! No spawning, polling, or persistence here.

use super::stage::{ReviewKind, Stage, Verdict};
use serde::{Deserialize, Serialize};

/// The role a session plays in the workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "&'static str", try_from = "String")]
pub(crate) enum Role {
    SolutionArchitect,
    Developer,
    Reviewer(ReviewKind),
}

impl Role {
    /// OpenCode agent id for this role.
    pub(crate) fn agent(self) -> &'static str {
        match self {
            Self::SolutionArchitect => "solution-architect",
            Self::Developer => "developer",
            Self::Reviewer(ReviewKind::Security) => "reviewer-security",
            Self::Reviewer(ReviewKind::Code) => "reviewer-code",
            Self::Reviewer(ReviewKind::UiUx) => "reviewer-uiux",
        }
    }
}

impl From<Role> for &'static str {
    fn from(role: Role) -> Self {
        role.agent()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown role {0}")]
pub(crate) struct UnknownRole(String);

impl TryFrom<String> for Role {
    type Error = UnknownRole;

    fn try_from(agent: String) -> Result<Self, Self::Error> {
        match agent.as_str() {
            "solution-architect" => Ok(Self::SolutionArchitect),
            "developer" => Ok(Self::Developer),
            "reviewer-security" => Ok(Self::Reviewer(ReviewKind::Security)),
            "reviewer-code" => Ok(Self::Reviewer(ReviewKind::Code)),
            "reviewer-uiux" => Ok(Self::Reviewer(ReviewKind::UiUx)),
            _ => Err(UnknownRole(agent)),
        }
    }
}

/// What the worker does next. Session ids stay with the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Spawn a new session.
    Start(Role),
    /// Continue a role's session with feedback.
    Resume(Role, String),
    /// Continue the developer's session to answer a human question.
    Answer(String),
    /// Wait for a human on the pull request.
    AwaitHumanReview,
    /// Merge the reviewed commit; the workflow ends.
    Merge,
}

/// Human input from the pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Signal {
    /// Feedback restarts planning.
    Feedback { text: String },
    /// A question for the developer.
    Question { text: String },
    /// The approval label, seen while the pull request head was `head`.
    Approved { head: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Workflow {
    action: Action,
    /// Commit all three reviews passed on. Only this commit can be approved.
    reviewed: Option<String>,
}

impl Default for Workflow {
    fn default() -> Self {
        Self {
            action: Action::Start(Role::SolutionArchitect),
            reviewed: None,
        }
    }
}

impl Workflow {
    /// The action currently in flight.
    pub(crate) fn next(&self) -> &Action {
        &self.action
    }

    pub(crate) fn reviewed(&self) -> Option<&str> {
        self.reviewed.as_deref()
    }

    /// Apply a stage reported at commit `head`. `None` when it does not follow
    /// the current action (stale or duplicated); state is then unchanged.
    pub(crate) fn advance(&mut self, stage: Stage, head: &str) -> Option<&Action> {
        let next = match (&self.action, stage) {
            (
                Action::Start(Role::SolutionArchitect) | Action::Resume(Role::SolutionArchitect, _),
                Stage::Planning,
            ) => Action::Start(Role::Developer),
            (
                Action::Start(Role::Developer) | Action::Resume(Role::Developer, _),
                Stage::Development,
            ) => Action::Start(Role::Reviewer(ReviewKind::Security)),
            (Action::Answer(_), Stage::Development) => {
                if self.reviewed.as_deref() == Some(head) {
                    Action::AwaitHumanReview
                } else {
                    Action::Start(Role::Reviewer(ReviewKind::Security))
                }
            }
            (Action::Start(Role::Reviewer(current)), Stage::Review(kind, verdict))
                if *current == kind =>
            {
                match (kind, verdict) {
                    (ReviewKind::Security, Verdict::Completed) => {
                        Action::Start(Role::Reviewer(ReviewKind::Code))
                    }
                    (ReviewKind::Code, Verdict::Completed) => {
                        Action::Start(Role::Reviewer(ReviewKind::UiUx))
                    }
                    (ReviewKind::UiUx, Verdict::Completed) => {
                        self.reviewed = Some(head.to_string());
                        Action::AwaitHumanReview
                    }
                    (_, Verdict::ChangeRequested(text) | Verdict::NeedMoreEvidence(text)) => {
                        Action::Resume(Role::Developer, text)
                    }
                }
            }
            _ => return None,
        };
        self.action = next;
        Some(&self.action)
    }

    /// Apply a human signal. Only accepted while awaiting human review; approval
    /// only for the reviewed commit.
    pub(crate) fn signal(&mut self, signal: Signal) -> Option<&Action> {
        if self.action != Action::AwaitHumanReview {
            return None;
        }
        self.action = match signal {
            Signal::Feedback { text } => Action::Resume(Role::SolutionArchitect, text),
            Signal::Question { text } => Action::Answer(text),
            Signal::Approved { head } if self.reviewed.as_deref() == Some(head.as_str()) => {
                Action::Merge
            }
            Signal::Approved { .. } => return None,
        };
        Some(&self.action)
    }
}
