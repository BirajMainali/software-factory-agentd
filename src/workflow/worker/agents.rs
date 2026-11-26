//! Running one sequence step: create or continue the step's sub-agent session under
//! the task's main session, prompt it, and learn from opencode when the run ended.

use super::{Error, Worker, prompts};
use crate::opencode::{NewSession, Outcome};
use crate::workflow::event::Event;
use crate::workflow::sequence::{Role, Task};

impl Worker<'_> {
    /// Run sequence step `index` to completion. A step cut off by a crash is picked
    /// up again: its run finished, still running, or never sent.
    pub(super) fn run_step(&mut self, index: usize, task: Task) -> Result<(), Error> {
        let role = task.role();
        let prompt = prompts::for_task(task, &self.issue, &self.factory_dir);
        let open = self.state.turn.clone().filter(|turn| turn.step == index);
        let (session, after) = match open {
            Some(turn) => {
                let finished = self.opencode.last_run(&turn.session)? != turn.after;
                if !finished && !self.opencode.is_running(&turn.session)? {
                    self.prompt_subagent(&turn.session, &prompt)?;
                }
                (turn.session, turn.after)
            }
            None => {
                let session = self.session_for(task)?;
                let after = self.opencode.last_run(&session)?;
                self.record(Event::TurnStarted {
                    step: index,
                    role,
                    session: session.clone(),
                    after: after.clone(),
                })?;
                self.prompt_subagent(&session, &prompt)?;
                (session, after)
            }
        };

        let outcome = self.opencode.wait_for_run(&session, after.as_deref())?;
        self.quiet_main()?;
        self.record(Event::SubagentFinished {
            role,
            session,
            outcome,
        })?;
        match outcome {
            Outcome::Succeeded => self.record(Event::StepCompleted { step: index }),
            Outcome::Failed | Outcome::Interrupted => Err(Error::SubagentEnded { role, outcome }),
        }
    }

    /// The session a step runs in: a new sub-agent, or the role's existing one.
    fn session_for(&mut self, task: Task) -> Result<String, Error> {
        let role = task.role();
        if !task.starts_session() {
            return self
                .state
                .children
                .get(&role)
                .cloned()
                .ok_or(Error::NoSession(role));
        }
        match self.state.pending_child.clone() {
            Some((pending, session)) if pending == role => Ok(session),
            _ => self.create_subagent(role),
        }
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
            directory: Some(&self.workspace),
            parent: None,
            agent: None,
        })?;
        self.record(Event::SessionOpened {
            session: main.clone(),
        })?;
        Ok(main)
    }

    fn prompt_subagent(&mut self, child: &str, prompt: &str) -> Result<(), Error> {
        // Main must be idle while a sub-agent works.
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
}
