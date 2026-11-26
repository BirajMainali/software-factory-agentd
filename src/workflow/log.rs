//! Append-only JSON Lines event log, one file per task.

use super::event::{Event, Record};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("cannot access event log {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("event log {path} line {line} is invalid: {source}")]
    Corrupt {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("cannot encode event: {0}")]
    Encode(#[source] serde_json::Error),
}

/// Appends events. Hold the task lock while a `Log` is open: one writer at a time.
pub(crate) struct Log {
    path: PathBuf,
    file: File,
}

impl Log {
    pub(crate) fn open(path: &Path) -> Result<Self, Error> {
        let io_error = |source| Error::Io {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .map_err(io_error)?;

        // A crash mid-append can leave a partial last line; start the next one cleanly.
        if file.metadata().map_err(io_error)?.len() > 0 {
            let mut last = [0_u8];
            file.seek(SeekFrom::End(-1)).map_err(io_error)?;
            file.read_exact(&mut last).map_err(io_error)?;
            if last[0] != b'\n' {
                file.write_all(b"\n").map_err(io_error)?;
            }
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
        })
    }

    /// Write `event` durably before returning.
    pub(crate) fn append(&mut self, event: Event) -> Result<Record, Error> {
        let record = Record { ts: now(), event };
        let mut line = serde_json::to_string(&record).map_err(Error::Encode)?;
        line.push('\n');
        let io_error = |source| Error::Io {
            path: self.path.clone(),
            source,
        };
        self.file.write_all(line.as_bytes()).map_err(io_error)?;
        self.file.sync_data().map_err(io_error)?;
        Ok(record)
    }
}

/// All records in `path`; empty when the task has no log yet. A partial last line
/// (crash mid-append) is skipped; any other invalid line is an error.
pub(crate) fn read(path: &Path) -> Result<Vec<Record>, Error> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let complete = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let mut records = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(record) => records.push(record),
            Err(_) if !complete && index + 1 == lines.len() => {}
            Err(source) => {
                return Err(Error::Corrupt {
                    path: path.to_path_buf(),
                    line: index + 1,
                    source,
                });
            }
        }
    }
    Ok(records)
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
