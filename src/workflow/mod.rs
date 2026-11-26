//! Task workflow: the pure state machine, the event log it is replayed from, and
//! the worker that drives one task.

mod event;
mod log;
mod machine;
mod report;
mod stage;
mod state;
mod watcher;
mod worker;

pub(crate) use event::Event;
pub(crate) use log::{Error as LogError, Log, now, read as read_log};
pub(crate) use state::{Attention, TaskState};
pub(crate) use worker::Worker;
