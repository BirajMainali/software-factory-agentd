//! Run child processes under a deadline; a hung child is killed, never awaited forever.

use std::io::{Read, Write as _};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const EXIT_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("failed to run {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to wait for {program}: {source}")]
    Wait {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{program} timed out after {}s", .timeout.as_secs())]
    TimedOut { program: String, timeout: Duration },
}

pub(crate) struct Output {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

impl Output {
    /// Stderr, or stdout when stderr is empty, for failure messages.
    pub(crate) fn failure_message(&self) -> String {
        let message = match self.stderr.trim() {
            "" => self.stdout.trim(),
            stderr => stderr,
        };
        if message.is_empty() {
            format!("exit status {}", self.status)
        } else {
            message.to_string()
        }
    }
}

/// Run `command` to completion within `timeout`.
pub(crate) fn run(command: &mut Command, timeout: Duration) -> Result<Output, Error> {
    run_with_input(command, "", timeout)
}

/// Like [`run`], writing `input` to the child's stdin. Keeps secrets out of argv.
pub(crate) fn run_with_input(
    command: &mut Command,
    input: &str,
    timeout: Duration,
) -> Result<Output, Error> {
    let program = command.get_program().to_string_lossy().into_owned();
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdin(if input.is_empty() {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Spawn {
            program: program.clone(),
            source,
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_string();
        // A child that exits without reading its input reports its own error.
        thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let stdout = child.stdout.take().map(read_all);
    let stderr = child.stderr.take().map(read_all);

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                return Err(kill(&mut child, program, timeout));
            }
            Ok(None) => thread::sleep(EXIT_POLL),
            Err(source) => return Err(Error::Wait { program, source }),
        }
    };
    let collect = |pipe: Option<Receiver<String>>| {
        pipe.and_then(|pipe| {
            pipe.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .ok()
        })
        .unwrap_or_default()
    };

    Ok(Output {
        status,
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

/// Drain a pipe on its own thread so a full pipe never blocks the child.
fn read_all(mut pipe: impl Read + Send + 'static) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        // A read error only truncates the captured text.
        let _ = pipe.read_to_end(&mut bytes);
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}

fn kill(child: &mut Child, program: String, timeout: Duration) -> Error {
    // The child may have exited between the deadline and the kill; either way it is gone.
    let _ = child.kill();
    let _ = child.wait();
    Error::TimedOut { program, timeout }
}
