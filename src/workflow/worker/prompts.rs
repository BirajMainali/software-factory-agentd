//! What the router sends agents: task data and file paths, relative to the
//! workspace under `$FACTORY_DIR`. How to do the work lives in the agent files.

use crate::github::GitHubUrl;
use crate::workflow::step::{ReviewKind, Step};

pub(super) const ISSUE_FILE: &str = "issue.md";
const PLAN_FILE: &str = "plan.md";

/// The prompt for one step.
pub(super) fn for_step(step: Step, issue: &GitHubUrl, dir: &str) -> String {
    match step {
        Step::Plan => format!(
            "Here is the issue that you need to plan for: {issue}\n\
             Issue text: {dir}/{ISSUE_FILE}\n\
             Write the plan to: {dir}/{PLAN_FILE}"
        ),
        Step::Develop => format!(
            "Here is the {PLAN_FILE} that you need to implement: {dir}/{PLAN_FILE}\n\
             Issue: {issue}"
        ),
        Step::Review(kind) => format!(
            "Here is the change that you need to review: {issue}\n\
             Review: {}\n\
             Write your review to: {}",
            kind.as_str(),
            review_file(dir, kind)
        ),
        Step::ApplyReview(kind) => format!(
            "Here is the PR feedback that you need to work on: {}",
            review_file(dir, kind)
        ),
        Step::OpenPullRequest => {
            format!("All changes are approved, you need to open a PR for: {issue}")
        }
    }
}

fn review_file(dir: &str, kind: ReviewKind) -> String {
    format!("{dir}/review-{}.md", kind.as_str())
}
