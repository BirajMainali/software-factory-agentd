//! What the runtime sends agents: task data only. How to do the work lives in the
//! agent files.

use crate::github::GitHubUrl;
use crate::project::FACTORY_DIR;
use crate::workflow::machine::Role;

/// Task data for a role's first turn.
pub(super) fn start(issue: &GitHubUrl, role: Role) -> String {
    match role {
        Role::SolutionArchitect => format!(
            "Issue: {issue}\nIssue text: {FACTORY_DIR}/issue.md\nPlan: {FACTORY_DIR}/plan.md"
        ),
        Role::Developer => format!("Issue: {issue}\nPlan: {FACTORY_DIR}/plan.md"),
        Role::Reviewer(kind) => format!(
            "Issue: {issue}\nReview: {}\nChanges: git diff origin/HEAD...HEAD",
            kind.as_str()
        ),
    }
}

/// Follow-up for a role's existing session.
pub(super) fn resume(role: Role, text: &str) -> String {
    match role {
        Role::SolutionArchitect => format!("Pull request feedback:\n{text}"),
        Role::Developer | Role::Reviewer(_) => format!("Review requested changes:\n{text}"),
    }
}

/// A human question for the developer.
pub(super) fn answer(question: &str) -> String {
    format!("Pull request question:\n{question}")
}
