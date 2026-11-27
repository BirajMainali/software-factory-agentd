//! The home repo (`$HOME_REPO_PATH`): per-task event logs, locks, and workspaces.

use crate::github::GitHubUrl;
use std::env;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("set HOME_REPO_PATH to the home repo directory")]
    NoHome,
    #[error("HOME_REPO_PATH is not a usable directory: {0}")]
    InvalidHome(String),
    #[error("cannot lock {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// The home repo, from `$HOME_REPO_PATH`. Required: no built-in default.
/// Task state lives here, and it is the directory agent sessions run in, so
/// opencode resolves this repo's agents and config.
pub(crate) fn home() -> Result<PathBuf, Error> {
    let raw = env::var_os("HOME_REPO_PATH")
        .filter(|dir| !dir.is_empty())
        .ok_or(Error::NoHome)?;
    let path = PathBuf::from(raw);
    fs::canonicalize(&path)
        .ok()
        .filter(|path| path.is_dir())
        .ok_or_else(|| Error::InvalidHome(path.display().to_string()))
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
