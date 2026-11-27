//! What the router sends agents: one sentence, the file to read, the file to write.
//! Paths are absolute, inside the `--path` folder the caller chose. How to do the
//! work is the agent's.

use crate::github::GitHubUrl;
use crate::workflow::step::{Artifact, Slots, Step, def};
use std::path::{Path, PathBuf};

/// The prompt for one step.
pub(super) fn for_step(step: Step, issue: &GitHubUrl, dir: &Path, slots: &Slots) -> String {
    let def = def(step);
    let mut lines = vec![format!("{}: {issue}", def.instruction)];
    for (read, n) in &slots.reads {
        lines.push(format!(
            "{}: {}",
            read.label(),
            file(dir, *read, *n).display()
        ));
    }
    if let Step::Review(kind) = step {
        lines.push(format!("Review: {}", kind.as_str()));
    }
    if step == Step::DevelopPlan {
        lines.push(format!(
            "Push the branch and open a draft PR that references this issue (for example `Fixes #{}`).",
            issue.number
        ));
    }
    lines.push(format!(
        "Write your {} to this file: {}",
        def.writes.write_label(),
        file(dir, def.writes, slots.write).display()
    ));
    lines.join("\n")
}

/// One numbered artifact, directly inside the path the caller gave.
pub(super) fn file(dir: &Path, artifact: Artifact, n: u32) -> PathBuf {
    dir.join(artifact.file(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::step::{Reads, ReviewKind};

    fn issue(n: u64) -> GitHubUrl {
        format!("https://github.com/o/r/issues/{n}")
            .parse()
            .unwrap()
    }

    fn slots(reads: &[(Artifact, u32)], write: u32) -> Slots {
        Slots {
            reads: reads.to_vec(),
            write,
        }
    }

    fn slots_for(step: Step, write: u32) -> Slots {
        let reads = match def(step).reads {
            Reads::None => Vec::new(),
            Reads::One(artifact) => vec![(artifact, 1)],
            Reads::AllReviews => ReviewKind::ALL
                .iter()
                .map(|kind| (Artifact::Review(*kind), 1))
                .collect(),
        };
        Slots { reads, write }
    }

    #[test]
    fn plans_and_reviews_are_numbered_per_run() {
        let issue = issue(7);
        let dir = Path::new("/work");
        let plan = for_step(Step::Plan, &issue, dir, &slots(&[(Artifact::Issue, 1)], 2));
        assert!(plan.contains("/work/issue.md"));
        assert!(plan.ends_with("/work/plan/2-plan.md"));

        let review = for_step(Step::Review(ReviewKind::Code), &issue, dir, &slots(&[], 3));
        assert!(review.contains("Review: code"));
        assert!(review.ends_with("/work/review/3-code.md"));
    }

    #[test]
    fn every_step_names_a_file_in_the_given_path() {
        // DevelopPrFeedback is not in route(): the PR watch selects it.
        for step in Step::route().chain(Some(Step::DevelopPrFeedback)) {
            let prompt = for_step(step, &issue(7), Path::new("/work"), &slots_for(step, 1));
            assert!(prompt.contains("/work/"), "{step:?}: {prompt}");
        }
    }

    #[test]
    fn develop_plan_asks_for_a_draft_pr_referencing_the_issue() {
        let prompt = for_step(
            Step::DevelopPlan,
            &issue(7),
            Path::new("/work"),
            &slots_for(Step::DevelopPlan, 2),
        );
        assert!(
            prompt.contains("open a draft PR that references this issue (for example `Fixes #7`)")
        );
    }

    #[test]
    fn address_reviews_prompt_names_all_three_reviews() {
        let prompt = for_step(
            Step::AddressReviews,
            &issue(7),
            Path::new("/work"),
            &slots(
                &[
                    (Artifact::Review(ReviewKind::Security), 1),
                    (Artifact::Review(ReviewKind::Code), 2),
                    (Artifact::Review(ReviewKind::UiUx), 1),
                ],
                3,
            ),
        );
        assert!(prompt.contains("Review (security): /work/review/1-security.md"));
        assert!(prompt.contains("Review (code): /work/review/2-code.md"));
        assert!(prompt.contains("Review (uiux): /work/review/1-uiux.md"));
        assert!(prompt.ends_with("/work/development/3-development.md"));
    }

    #[test]
    fn legacy_develop_review_still_names_its_review() {
        let prompt = for_step(
            Step::DevelopReview(ReviewKind::Code),
            &issue(7),
            Path::new("/work"),
            &slots(&[(Artifact::Review(ReviewKind::Code), 2)], 3),
        );
        assert!(prompt.contains("Review (code): /work/review/2-code.md"));
        assert!(prompt.ends_with("/work/development/3-development.md"));
    }

    #[test]
    fn pr_feedback_round_names_the_feedback_and_summary_files() {
        let prompt = for_step(
            Step::DevelopPrFeedback,
            &issue(7),
            Path::new("/work"),
            &slots(&[(Artifact::PrFeedback, 2)], 6),
        );
        assert!(prompt.contains("PR feedback: /work/pr-feedback/2-pr-feedback.md"));
        assert!(prompt.ends_with("/work/development/6-development.md"));
    }

    #[test]
    fn developer_steps_write_a_development_summary() {
        let steps = [
            Step::DevelopPlan,
            Step::AddressReviews,
            Step::OpenPullRequest,
        ];
        for step in steps {
            assert!(
                for_step(step, &issue(7), Path::new("/work"), &slots_for(step, 4))
                    .ends_with("/work/development/4-development.md"),
                "{step:?}"
            );
        }
    }
}
