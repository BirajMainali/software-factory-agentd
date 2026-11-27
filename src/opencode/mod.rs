//! Client for the local opencode service API. The runtime creates sessions, sends
//! prompts, and reads only lifecycle facts back: when a run ended and with which
//! outcome. It never reads what agents wrote.
//!
//! The service writes its address and access key to `service.json`; the key goes to
//! `curl` through stdin, never argv, logs, or events.

use crate::process;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RUN_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const RUN_POLL: Duration = Duration::from_secs(3);
const SERVICE_USER: &str = "opencode";

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("cannot read the opencode service file {path} (is opencode running?): {reason}")]
    Service { path: PathBuf, reason: String },
    #[error("failed to call the opencode service: {0}")]
    Call(#[from] process::Error),
    #[error("opencode {method} {path} failed with HTTP {status}")]
    Status {
        method: &'static str,
        path: String,
        status: u16,
    },
    #[error("opencode {path} returned an unexpected response")]
    Response { path: String },
    #[error("session {0} did not finish within {secs}s", secs = RUN_TIMEOUT.as_secs())]
    RunTimedOut(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Succeeded,
    Failed,
    Interrupted,
}

pub(crate) struct NewSession<'a> {
    pub(crate) title: &'a str,
    /// Working directory; a child inherits its parent's.
    pub(crate) directory: Option<&'a Path>,
    pub(crate) parent: Option<&'a str>,
    pub(crate) agent: Option<&'a str>,
}

pub(crate) struct Client {
    url: String,
    key: String,
}

#[derive(Deserialize)]
struct Service {
    url: String,
    password: String,
}

#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

#[derive(Deserialize)]
struct SessionInfo {
    id: String,
}

#[derive(Deserialize)]
struct IdleMessage {
    id: String,
    outcome: Outcome,
}

impl Client {
    pub(crate) fn connect() -> Result<Self, Error> {
        let path = service_file();
        let service_error = |reason: String| Error::Service {
            path: path.clone(),
            reason,
        };
        let text = fs::read_to_string(&path).map_err(|error| service_error(error.to_string()))?;
        let service: Service =
            serde_json::from_str(&text).map_err(|error| service_error(error.to_string()))?;
        Ok(Self {
            url: service.url,
            key: service.password,
        })
    }

    pub(crate) fn create_session(&self, new: &NewSession<'_>) -> Result<String, Error> {
        let body = json!({
            "title": new.title,
            "location": new.directory.map(|dir| json!({ "directory": dir })),
            "parentID": new.parent,
            "agent": new.agent,
        });
        let created: Data<SessionInfo> = self.call_json("POST", "/api/session", Some(&body))?;
        Ok(created.data.id)
    }

    pub(crate) fn prompt(&self, session: &str, text: &str) -> Result<(), Error> {
        let path = format!("/api/session/{session}/prompt");
        self.call("POST", &path, Some(&json!({ "text": text })))?;
        Ok(())
    }

    pub(crate) fn interrupt(&self, session: &str) -> Result<(), Error> {
        self.call("POST", &format!("/api/session/{session}/interrupt"), None)?;
        Ok(())
    }

    pub(crate) fn last_run(&self, session: &str) -> Result<Option<String>, Error> {
        Ok(self.latest_idle(session)?.map(|idle| idle.id))
    }

    /// Wait for a run that finishes after `after` (a [`Client::last_run`] id) and
    /// return how it ended.
    pub(crate) fn wait_for_run(
        &self,
        session: &str,
        after: Option<&str>,
    ) -> Result<Outcome, Error> {
        let deadline = Instant::now() + RUN_TIMEOUT;
        loop {
            if let Some(idle) = self.latest_idle(session)?
                && Some(idle.id.as_str()) != after
            {
                return Ok(idle.outcome);
            }
            if Instant::now() >= deadline {
                return Err(Error::RunTimedOut(session.to_string()));
            }
            thread::sleep(RUN_POLL);
        }
    }

    pub(crate) fn is_running(&self, session: &str) -> Result<bool, Error> {
        let active: Data<serde_json::Map<String, Value>> =
            self.call_json("GET", "/api/session/active", None)?;
        Ok(active.data.contains_key(session))
    }

    fn latest_idle(&self, session: &str) -> Result<Option<IdleMessage>, Error> {
        let path = format!("/api/session/{session}/message?type=idle&order=desc&limit=1");
        let idle: Data<Vec<IdleMessage>> = self.call_json("GET", &path, None)?;
        Ok(idle.data.into_iter().next())
    }

    fn call_json<T: serde::de::DeserializeOwned>(
        &self,
        method: &'static str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<T, Error> {
        let text = self.call(method, path, body)?;
        serde_json::from_str(&text).map_err(|_| Error::Response {
            path: path.to_string(),
        })
    }

    /// One HTTP call through `curl`; the access key goes in via stdin config.
    fn call(
        &self,
        method: &'static str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<String, Error> {
        let mut command = Command::new("curl");
        command.args([
            "--silent",
            "--show-error",
            "--config",
            "-",
            "--request",
            method,
            "--header",
            "Content-Type: application/json",
            "--write-out",
            "\n%{http_code}",
        ]);
        if let Some(body) = body {
            command.arg("--data-binary").arg(body.to_string());
        }
        command.arg(format!("{}{path}", self.url));
        let config = format!("user = \"{SERVICE_USER}:{}\"\n", escape_config(&self.key));
        let output = process::run_with_input(&mut command, &config, REQUEST_TIMEOUT)?;

        let response_error = || Error::Response {
            path: path.to_string(),
        };
        let (response, status) = output.stdout.rsplit_once('\n').ok_or_else(response_error)?;
        let status: u16 = status.trim().parse().map_err(|_| response_error())?;
        if !(200..300).contains(&status) {
            return Err(Error::Status {
                method,
                path: path.to_string(),
                status,
            });
        }
        Ok(response.to_string())
    }
}

/// `$XDG_STATE_HOME/opencode/service.json`, else `~/.local/state/opencode/service.json`.
fn service_file() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_default()
        .join("opencode/service.json")
}

fn escape_config(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
