//! Task state derived from its event log. Live runs and replay apply the same
//! events through [`TaskState::apply`], so a restart rebuilds exactly this state.

use super::event::{Event, Record};
use super::sequence::{Role, SEQUENCE, Task};
use crate::opencode::Outcome;
use serde::Serialize;
use std::collections::BTreeMap;

/// Failed worker runs after which the runtime stops retrying until a human steps in.
const MAX_FAILURES: u32 = 3;
/// Seconds before the first retry; doubles with each further failure.
const RETRY_BACKOFF_SECS: u64 = 60;

/// A turn the worker prompted and has not completed: running, or cut off by a crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenTurn {
    pub(crate) step: usize,
    pub(crate) session: String,
    /// The session's last finished run before this turn's prompt.
    pub(crate) after: Option<String>,
}

/// Why a task needs a human or is held back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Attention {
    AwaitingHuman,
    Stuck,
    BackingOff,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct TaskState {
    /// Index into [`SEQUENCE`] of the next step to run.
    pub(crate) next_step: usize,
    /// The task's main opencode session.
    pub(crate) main_session: Option<String>,
    /// Latest sub-agent session per role; continued by steps that resume a role.
    pub(crate) children: BTreeMap<Role, String>,
    pub(crate) turn: Option<OpenTurn>,
    /// Sub-agent created for the next step but not yet prompted.
    pub(crate) pending_child: Option<(Role, String)>,
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
            Event::SessionOpened { session } => self.main_session = Some(session.clone()),
            Event::SubagentStarted { role, session, .. } => {
                self.children.insert(*role, session.clone());
                self.pending_child = Some((*role, session.clone()));
            }
            Event::TurnStarted {
                step,
                session,
                after,
                ..
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
                self.next_step = step + 1;
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

    /// The next task to run; `None` once the whole sequence ran.
    pub(crate) fn next(&self) -> Option<Task> {
        SEQUENCE.get(self.next_step).copied()
    }

    /// The sequence ran to the end; the task waits for a human.
    pub(crate) fn is_finished(&self) -> bool {
        self.next().is_none()
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

    pub(crate) fn attention(&self, now: u64) -> Option<Attention> {
        if self.is_finished() {
            Some(Attention::AwaitingHuman)
        } else if self.is_stuck() {
            Some(Attention::Stuck)
        } else if self.retry_at().is_some_and(|at| now < at) {
            Some(Attention::BackingOff)
        } else {
            None
        }
    }

    /// Agent working on the current step right now, if a turn is in flight.
    pub(crate) fn working_agent(&self) -> Option<&'static str> {
        let turn = self.turn.as_ref()?;
        SEQUENCE.get(turn.step).map(|task| task.role().agent())
    }

    /// Short machine-readable status for the dashboard.
    pub(crate) fn status(&self) -> &'static str {
        self.next().map_or("awaiting_human", Task::status)
    }
}
