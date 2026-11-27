//! The factory home (`$FACTORY_HOME`): per-task event logs, locks, and workspaces.

use crate::github::GitHubUrl;
use std::env;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("set FACTORY_HOME to the factory home directory")]
    NoHome,
    #[error("set PROJECT_PATH to the project directory agents work in")]
    NoProject,
    #[error("PROJECT_PATH is not a usable directory: {0}")]
    InvalidProject(String),
    #[error("cannot lock {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// The factory home, from `$FACTORY_HOME`. Required: no built-in default.
pub(crate) fn home() -> Result<PathBuf, Error> {
    env::var_os("FACTORY_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .ok_or(Error::NoHome)
}

/// Where the main session and every agent session run, from `$PROJECT_PATH`.
/// It is the project the agents read, edit, build, and commit in.
pub(crate) fn project_path() -> Result<PathBuf, Error> {
    let raw = env::var_os("PROJECT_PATH")
        .filter(|dir| !dir.is_empty())
        .ok_or(Error::NoProject)?;
    let path = PathBuf::from(raw);
    fs::canonicalize(&path)
        .ok()
        .filter(|path| path.is_dir())
        .ok_or_else(|| Error::InvalidProject(path.display().to_string()))
}

pub(crate) fn tasks_root(home: &Path) -> PathBuf {
    home.join("tasks")
}

/// Holds one task's event log and lock.
pub(crate) fn task_dir(home: &Path, issue: &GitHubUrl) -> PathBuf {
    tasks_root(home)
        .join(&issue.owner)
        .join(&issue.repo)
        .join(issue.number.to_string())
}

pub(crate) fn task_lock_path(task_dir: &Path) -> PathBuf {
    task_dir.join("lock")
}

/// The directory a task's agent sessions run in, when the runtime schedules it.
/// A manual `work` names its own path. Agents own everything in it.
pub(crate) fn workspace_dir(task_dir: &Path) -> PathBuf {
    task_dir.join("workspace")
}

pub(crate) fn events_path(task_dir: &Path) -> PathBuf {
    task_dir.join("events.jsonl")
}

/// Take `path`'s exclusive lock without waiting. `None` while another process holds it.
/// The OS releases it when the returned file drops or the process dies.
pub(crate) fn try_lock(path: &Path) -> Result<Option<File>, Error> {
    let file = open_lock_file(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(source)) => Err(lock_error(path, source)),
    }
}

fn open_lock_file(path: &Path) -> Result<File, Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| lock_error(path, source))?;
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|source| lock_error(path, source))
}

fn lock_error(path: &Path, source: io::Error) -> Error {
    Error::Lock {
        path: path.to_path_buf(),
        source,
    }
}
