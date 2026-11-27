//! What the router sends agents: one sentence, the file to read, the file to write.
//! Paths are relative to the workspace under `$FACTORY_DIR`. How to do the work is
//! the agent's.

use crate::github::GitHubUrl;
use crate::workflow::step::{Artifact, Slots, Step, def};

pub(super) const ISSUE_FILE: &str = "issue.md";

/// The prompt for one step: what it reads, where it writes.
pub(super) fn for_step(step: Step, issue: &GitHubUrl, dir: &str, slots: Slots) -> String {
    let def = def(step);
    let mut lines = vec![format!("{}: {issue}", def.instruction)];
    if let Some(read) = def.reads {
        lines.push(format!(
            "{}: {}",
            read.label(),
            path(dir, issue, read, slots.read)
        ));
    }
    if let Step::Review(kind) = step {
        lines.push(format!("Review: {}", kind.as_str()));
    }
    if step == Step::OpenPullRequest {
        lines.push(format!(
            "Reference the issue in the PR (for example `Fixes #{}`) so it can be found.",
            issue.number
        ));
    }
    lines.push(format!(
        "Write your {} to this file: {}",
        def.writes.write_label(),
        path(dir, issue, def.writes, slots.write)
    ));
    lines.join("\n")
}

/// Where agents dump everything for this issue: `<$FACTORY_DIR>/#<number>/`.
pub(super) fn issue_dir(dir: &str, issue: &GitHubUrl) -> String {
    format!("{dir}/#{}", issue.number)
}

/// One numbered file, relative to the workspace.
pub(super) fn path(dir: &str, issue: &GitHubUrl, artifact: Artifact, n: u32) -> String {
    format!("{}/{}", issue_dir(dir, issue), artifact.file(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::step::ReviewKind;

    fn issue(n: u64) -> GitHubUrl {
        format!("https://github.com/o/r/issues/{n}")
            .parse()
            .unwrap()
    }

    fn slots(read: u32, write: u32) -> Slots {
        Slots { read, write }
    }

    #[test]
    fn plans_and_reviews_are_numbered_per_run() {
        let issue = issue(7);
        let plan = for_step(Step::Plan, &issue, "factory", slots(1, 2));
        assert!(plan.contains("factory/#7/issue.md"));
        assert!(plan.ends_with("factory/#7/plan/2-plan.md"));

        let review = for_step(
            Step::Review(ReviewKind::Code),
            &issue,
            "factory",
            slots(1, 3),
        );
        assert!(review.contains("Review: code"));
        assert!(review.ends_with("factory/#7/review/3-code.md"));
    }

    #[test]
    fn every_step_names_a_file_in_the_issue_folder() {
        // DevelopPrFeedback is not in route(): the PR watch selects it.
        for step in Step::route().chain(Some(Step::DevelopPrFeedback)) {
            let prompt = for_step(step, &issue(7), "factory", slots(1, 1));
            assert!(prompt.contains("factory/#7/"), "{step:?}: {prompt}");
        }
    }

    #[test]
    fn apply_review_prompt_names_both_files() {
        let develop = for_step(
            Step::DevelopReview(ReviewKind::Code),
            &issue(7),
            "factory",
            slots(2, 3),
        );
        assert!(develop.contains("Review (code): factory/#7/review/2-code.md"));
        assert!(develop.ends_with("factory/#7/development/3-development.md"));
    }

    #[test]
    fn pr_feedback_round_names_the_feedback_and_summary_files() {
        let prompt = for_step(Step::DevelopPrFeedback, &issue(7), "factory", slots(2, 6));
        assert!(prompt.contains("PR feedback: factory/#7/pr-feedback/2-pr-feedback.md"));
        assert!(prompt.ends_with("factory/#7/development/6-development.md"));
    }

    #[test]
    fn developer_steps_write_a_development_summary() {
        let steps = [
            Step::DevelopPlan,
            Step::DevelopReview(ReviewKind::Security),
            Step::OpenPullRequest,
        ];
        for step in steps {
            assert!(
                for_step(step, &issue(7), "factory", slots(1, 4))
                    .ends_with("factory/#7/development/4-development.md"),
                "{step:?}"
            );
        }
    }
}
