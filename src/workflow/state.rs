//! Task state derived from its event log. Live runs and replay apply the same
//! events through [`TaskState::apply`], so a restart rebuilds exactly this state.

use super::event::{Event, Record};
use super::step::{Artifact, Reads, ReviewKind, Role, Slots, Step, def};
use crate::opencode::Outcome;
use std::collections::BTreeMap;

/// Failed worker runs after which the supervisor stops retrying until a human steps in.
const MAX_FAILURES: u32 = 3;
/// Seconds before the first retry; doubles with each further failure.
const RETRY_BACKOFF_SECS: u64 = 60;

/// A turn the worker prompted and has not completed: running, or cut off by a crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenTurn {
    pub(crate) step: Step,
    pub(crate) session: String,
    /// The session's last finished run before this turn's prompt.
    pub(crate) after: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct TaskState {
    pub(crate) title: Option<String>,
    /// The step to run next, unless `finished`.
    pub(crate) step: Step,
    /// Agents are done; the task waits for a human.
    pub(crate) finished: bool,
    pub(crate) main_session: Option<String>,
    /// Latest sub-agent session per role; continued by steps that resume a role.
    pub(crate) children: BTreeMap<Role, String>,
    pub(crate) turn: Option<OpenTurn>,
    /// The folder the last worker wrote artifacts to.
    pub(crate) work_path: Option<String>,
    /// The task's pull request, once it is known.
    pub(crate) pr: Option<String>,
    /// The PR is merged or closed; nothing wakes the task again.
    pub(crate) pr_closed: bool,
    /// Sub-agent created for a step but not yet prompted.
    pub(crate) pending_child: Option<(Step, String)>,
    /// Times each artifact was written; the next run uses `runs + 1`.
    runs: BTreeMap<Artifact, u32>,
    /// The last turn ended without completing its step, so its retry reuses the
    /// attempt's file instead of numbering a new one.
    retry: bool,
    /// What happened, in order, as the dashboard names it: steps, waits and
    /// reviews. Repeats are the loop the task went through.
    pub(crate) display: Vec<&'static str>,
    /// Fingerprint of the PR feedback already recorded, so the watch does not repeat it.
    feedback_key: Option<String>,
    /// The key before the latest round; its time bounds what the round shows.
    prior_feedback_key: Option<String>,
    /// Failed worker exits since the last progress.
    pub(crate) failures: u32,
    pub(crate) last_failure_at: u64,
    pub(crate) last_failure: Option<String>,
    pub(crate) step_since: u64,
    pub(crate) updated: u64,
}

impl TaskState {
    pub(crate) fn replay(records: &[Record]) -> Self {
        let mut state = Self::default();
        for record in records {
            state.apply(record);
        }
        state
    }

    pub(crate) fn apply(&mut self, record: &Record) {
        if self.step_since == 0 {
            self.step_since = record.ts;
        }
        self.updated = record.ts;
        match &record.event {
            Event::WorkerStarted => {}
            Event::IssueFetched { title } => self.title = Some(title.clone()),
            Event::SessionOpened { session } => self.main_session = Some(session.clone()),
            Event::SubagentStarted { step, session, .. } => {
                self.children.insert(step.role(), session.clone());
                self.pending_child = Some((*step, session.clone()));
            }
            Event::TurnStarted {
                step,
                session,
                after,
            } => {
                self.pending_child = None;
                if self.retry {
                    self.retry = false;
                } else {
                    *self.runs.entry(def(*step).writes).or_default() += 1;
                }
                self.turn = Some(OpenTurn {
                    step: *step,
                    session: session.clone(),
                    after: after.clone(),
                });
            }
            Event::SubagentFinished { outcome, .. } => {
                if *outcome != Outcome::Succeeded {
                    self.turn = None;
                    self.retry = true;
                }
            }
            Event::StepCompleted { step, next } => {
                self.turn = None;
                self.retry = false;
                self.display.push(step.stage_label());
                match next.or_else(|| replayed_next(*step)) {
                    Some(next) => self.step = next,
                    None => self.finished = true,
                }
                self.step_since = record.ts;
                self.failures = 0;
                self.last_failure = None;
            }
            Event::WorkPath { path } => self.work_path = Some(path.clone()),
            Event::PullRequestFound { url, key } => {
                self.pr = Some(url.clone());
                self.feedback_key.clone_from(key);
            }
            Event::PullRequestClosed => self.pr_closed = true,
            Event::PrFeedbackArrived { number, key } => {
                self.runs.insert(Artifact::PrFeedback, *number);
                self.prior_feedback_key.clone_from(&self.feedback_key);
                self.feedback_key = Some(key.clone());
                self.retry = false;
                self.step = Step::DevelopPrFeedback;
                self.finished = false;
                self.step_since = record.ts;
            }
            Event::PrReady => {
                self.finished = true;
                self.display.push("PR review");
            }
            Event::WorkerFailed { reason } => self.last_failure = Some(reason.clone()),
            Event::WorkerExited { code } => {
                if *code != Some(0) {
                    self.failures += 1;
                    self.last_failure_at = record.ts;
                }
            }
        }
    }

    pub(crate) fn current(&self) -> Option<Step> {
        (!self.finished).then_some(self.step)
    }

    /// Steps already done, in route order.
    pub(crate) fn completed(&self) -> Vec<Step> {
        let current = self.current();
        Step::route()
            .take_while(|step| Some(*step) != current)
            .collect()
    }

    /// The numbered files `step` uses: the stages it reads and the one it writes.
    /// A resumed turn keeps the numbers it already has.
    pub(crate) fn slots(&self, step: Step, resumed: bool) -> Slots {
        let def = def(step);
        let reads = match def.reads {
            Reads::None => Vec::new(),
            Reads::One(artifact) => vec![(artifact, self.run_count(artifact).max(1))],
            Reads::AllReviews => ReviewKind::ALL
                .iter()
                .map(|kind| {
                    let artifact = Artifact::Review(*kind);
                    (artifact, self.run_count(artifact).max(1))
                })
                .collect(),
            Reads::PlanOrReviews => {
                let reviews: Vec<_> = ReviewKind::ALL
                    .iter()
                    .map(|kind| {
                        let artifact = Artifact::Review(*kind);
                        (artifact, self.run_count(artifact).max(1))
                    })
                    .collect();
                if reviews
                    .iter()
                    .any(|(artifact, _)| self.run_count(*artifact) > 0)
                {
                    reviews
                } else {
                    vec![(Artifact::Plan, self.run_count(Artifact::Plan).max(1))]
                }
            }
        };
        Slots {
            write: if resumed || self.retry {
                self.run_count(def.writes).max(1)
            } else {
                self.run_count(def.writes) + 1
            },
            reads,
        }
    }

    /// The numbered `pr-feedback` file the next round of feedback goes to.
    pub(crate) fn next_pr_feedback(&self) -> u32 {
        self.run_count(Artifact::PrFeedback) + 1
    }

    fn run_count(&self, artifact: Artifact) -> u32 {
        self.runs.get(&artifact).copied().unwrap_or(0)
    }

    pub(crate) fn feedback_key(&self) -> Option<&str> {
        self.feedback_key.as_deref()
    }

    /// The time of the last feedback the developer was given, if any.
    pub(crate) fn feedback_since(&self) -> Option<&str> {
        self.prior_feedback_key
            .as_deref()
            .and_then(|key| key.split_once(':'))
            .map(|(_, time)| time)
            .filter(|time| !time.is_empty())
    }

    pub(crate) fn pr(&self) -> Option<&str> {
        self.pr.as_deref()
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    /// Out of retries: the supervisor no longer schedules it.
    pub(crate) fn is_stuck(&self) -> bool {
        !self.is_finished() && self.failures >= MAX_FAILURES
    }

    /// Earliest time the supervisor retries after a failure; `None` without failures.
    pub(crate) fn retry_at(&self) -> Option<u64> {
        if self.failures == 0 {
            return None;
        }
        let factor = 1_u64 << (self.failures - 1).min(16);
        Some(
            self.last_failure_at
                .saturating_add(RETRY_BACKOFF_SECS.saturating_mul(factor)),
        )
    }
}

/// The successor a log written before `StepCompleted.next` implies. The only
/// edge that changed is the PR feedback, which used to end the route.
fn replayed_next(step: Step) -> Option<Step> {
    match step {
        Step::DevelopPrFeedback => None,
        other => other.next(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::step::ReviewKind;

    fn prompt(state: &mut TaskState, step: Step) {
        state.apply(&Record {
            ts: 0,
            event: Event::TurnStarted {
                step,
                session: "s".to_string(),
                after: None,
            },
        });
    }

    #[test]
    fn develop_reads_the_plan_until_reviews_exist() {
        let mut state = TaskState::default();
        let slots = state.slots(Step::Develop, false);
        assert_eq!(slots.reads, vec![(Artifact::Plan, 1)]);

        prompt(&mut state, Step::Review(ReviewKind::Code));
        let slots = state.slots(Step::Develop, false);
        assert_eq!(
            slots.reads,
            vec![
                (Artifact::Review(ReviewKind::Security), 1),
                (Artifact::Review(ReviewKind::Code), 1),
                (Artifact::Review(ReviewKind::UiUx), 1),
            ]
        );
    }

    #[test]
    fn plans_and_reviews_number_each_run() {
        let mut state = TaskState::default();
        assert_eq!(state.slots(Step::Plan, false).write, 1);
        prompt(&mut state, Step::Plan);
        assert_eq!(state.slots(Step::Develop, false).read(Artifact::Plan), 1);
        assert_eq!(state.slots(Step::Plan, false).write, 2);

        let code = Step::Review(ReviewKind::Code);
        prompt(&mut state, code);
        assert_eq!(
            state
                .slots(Step::DevelopReview(ReviewKind::Code), false)
                .read(Artifact::Review(ReviewKind::Code)),
            1
        );
        prompt(&mut state, code);
        assert_eq!(
            state
                .slots(Step::DevelopReview(ReviewKind::Code), false)
                .read(Artifact::Review(ReviewKind::Code)),
            2
        );
        assert_eq!(state.slots(code, false).write, 3);
        assert_eq!(state.slots(code, true).write, 2);
    }

    #[test]
    fn address_reviews_reads_the_latest_of_each_kind() {
        let mut state = TaskState::default();
        prompt(&mut state, Step::Review(ReviewKind::Security));
        prompt(&mut state, Step::Review(ReviewKind::Code));
        prompt(&mut state, Step::Review(ReviewKind::Code));
        prompt(&mut state, Step::Review(ReviewKind::UiUx));

        let slots = state.slots(Step::AddressReviews, false);
        assert_eq!(slots.read(Artifact::Review(ReviewKind::Security)), 1);
        assert_eq!(slots.read(Artifact::Review(ReviewKind::Code)), 2);
        assert_eq!(slots.read(Artifact::Review(ReviewKind::UiUx)), 1);
    }

    #[test]
    fn a_found_pr_seeds_seen_feedback_and_closed_ends_the_task() {
        let mut state = TaskState::default();
        state.apply(&Record {
            ts: 1,
            event: Event::PullRequestFound {
                url: "https://github.com/o/r/pull/3".to_string(),
                key: Some("0:".to_string()),
            },
        });
        assert_eq!(state.pr(), Some("https://github.com/o/r/pull/3"));
        assert_eq!(state.feedback_key(), Some("0:"));
        assert!(!state.pr_closed);

        state.apply(&Record {
            ts: 2,
            event: Event::PullRequestClosed,
        });
        assert!(state.pr_closed);
    }

    #[test]
    fn a_found_pr_with_unseen_feedback_does_not_seed_the_key() {
        let mut state = TaskState::default();
        state.apply(&Record {
            ts: 1,
            event: Event::PullRequestFound {
                url: "https://github.com/o/r/pull/3".to_string(),
                key: None,
            },
        });
        assert_eq!(state.feedback_key(), None);
    }

    #[test]
    fn pr_feedback_wakes_the_task_and_numbers_each_round() {
        let mut state = TaskState::default();
        state.apply(&Record {
            ts: 1,
            event: Event::StepCompleted {
                step: Step::OpenPullRequest,
                next: None,
            },
        });
        assert!(state.is_finished());

        let arrived = |number, key: &str| Record {
            ts: 2,
            event: Event::PrFeedbackArrived {
                number,
                key: key.to_string(),
            },
        };
        state.apply(&arrived(1, "1:2026-01-01T00:00:00Z"));
        assert_eq!(state.current(), Some(Step::DevelopPrFeedback));
        assert_eq!(
            state
                .slots(Step::DevelopPrFeedback, false)
                .read(Artifact::PrFeedback),
            1
        );
        assert_eq!(state.next_pr_feedback(), 2);
        assert_eq!(state.feedback_key(), Some("1:2026-01-01T00:00:00Z"));
        assert_eq!(state.feedback_since(), None);

        state.apply(&Record {
            ts: 3,
            event: Event::StepCompleted {
                step: Step::DevelopPrFeedback,
                next: Some(Step::Review(ReviewKind::Security)),
            },
        });
        // The new head is reviewed again before the task goes back to the watch.
        assert_eq!(state.current(), Some(Step::Review(ReviewKind::Security)));

        state.apply(&arrived(2, "2:2026-01-02T00:00:00Z"));
        assert_eq!(
            state
                .slots(Step::DevelopPrFeedback, false)
                .read(Artifact::PrFeedback),
            2
        );
        assert_eq!(state.next_pr_feedback(), 3);
        assert_eq!(state.feedback_since(), Some("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn every_developer_run_gets_its_own_summary_number() {
        let mut state = TaskState::default();
        assert_eq!(state.slots(Step::Develop, false).write, 1);
        prompt(&mut state, Step::Develop);
        assert_eq!(state.slots(Step::AddressReviews, false).write, 2);
        prompt(&mut state, Step::AddressReviews);
        assert_eq!(state.slots(Step::OpenPullRequest, false).write, 3);
        prompt(&mut state, Step::OpenPullRequest);
        assert_eq!(state.slots(Step::Develop, false).write, 4);
        assert_eq!(state.slots(Step::OpenPullRequest, true).write, 3);
    }

    #[test]
    fn a_retried_step_reuses_its_file_instead_of_numbering_another() {
        let mut state = TaskState::default();
        assert_eq!(state.slots(Step::Develop, false).write, 1);
        prompt(&mut state, Step::Develop);

        state.apply(&Record {
            ts: 1,
            event: Event::SubagentFinished {
                step: Step::Develop,
                outcome: Outcome::Failed,
            },
        });
        assert_eq!(state.slots(Step::Develop, false).write, 1);
        prompt(&mut state, Step::Develop);

        state.apply(&Record {
            ts: 2,
            event: Event::StepCompleted {
                step: Step::Develop,
                next: Some(Step::Review(ReviewKind::Security)),
            },
        });
        assert_eq!(state.slots(Step::AddressReviews, false).write, 2);
    }

    #[test]
    fn a_ready_label_ends_the_route() {
        let mut state = TaskState::default();
        state.apply(&Record {
            ts: 1,
            event: Event::PrReady,
        });
        assert!(state.is_finished());
    }

    #[test]
    fn an_old_feedback_round_replays_as_finished() {
        // Logs written before the review loop have no `next`; they ended here.
        let mut old = TaskState::default();
        old.apply(&Record {
            ts: 1,
            event: Event::StepCompleted {
                step: Step::DevelopPrFeedback,
                next: None,
            },
        });
        assert!(old.is_finished());

        let mut current = TaskState::default();
        current.apply(&Record {
            ts: 1,
            event: Event::StepCompleted {
                step: Step::DevelopPrFeedback,
                next: Some(Step::Review(ReviewKind::Security)),
            },
        });
        assert_eq!(current.current(), Some(Step::Review(ReviewKind::Security)));
    }
}
