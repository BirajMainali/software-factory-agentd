//! The route: which step runs after which. Agents never choose what runs next.

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Step {
    /// Every task starts here.
    #[default]
    Plan,
    Develop,
    /// A reviewer reviews the work and writes the review to a file.
    Review(ReviewKind),
    /// The developer reads a finished review and acts on it.
    ApplyReview(ReviewKind),
    /// All reviews are done; the developer opens the pull request.
    OpenPullRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewKind {
    Security,
    Code,
    #[serde(rename = "uiux")]
    UiUx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Role {
    SolutionArchitect,
    Developer,
    Reviewer(ReviewKind),
}

impl Step {
    /// What runs once this step completes; `None` when agents are done and a human
    /// takes over.
    pub(crate) fn next(self) -> Option<Self> {
        match self {
            Self::Plan => Some(Self::Develop),
            Self::Develop => Some(Self::Review(ReviewKind::Security)),
            // A review is done: the developer works on it.
            Self::Review(kind) => Some(Self::ApplyReview(kind)),
            // The developer worked on a review: the next review, or the pull request.
            Self::ApplyReview(ReviewKind::Security) => Some(Self::Review(ReviewKind::Code)),
            Self::ApplyReview(ReviewKind::Code) => Some(Self::Review(ReviewKind::UiUx)),
            Self::ApplyReview(ReviewKind::UiUx) => Some(Self::OpenPullRequest),
            Self::OpenPullRequest => None,
        }
    }

    /// The role that does this step.
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

    /// Every step in route order, following [`Step::next`] from the first.
    pub(crate) fn route() -> impl Iterator<Item = Self> {
        std::iter::successors(Some(Self::default()), |step| step.next())
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
