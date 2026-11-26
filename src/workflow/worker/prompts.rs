//! What the router sends agents: task data and file paths, relative to the
//! workspace under `$FACTORY_DIR`. How to do the work lives in the agent files.

use crate::github::GitHubUrl;
use crate::workflow::sequence::{ReviewKind, Task};

pub(super) const ISSUE_FILE: &str = "issue.md";
const PLAN_FILE: &str = "plan.md";

/// The prompt for one sequence step.
pub(super) fn for_task(task: Task, issue: &GitHubUrl, dir: &str) -> String {
    match task {
        Task::Plan => format!(
            "Here is the issue that you need to plan for: {issue}\n\
             Issue text: {dir}/{ISSUE_FILE}\n\
             Write the plan to: {dir}/{PLAN_FILE}"
        ),
        Task::Develop => format!(
            "Here is the {PLAN_FILE} that you need to implement: {dir}/{PLAN_FILE}\n\
             Issue: {issue}"
        ),
        Task::Review(kind) => format!(
            "Here is the change that you need to review: {issue}\n\
             Review: {}\n\
             Write your review to: {}",
            kind.as_str(),
            review_file(dir, kind)
        ),
        Task::ApplyReview(kind) => format!(
            "Here is the PR feedback that you need to work on: {}",
            review_file(dir, kind)
        ),
        Task::OpenPullRequest => {
            format!("All changes are approved, you need to open a PR for: {issue}")
        }
    }
}

fn review_file(dir: &str, kind: ReviewKind) -> String {
    format!("{dir}/review-{}.md", kind.as_str())
}
