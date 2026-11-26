//! Task state derived from its event log. Live runs and replay apply the same
//! events through [`TaskState::apply`], so a restart rebuilds exactly this state.

use super::event::{Event, Record};
use super::machine::{Action, Role, Workflow};
use super::stage::ReviewKind;
use super::watcher::Cursor;
use crate::github::pulls::PullRequest;
use crate::opencode::Outcome;
use serde::Serialize;
use std::collections::BTreeMap;

/// Failed worker runs after which the runtime stops retrying until a human steps in.
const MAX_FAILURES: u32 = 3;
/// Seconds before the first retry; doubles with each further failure.
const RETRY_BACKOFF_SECS: u64 = 60;

/// A turn the worker prompted and has not finished recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenTurn {
    pub(crate) role: Role,
    /// Commit when the turn started; reviewers must leave it unchanged.
    pub(crate) head: String,
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
    pub(crate) workflow: Workflow,
    /// The task's main opencode session.
    pub(crate) main_session: Option<String>,
    /// Latest sub-agent child session per role; resumed for follow-ups.
    pub(crate) children: BTreeMap<Role, String>,
    /// A prompted turn without a stage report yet: running, or cut off by a crash.
    pub(crate) turn: Option<OpenTurn>,
    /// Sub-agent created for the current start action, not yet reported on.
    pub(crate) pending_child: Option<(Role, String)>,
    pub(crate) branch: Option<String>,
    pub(crate) pull_request: Option<PullRequest>,
    pub(crate) pushed: Option<String>,
    /// Set while watching the pull request for the current review round.
    pub(crate) watching: Option<Cursor>,
    pub(crate) cursor: Cursor,
    pub(crate) pending_answer: Option<String>,
    pub(crate) merged: bool,
    /// Failed worker exits since the last progress.
    pub(crate) failures: u32,
    pub(crate) last_failure_at: u64,
    pub(crate) last_failure: Option<String>,
    /// When the current action (or the merge) began.
    pub(crate) action_since: u64,
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
        let before = self.workflow.next().clone();
        self.apply_event(record);
        self.updated = record.ts;
        if self.action_since == 0 || *self.workflow.next() != before {
            self.action_since = record.ts;
        }
    }

    fn apply_event(&mut self, record: &Record) {
        match &record.event {
            Event::WorkerStarted => {}
            Event::WorktreeReady { branch, .. } => self.branch = Some(branch.clone()),
            Event::SessionOpened { session } => self.main_session = Some(session.clone()),
            Event::TurnStarted {
                role,
                head,
                session,
                after,
            } => {
                self.turn = Some(OpenTurn {
                    role: *role,
                    head: head.clone(),
                    session: session.clone(),
                    after: after.clone(),
                });
            }
            Event::SubagentFinished { outcome, .. } => {
                if *outcome != Outcome::Succeeded {
                    self.turn = None;
                }
            }
            Event::SubagentStarted { role, session, .. } => {
                self.children.insert(*role, session.clone());
                self.pending_child = Some((*role, session.clone()));
            }
            Event::StageReported { head, stage, .. } => {
                self.turn = None;
                self.pending_child = None;
                self.workflow.advance(stage.clone(), head);
                self.progressed();
            }
            Event::AnswerWritten { text } => self.pending_answer = Some(text.clone()),
            Event::AnswerPosted => {
                self.pending_answer = None;
                self.progressed();
            }
            Event::Pushed { head } => {
                self.pushed = Some(head.clone());
                self.progressed();
            }
            Event::PullRequestOpened { pull_request } => {
                self.pull_request = Some(pull_request.clone());
                self.progressed();
            }
            Event::Watching { cursor } => {
                self.cursor = *cursor;
                self.watching = Some(*cursor);
            }
            Event::SignalReceived { signal, cursor } => {
                self.cursor = *cursor;
                self.watching = Some(*cursor);
                if self.workflow.signal(signal.clone()).is_some() {
                    self.watching = None;
                }
                self.progressed();
            }
            Event::Merged => {
                self.merged = true;
                self.action_since = record.ts;
                self.progressed();
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

    /// Out of retries: the runtime no longer schedules it.
    pub(crate) fn is_stuck(&self) -> bool {
        !self.merged && self.failures >= MAX_FAILURES
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
        if self.merged {
            None
        } else if self.is_stuck() {
            Some(Attention::Stuck)
        } else if self.retry_at().is_some_and(|at| now < at) {
            Some(Attention::BackingOff)
        } else if *self.workflow.next() == Action::AwaitHumanReview {
            Some(Attention::AwaitingHuman)
        } else {
            None
        }
    }

    /// Short machine-readable status for the dashboard.
    pub(crate) fn status(&self) -> &'static str {
        if self.merged {
            return "merged";
        }
        match self.workflow.next() {
            Action::Start(role) | Action::Resume(role, _) => match role {
                Role::SolutionArchitect => "planning",
                Role::Developer => "developing",
                Role::Reviewer(ReviewKind::Security) => "security_review",
                Role::Reviewer(ReviewKind::Code) => "code_review",
                Role::Reviewer(ReviewKind::UiUx) => "uiux_review",
            },
            Action::Answer(_) => "answering_question",
            Action::AwaitHumanReview => "awaiting_human_review",
            Action::Merge => "merging",
        }
    }

    fn progressed(&mut self) {
        self.failures = 0;
        self.last_failure = None;
    }
}
