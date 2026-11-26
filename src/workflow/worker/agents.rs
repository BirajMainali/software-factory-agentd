//! Sub-agent turns. The runtime creates each role's sub-agent session under the
//! task's main session, prompts it, and learns from opencode when the run ended.

use super::{Error, Worker, prompts};
use crate::opencode::{NewSession, Outcome};
use crate::workflow::event::Event;
use crate::workflow::machine::{Action, Role};
use crate::workflow::report::{self, REPORT_FILE};

impl Worker<'_> {
    /// Create a new sub-agent session for `role` under the main session and run its
    /// first turn. After a crash, the sub-agent already created is reused.
    pub(super) fn start_subagent(&mut self, role: Role) -> Result<(), Error> {
        let child = match self.state.pending_child.clone() {
            Some((pending, child)) if pending == role => child,
            _ => self.create_subagent(role)?,
        };
        let prompt = prompts::start(&self.issue, role);
        self.run_turn(role, &child, &prompt)
    }

    /// Continue `role`'s existing sub-agent session with a follow-up.
    pub(super) fn resume_subagent(&mut self, role: Role, prompt: &str) -> Result<(), Error> {
        let child = self
            .state
            .children
            .get(&role)
            .cloned()
            .ok_or(Error::NoSession(role))?;
        self.run_turn(role, &child, prompt)
    }

    fn create_subagent(&mut self, role: Role) -> Result<String, Error> {
        let main = self.main_session()?;
        let child = self.opencode.create_session(&NewSession {
            title: role.agent(),
            directory: None,
            parent: Some(&main),
            agent: Some(role.agent()),
        })?;
        self.record(Event::SubagentStarted {
            role,
            session: child.clone(),
            parent: main,
        })?;
        Ok(child)
    }

    /// The task's main session, created on first use and titled for the GUI.
    fn main_session(&mut self) -> Result<String, Error> {
        if let Some(main) = &self.state.main_session {
            return Ok(main.clone());
        }
        let title = format!(
            "{}#{} {}",
            self.issue.repo_slug(),
            self.issue.number,
            self.title
        );
        let main = self.opencode.create_session(&NewSession {
            title: &title,
            directory: Some(&self.worktree.path),
            parent: None,
            agent: None,
        })?;
        self.record(Event::SessionOpened {
            session: main.clone(),
        })?;
        Ok(main)
    }

    /// Prompt `child` and wait for opencode to report the run ended. A turn cut
    /// off by a crash is picked up again: finished, still running, or never sent.
    fn run_turn(&mut self, role: Role, child: &str, prompt: &str) -> Result<(), Error> {
        let report_path = self.worktree.factory_file(REPORT_FILE);
        let open = self
            .state
            .turn
            .clone()
            .filter(|turn| turn.role == role && turn.session == child);
        let after = match open {
            Some(turn) => {
                let finished = self.opencode.last_run(child)? != turn.after;
                if !finished && !self.opencode.is_running(child)? {
                    self.prompt_subagent(child, prompt)?;
                }
                turn.after
            }
            None => {
                report::clear(&report_path)?;
                let after = self.opencode.last_run(child)?;
                self.record(Event::TurnStarted {
                    role,
                    head: self.worktree.head()?,
                    session: child.to_string(),
                    after: after.clone(),
                })?;
                self.prompt_subagent(child, prompt)?;
                after
            }
        };

        let outcome = self.opencode.wait_for_run(child, after.as_deref())?;
        self.quiet_main()?;
        self.record(Event::SubagentFinished {
            role,
            session: child.to_string(),
            outcome,
        })?;
        match outcome {
            Outcome::Succeeded => self.finish_turn(role, child.to_string()),
            Outcome::Failed | Outcome::Interrupted => Err(Error::SubagentEnded { role, outcome }),
        }
    }

    fn prompt_subagent(&mut self, child: &str, prompt: &str) -> Result<(), Error> {
        // Main must be idle before a sub-agent works in the shared worktree.
        self.quiet_main()?;
        self.opencode.prompt(child, prompt)?;
        Ok(())
    }

    /// opencode wakes the main session whenever a sub-agent reports back; the runtime,
    /// not main, decides the next step, so main is stopped.
    fn quiet_main(&self) -> Result<(), Error> {
        if let Some(main) = &self.state.main_session {
            self.opencode.interrupt(main)?;
        }
        Ok(())
    }

    /// Check what the sub-agent did and record the verdict it reported.
    fn finish_turn(&mut self, role: Role, session: String) -> Result<(), Error> {
        let report = report::read(&self.worktree.factory_file(REPORT_FILE))?;
        let before = self.state.turn.as_ref().map(|turn| turn.head.clone());
        match role {
            Role::SolutionArchitect => {}
            Role::Developer => self
                .worktree
                .commit_all(&format!("factory: work on #{}", self.issue.number))?,
            Role::Reviewer(_) => {
                let head = self.worktree.head()?;
                if before.as_deref() != Some(head.as_str()) || !self.worktree.is_clean()? {
                    return Err(Error::ReviewerChangedCode(role));
                }
            }
        }

        let head = self.worktree.head()?;
        let stage = report.stage;
        let answering = matches!(self.state.workflow.next(), Action::Answer(_));
        if self
            .state
            .workflow
            .clone()
            .advance(stage.clone(), &head)
            .is_none()
        {
            return Err(Error::StaleStage(role));
        }
        if answering {
            let text = report.text.ok_or(Error::NoAnswer)?;
            self.record(Event::AnswerWritten { text })?;
        }
        self.record(Event::StageReported {
            role,
            session,
            head,
            stage,
        })?;
        report::clear(&self.worktree.factory_file(REPORT_FILE))?;
        Ok(())
    }
}
