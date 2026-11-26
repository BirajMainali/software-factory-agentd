//! Run the main session, or continue it with another prompt.

use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("failed to run opencode: {0}")]
    Run(#[source] std::io::Error),
    #[error("opencode failed: {0}")]
    Failed(String),
    #[error("opencode did not report a session id")]
    NoSession,
}

pub(crate) struct OpenCode;

impl OpenCode {
    /// Run `opencode` with a prompt and return the session id. With no session,
    /// a new main session starts; with one, that session continues.
    pub(crate) fn spawn(
        &self,
        session: Option<&str>,
        prompt: &str,
        title: Option<&str>,
    ) -> Result<String, Error> {
        let mut command = Command::new("opencode");
        command.args(["run", "--format", "json"]);
        if let Some(session) = session {
            command.args(["--session", session]);
        }
        if let Some(title) = title {
            command.args(["--title", title]);
        }
        let output = command.arg(prompt).output().map_err(Error::Run)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Failures are sometimes reported on stdout in JSON mode.
            let message = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            return Err(Error::Failed(message.trim().to_string()));
        }
        session_id(&String::from_utf8_lossy(&output.stdout))
            .or_else(|| session.map(str::to_string))
            .ok_or(Error::NoSession)
    }
}

fn session_id(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Event>(line).ok())
        .find_map(|event| event.session_id)
}

#[derive(serde::Deserialize)]
struct Event {
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
}
