//! The factory home: clones, worktrees, and task logs. Lives outside this repo and
//! every product repo.

use crate::github::GitHubUrl;
use std::env;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("set FACTORY_HOME or HOME to locate the factory home")]
    NoHome,
    #[error("cannot lock {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// `$FACTORY_HOME`, else `$XDG_DATA_HOME/software-factory`, else
/// `~/.local/share/software-factory`.
pub(crate) fn home() -> Result<PathBuf, Error> {
    if let Some(dir) = env::var_os("FACTORY_HOME") {
        return Ok(dir.into());
    }
    if let Some(dir) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(dir).join("software-factory"));
    }
    env::var_os("HOME")
        .map(|dir| PathBuf::from(dir).join(".local/share/software-factory"))
        .ok_or(Error::NoHome)
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

/// The task's append-only event log.
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

/// Take `path`'s exclusive lock, waiting for the holder to release it.
pub(crate) fn lock(path: &Path) -> Result<File, Error> {
    let file = open_lock_file(path)?;
    file.lock().map_err(|source| lock_error(path, source))?;
    Ok(file)
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
