//! The human gate: push the reviewed commit, open the pull request, wait for a
//! human signal on it, post answers, and merge the approved commit.

use super::{Error, Worker};
use crate::github::pulls::{self, PullRequest, PullState};
use crate::workflow::event::Event;
use crate::workflow::watcher::{self, Poll};
use std::thread;
use std::time::Duration;

const WATCH_INTERVAL: Duration = Duration::from_secs(30);
const MAX_WATCH_ERRORS: u32 = 5;

impl Worker<'_> {
    /// Push the reviewed commit, make sure a pull request exists, then wait for a
    /// human signal on it.
    pub(super) fn await_human(&mut self) -> Result<(), Error> {
        let head = self.worktree.head()?;
        if self.state.pushed.as_deref() != Some(head.as_str()) {
            self.worktree.push()?;
            self.record(Event::Pushed { head })?;
        }
        let pull = self.pull_request()?;
        let mut cursor = match self.state.watching {
            Some(cursor) => cursor,
            None => {
                let cursor = watcher::start(&self.issue, pull.number, self.state.cursor)?;
                self.record(Event::Watching { cursor })?;
                cursor
            }
        };

        let mut errors = 0;
        loop {
            match watcher::poll(&self.issue, pull.number, cursor) {
                Ok(Poll::Signal {
                    signal,
                    cursor: next,
                }) => {
                    self.record(Event::SignalReceived {
                        signal,
                        cursor: next,
                    })?;
                    if self.state.watching.is_none() {
                        return Ok(());
                    }
                    // Not accepted (approval of an unreviewed head); keep watching.
                    cursor = next;
                    errors = 0;
                    continue;
                }
                Ok(Poll::Merged) => return self.record(Event::Merged),
                Ok(Poll::Closed) => return Err(Error::PullRequestClosed),
                Ok(Poll::Waiting) => errors = 0,
                Err(error) => {
                    errors += 1;
                    if errors >= MAX_WATCH_ERRORS {
                        return Err(error.into());
                    }
                }
            }
            thread::sleep(WATCH_INTERVAL * errors.max(1));
        }
    }

    fn pull_request(&mut self) -> Result<PullRequest, Error> {
        if let Some(pull) = &self.state.pull_request {
            return Ok(pull.clone());
        }
        let pull = pulls::find_or_create(&self.issue, &self.worktree.branch, &self.title)?;
        self.record(Event::PullRequestOpened {
            pull_request: pull.clone(),
        })?;
        Ok(pull)
    }

    pub(super) fn post_answer(&mut self, answer: &str) -> Result<(), Error> {
        let pull = self
            .state
            .pull_request
            .clone()
            .ok_or(Error::NoPullRequest)?;
        let marker = format!(
            "<!-- factory-answer:{} -->",
            self.state.cursor.comments_after
        );
        pulls::comment_once(&self.issue, pull.number, answer, &marker)?;
        self.record(Event::AnswerPosted)
    }

    /// Merge exactly the reviewed and approved commit.
    pub(super) fn merge(&mut self) -> Result<(), Error> {
        let head = self
            .state
            .workflow
            .reviewed()
            .ok_or(Error::NothingReviewed)?
            .to_string();
        let pull = self
            .state
            .pull_request
            .clone()
            .ok_or(Error::NoPullRequest)?;
        match pulls::state(&self.issue, pull.number)? {
            PullState::Merged => {}
            PullState::Closed => return Err(Error::PullRequestClosed),
            PullState::Open { .. } => pulls::merge(&self.issue, pull.number, &head)?,
        }
        self.record(Event::Merged)
    }
}
