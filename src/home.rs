//! The factory home (`$FACTORY_HOME`): per-task event logs, locks, and workspaces.

use crate::github::GitHubUrl;
use std::env;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("set FACTORY_HOME to the factory home directory")]
    NoHome,
    #[error("set FACTORY_DIR to the folder name agents and the runtime share in a workspace")]
    NoFactoryDir,
    #[error("FACTORY_DIR must be a relative path inside the workspace, got {0:?}")]
    InvalidFactoryDir(String),
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

/// Folder inside each workspace where the runtime and agents exchange files (issue
/// text, plan, report), from `$FACTORY_DIR`. Required: no built-in default. Must be a
/// relative path inside the workspace.
pub(crate) fn factory_dir() -> Result<String, Error> {
    let name = env::var("FACTORY_DIR").map_err(|_| Error::NoFactoryDir)?;
    let inside_workspace = !name.is_empty()
        && Path::new(&name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if inside_workspace {
        Ok(name)
    } else {
        Err(Error::InvalidFactoryDir(name))
    }
}

/// The directory a task's agent sessions run in. Agents own everything in it.
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
