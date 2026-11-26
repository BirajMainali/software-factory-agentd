//! Task state derived from its event log. Live runs and replay apply the same
//! events through [`TaskState::apply`], so a restart rebuilds exactly this state.

use super::event::{Event, Record};
use super::step::{Role, Step};
use crate::opencode::Outcome;
use std::collections::BTreeMap;

/// Failed worker runs after which the runtime stops retrying until a human steps in.
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
    /// The issue's title, once fetched.
    pub(crate) title: Option<String>,
    /// The step to run next, unless `finished`.
    pub(crate) step: Step,
    /// Agents are done; the task waits for a human.
    pub(crate) finished: bool,
    /// The task's main opencode session.
    pub(crate) main_session: Option<String>,
    /// Latest sub-agent session per role; continued by steps that resume a role.
    pub(crate) children: BTreeMap<Role, String>,
    pub(crate) turn: Option<OpenTurn>,
    /// Sub-agent created for a step but not yet prompted.
    pub(crate) pending_child: Option<(Step, String)>,
    /// Failed worker exits since the last progress.
    pub(crate) failures: u32,
    pub(crate) last_failure_at: u64,
    pub(crate) last_failure: Option<String>,
    /// When the current step began.
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
                self.turn = Some(OpenTurn {
                    step: *step,
                    session: session.clone(),
                    after: after.clone(),
                });
            }
            Event::SubagentFinished { outcome, .. } => {
                if *outcome != Outcome::Succeeded {
                    self.turn = None;
                }
            }
            Event::StepCompleted { step } => {
                self.turn = None;
                match step.next() {
                    Some(next) => self.step = next,
                    None => self.finished = true,
                }
                self.step_since = record.ts;
                self.failures = 0;
                self.last_failure = None;
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

    /// The step to run next; `None` once agents are done.
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

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    /// Out of retries: the runtime no longer schedules it.
    pub(crate) fn is_stuck(&self) -> bool {
        !self.is_finished() && self.failures >= MAX_FAILURES
    }

    /// Earliest time the runtime retries after a failure; `None` without failures.
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
