//! The route: which step runs after which. Agents never choose what runs next.
//!
//! Every fact about a step — its agent, session, files, and successor — is one
//! row in [`def`]. Routing code reads that table instead of matching on [`Step`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Step {
    #[default]
    Plan,
    #[serde(rename = "develop", alias = "develop_plan")]
    Develop,
    Review(ReviewKind),
    #[serde(rename = "develop_review", alias = "apply_review")]
    DevelopReview(ReviewKind),
    /// The three reviews are in; the developer addresses them in one pass.
    AddressReviews,
    OpenPullRequest,
    /// Selected by the PR watch, once per round of new feedback.
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
    Planner,
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

/// What a step reads before it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reads {
    None,
    One(Artifact),
    /// The latest review of each kind, collected before the developer runs.
    AllReviews,
}

pub(crate) struct StepDef {
    pub(crate) agent: Role,
    pub(crate) session: Session,
    pub(crate) reads: Reads,
    pub(crate) writes: Artifact,
    pub(crate) next: Option<Step>,
    /// First line of the prompt, before `: <issue url>`.
    pub(crate) instruction: &'static str,
}

/// The numbered files one run uses: the stages it reads, and its output.
#[derive(Debug, Clone)]
pub(crate) struct Slots {
    pub(crate) reads: Vec<(Artifact, u32)>,
    pub(crate) write: u32,
}

impl Slots {
    pub(crate) fn read(&self, artifact: Artifact) -> u32 {
        self.reads
            .iter()
            .find(|(read, _)| *read == artifact)
            .map_or(1, |(_, n)| *n)
    }
}

/// The one table. Adding a step means adding one row here.
pub(crate) fn def(step: Step) -> StepDef {
    match step {
        Step::Plan => StepDef {
            agent: Role::Planner,
            session: Session::Fresh,
            reads: Reads::One(Artifact::Issue),
            writes: Artifact::Plan,
            next: Some(Step::Develop),
            instruction: "Here is the issue that you need to plan for",
        },
        Step::Develop => StepDef {
            agent: Role::Developer,
            session: Session::Fresh,
            reads: Reads::One(Artifact::Plan),
            writes: Artifact::Development,
            // The PR label decides what happens next; the route has nothing left.
            next: None,
            instruction: "Here is the plan that you need to implement",
        },
        Step::Review(kind) => StepDef {
            agent: Role::Reviewer(kind),
            session: Session::Fresh,
            reads: Reads::None,
            writes: Artifact::Review(kind),
            next: Some(match kind {
                ReviewKind::Security => Step::Review(ReviewKind::Code),
                ReviewKind::Code => Step::Review(ReviewKind::UiUx),
                ReviewKind::UiUx => Step::AddressReviews,
            }),
            instruction: "Here is the change that you need to review",
        },
        Step::AddressReviews => StepDef {
            agent: Role::Developer,
            session: Session::Resume,
            reads: Reads::AllReviews,
            writes: Artifact::Development,
            next: Some(Step::OpenPullRequest),
            instruction: "Here are the reviews that you need to work on together",
        },
        // Kept so task logs written before reviews were collected still replay.
        Step::DevelopReview(kind) => StepDef {
            agent: Role::Developer,
            session: Session::Resume,
            reads: Reads::One(Artifact::Review(kind)),
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
            reads: Reads::None,
            writes: Artifact::Development,
            next: None,
            instruction: "All reviews are addressed, label the pull request ready for",
        },
        Step::DevelopPrFeedback => StepDef {
            agent: Role::Developer,
            session: Session::Resume,
            reads: Reads::One(Artifact::PrFeedback),
            writes: Artifact::Development,
            // Back to the watch once the developer has pushed and labeled ready.
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

    pub(crate) fn role(self) -> Role {
        def(self).agent
    }

    /// The short name the dashboard shows for this step.
    pub(crate) fn stage_label(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Develop
            | Self::AddressReviews
            | Self::DevelopPrFeedback
            | Self::DevelopReview(_) => "Develop",
            Self::Review(_) => "Review",
            Self::OpenPullRequest => "PR",
        }
    }

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
    pub(crate) const ALL: [Self; 3] = [Self::Security, Self::Code, Self::UiUx];

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
            Self::Planner => "planner",
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
        assert_eq!(parse(r#""develop_plan""#), Step::Develop);
        assert_eq!(parse(r#""develop""#), Step::Develop);
        assert_eq!(
            parse(r#"{"apply_review":"code"}"#),
            Step::DevelopReview(ReviewKind::Code)
        );
        assert_eq!(parse(r#""apply_pr_feedback""#), Step::DevelopPrFeedback);
    }

    #[test]
    fn the_route_is_plan_then_develop() {
        // The PR label, not the route, decides what happens after develop.
        assert_eq!(
            Step::route().collect::<Vec<_>>(),
            vec![Step::Plan, Step::Develop]
        );
    }

    #[test]
    fn a_feedback_round_ends_with_the_developer() {
        assert_eq!(Step::DevelopPrFeedback.next(), None);
    }

    #[test]
    fn new_logs_name_the_developer_step() {
        assert_eq!(
            serde_json::to_string(&Step::Develop).unwrap(),
            r#""develop""#
        );
        assert_eq!(
            serde_json::to_string(&Step::DevelopReview(ReviewKind::Security)).unwrap(),
            r#"{"develop_review":"security"}"#
        );
    }
}
