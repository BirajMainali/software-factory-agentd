//! The route every issue takes: a fixed sequence of tasks, held as data. The router
//! runs them in order; agents never choose what runs next.

use serde::{Deserialize, Serialize};

/// The role an agent session plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "&'static str", try_from = "String")]
pub(crate) enum Role {
    SolutionArchitect,
    Developer,
    Reviewer(ReviewKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ReviewKind {
    Security,
    Code,
    UiUx,
}

/// What a step asks of its agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Task {
    Plan,
    Develop,
    /// Review the work and write the review to a file.
    Review(ReviewKind),
    /// The developer reads a finished review and acts on it.
    ApplyReview(ReviewKind),
    /// All reviews are done; the developer opens the pull request.
    OpenPullRequest,
}

/// Every task, every time: plan, develop, each review followed by the developer,
/// then the developer opens the pull request.
pub(crate) const SEQUENCE: [Task; 9] = [
    Task::Plan,
    Task::Develop,
    Task::Review(ReviewKind::Security),
    Task::ApplyReview(ReviewKind::Security),
    Task::Review(ReviewKind::Code),
    Task::ApplyReview(ReviewKind::Code),
    Task::Review(ReviewKind::UiUx),
    Task::ApplyReview(ReviewKind::UiUx),
    Task::OpenPullRequest,
];

impl Task {
    /// The role that does this task.
    pub(crate) fn role(self) -> Role {
        match self {
            Self::Plan => Role::SolutionArchitect,
            Self::Develop | Self::ApplyReview(_) | Self::OpenPullRequest => Role::Developer,
            Self::Review(kind) => Role::Reviewer(kind),
        }
    }

    /// Whether the step starts a fresh agent session; otherwise it continues the
    /// role's existing one.
    pub(crate) fn starts_session(self) -> bool {
        !matches!(self, Self::ApplyReview(_) | Self::OpenPullRequest)
    }

    /// Short machine-readable name for status display.
    pub(crate) fn status(self) -> &'static str {
        match self {
            Self::Plan => "planning",
            Self::Develop => "developing",
            Self::Review(ReviewKind::Security) => "security_review",
            Self::Review(ReviewKind::Code) => "code_review",
            Self::Review(ReviewKind::UiUx) => "uiux_review",
            Self::ApplyReview(ReviewKind::Security) => "applying_security_review",
            Self::ApplyReview(ReviewKind::Code) => "applying_code_review",
            Self::ApplyReview(ReviewKind::UiUx) => "applying_uiux_review",
            Self::OpenPullRequest => "opening_pull_request",
        }
    }
}

impl ReviewKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Security => "security",
            Self::Code => "code",
            Self::UiUx => "uiux",
        }
    }
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
