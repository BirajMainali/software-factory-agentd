//! The route: which step runs after which. Agents never choose what runs next.
//!
//! Every fact about a step — its agent, session, files, and successor — is one
//! row in [`def`]. Routing code reads that table instead of matching on [`Step`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Step {
    /// Every task starts here.
    #[default]
    Plan,
    /// The developer implements the plan.
    #[serde(rename = "develop_plan", alias = "develop")]
    DevelopPlan,
    /// A reviewer reviews the work and writes the review to a file.
    Review(ReviewKind),
    /// The developer develops again, with the review as its input.
    #[serde(rename = "develop_review", alias = "apply_review")]
    DevelopReview(ReviewKind),
    /// All reviews are done; the developer opens the pull request.
    OpenPullRequest,
    /// The developer develops again, with the PR feedback as its input. The PR
    /// keeps being watched, so this runs once per round of feedback.
    #[serde(rename = "develop_pr_feedback", alias = "apply_pr_feedback")]
    DevelopPrFeedback,
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

/// A file the router hands an agent: the previous stage it reads, or its own output.
/// Its number is how many times that file was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Artifact {
    Issue,
    Plan,
    Review(ReviewKind),
    Development,
    PrFeedback,
}

/// Whether a step starts a session for its role or continues the role's last one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Session {
    Fresh,
    Resume,
}

/// Everything routing needs to know about one step.
pub(crate) struct StepDef {
    pub(crate) agent: Role,
    pub(crate) session: Session,
    pub(crate) reads: Option<Artifact>,
    pub(crate) writes: Artifact,
    pub(crate) next: Option<Step>,
    /// First line of the prompt, before `: <issue url>`.
    pub(crate) instruction: &'static str,
}

/// The numbered files one run uses: `read` is the previous stage, `write` its output.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Slots {
    pub(crate) read: u32,
    pub(crate) write: u32,
}

/// The one table. Adding a step means adding one row here.
pub(crate) fn def(step: Step) -> StepDef {
    match step {
        Step::Plan => StepDef {
            agent: Role::SolutionArchitect,
            session: Session::Fresh,
            reads: Some(Artifact::Issue),
            writes: Artifact::Plan,
            next: Some(Step::DevelopPlan),
            instruction: "Here is the issue that you need to plan for",
        },
        Step::DevelopPlan => StepDef {
            agent: Role::Developer,
            session: Session::Fresh,
            reads: Some(Artifact::Plan),
            writes: Artifact::Development,
            next: Some(Step::Review(ReviewKind::Security)),
            instruction: "Here is the plan that you need to implement",
        },
        Step::Review(kind) => StepDef {
            agent: Role::Reviewer(kind),
            session: Session::Fresh,
            reads: None,
            writes: Artifact::Review(kind),
            next: Some(Step::DevelopReview(kind)),
            instruction: "Here is the change that you need to review",
        },
        Step::DevelopReview(kind) => StepDef {
            agent: Role::Developer,
            session: Session::Resume,
            reads: Some(Artifact::Review(kind)),
            writes: Artifact::Development,
            next: Some(match kind {
                ReviewKind::Security => Step::Review(ReviewKind::Code),
                ReviewKind::Code => Step::Review(ReviewKind::UiUx),
                ReviewKind::UiUx => Step::OpenPullRequest,
            }),
            instruction: "Here is the review that you need to work on",
        },
        Step::OpenPullRequest => StepDef {
            agent: Role::Developer,
            session: Session::Resume,
            reads: None,
            writes: Artifact::Development,
            next: None,
            instruction: "All changes are approved, you need to open a PR for",
        },
        Step::DevelopPrFeedback => StepDef {
            agent: Role::Developer,
            session: Session::Resume,
            reads: Some(Artifact::PrFeedback),
            writes: Artifact::Development,
            next: None,
            instruction: "Here is the PR feedback that you need to work on",
        },
    }
}

impl Step {
    /// What runs once this step completes; `None` when agents are done.
    pub(crate) fn next(self) -> Option<Self> {
        def(self).next
    }

    /// The role that does this step.
    pub(crate) fn role(self) -> Role {
        def(self).agent
    }

    /// Whether the step starts a fresh agent session; otherwise it continues the
    /// role's existing one.
    pub(crate) fn starts_session(self) -> bool {
        matches!(def(self).session, Session::Fresh)
    }

    /// Every step in route order, following [`Step::next`] from the first.
    pub(crate) fn route() -> impl Iterator<Item = Self> {
        std::iter::successors(Some(Self::default()), |step| step.next())
    }
}

impl Artifact {
    /// Path inside the issue folder.
    pub(crate) fn file(self, n: u32) -> String {
        match self {
            Self::Issue => "issue.md".to_string(),
            Self::Plan => format!("plan/{n}-plan.md"),
            Self::Review(kind) => format!("review/{n}-{}.md", kind.as_str()),
            Self::Development => format!("development/{n}-development.md"),
            Self::PrFeedback => format!("pr-feedback/{n}-pr-feedback.md"),
        }
    }

    /// What the prompt calls it on the read line: `Plan: <path>`.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Issue => "Issue text".to_string(),
            Self::Plan => "Plan".to_string(),
            Self::Review(kind) => format!("Review ({})", kind.as_str()),
            Self::Development => "development summary".to_string(),
            Self::PrFeedback => "PR feedback".to_string(),
        }
    }
}

impl Artifact {
    /// What the prompt calls the file the agent writes.
    pub(crate) fn write_label(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Review(_) => "review",
            _ => "development summary",
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
    /// `OpenCode` agent id for this role.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Step {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn old_log_step_names_still_replay() {
        assert_eq!(parse(r#""develop""#), Step::DevelopPlan);
        assert_eq!(
            parse(r#"{"apply_review":"code"}"#),
            Step::DevelopReview(ReviewKind::Code)
        );
        assert_eq!(parse(r#""apply_pr_feedback""#), Step::DevelopPrFeedback);
    }

    #[test]
    fn new_logs_name_the_developer_step() {
        assert_eq!(
            serde_json::to_string(&Step::DevelopPlan).unwrap(),
            r#""develop_plan""#
        );
        assert_eq!(
            serde_json::to_string(&Step::DevelopReview(ReviewKind::Security)).unwrap(),
            r#"{"develop_review":"security"}"#
        );
    }
}
