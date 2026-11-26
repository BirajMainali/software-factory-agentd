//! The agent's verdict. opencode says when a sub-agent's run ended; only the agent
//! knows what it decided, so it writes one JSON object to
//! `<worktree>/.factory/report.json`, e.g.
//! `{"stage":"review","review":"code","status":"change_requested","text":"..."}`.
//! Read only after opencode reports the run succeeded.

use super::stage::Stage;
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const REPORT_FILE: &str = "report.json";

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("cannot access agent report {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("agent report {0} is not a valid stage report")]
    Invalid(PathBuf),
    #[error("the agent finished without writing its report to {0}")]
    Missing(PathBuf),
}

pub(crate) struct Report {
    pub(crate) stage: Stage,
    /// Free text: the developer's answer to a question, or a reviewer's requested change.
    pub(crate) text: Option<String>,
}

/// Remove a previous report so the next turn's report is the only one.
pub(crate) fn clear(path: &Path) -> Result<(), Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error(path, source)),
    }
}

/// The report the agent wrote at `path`.
pub(crate) fn read(path: &Path) -> Result<Report, Error> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(Error::Missing(path.to_path_buf()));
        }
        Err(source) => return Err(io_error(path, source)),
    };
    let invalid = || Error::Invalid(path.to_path_buf());
    let value: Value = serde_json::from_str(&text).map_err(|_| invalid())?;
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string);
    let stage = Stage::try_from(value).map_err(|_| invalid())?;
    Ok(Report { stage, text })
}

fn io_error(path: &Path, source: io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}
