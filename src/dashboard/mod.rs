//! Read-only dashboard. Replays every task log into its derived state and streams it
//! to browsers over server-sent events. Owns no state and writes nothing.
//! Shows status only: no agent output, feedback text, or repository content.

use crate::github::GitHubUrl;
use crate::home;
use crate::workflow::{self, Step, TaskState};
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

const INDEX: &str = include_str!("index.html");
const MAX_CLIENTS: usize = 32;
const MAX_REQUEST_LINE: u64 = 8 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const REFRESH: Duration = Duration::from_secs(2);
const MAX_REASON: usize = 200;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Home(#[from] home::Error),
    #[error("cannot listen on {addr}: {source}")]
    Bind {
        addr: String,
        #[source]
        source: io::Error,
    },
}

/// Facts about a task; the page decides how to word them.
#[derive(Serialize)]
struct TaskView {
    issue: String,
    repo: String,
    number: String,
    title: Option<String>,
    /// The log could not be read; nothing else is known.
    unreadable: bool,
    /// Steps done, out of `steps`.
    step: usize,
    steps: usize,
    /// Agent for the current step; `None` once agents are done.
    agent: Option<&'static str>,
    /// The next step's agent is running now.
    running: bool,
    stuck: bool,
    retry_at: Option<u64>,
    last_failure: Option<String>,
    since: u64,
    updated: u64,
}

/// Serve the dashboard on `addr` until the process is stopped.
pub(crate) fn serve(addr: &str) -> Result<(), Error> {
    let home = home::home()?;
    let listener = TcpListener::bind(addr).map_err(|source| Error::Bind {
        addr: addr.to_string(),
        source,
    })?;
    let _ = writeln!(
        io::stdout(),
        "{}",
        json!({ "event": "dashboard_listening", "addr": addr })
    );

    let clients = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        // A failed accept affects only that one connection.
        let Ok(mut stream) = stream else {
            continue;
        };
        if clients.fetch_add(1, Ordering::Relaxed) >= MAX_CLIENTS {
            clients.fetch_sub(1, Ordering::Relaxed);
            let _ = respond(&mut stream, "503 Service Unavailable", "text/plain", "busy");
            continue;
        }
        let home = home.clone();
        let clients = Arc::clone(&clients);
        thread::spawn(move || {
            // Errors here are client disconnects; the server carries on.
            let _ = handle(stream, &home);
            clients.fetch_sub(1, Ordering::Relaxed);
        });
    }
    Ok(())
}

fn handle(mut stream: TcpStream, home: &Path) -> io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut request_line = String::new();
    BufReader::new((&stream).take(MAX_REQUEST_LINE)).read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return respond(&mut stream, "400 Bad Request", "text/plain", "bad request");
    };
    if method != "GET" {
        return respond(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            "GET only",
        );
    }
    match path {
        "/" => respond(&mut stream, "200 OK", "text/html; charset=utf-8", INDEX),
        "/state" => respond_json(&mut stream, &snapshot(home)),
        "/events" => stream_events(&mut stream, home),
        _ => respond(&mut stream, "404 Not Found", "text/plain", "not found"),
    }
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

fn respond_json(stream: &mut TcpStream, value: &impl Serialize) -> io::Result<()> {
    let body = serde_json::to_string(value).map_err(io::Error::other)?;
    respond(stream, "200 OK", "application/json", &body)
}

/// Send the snapshot whenever it changes; a comment line otherwise, which also
/// notices a closed browser tab.
fn stream_events(stream: &mut TcpStream, home: &Path) -> io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
          Cache-Control: no-store\r\nConnection: keep-alive\r\n\r\n",
    )?;
    let mut last = String::new();
    loop {
        let json = serde_json::to_string(&snapshot(home)).map_err(io::Error::other)?;
        if json == last {
            stream.write_all(b": idle\n\n")?;
        } else {
            write!(stream, "data: {json}\n\n")?;
            last = json;
        }
        stream.flush()?;
        thread::sleep(REFRESH);
    }
}

fn snapshot(home: &Path) -> Vec<TaskView> {
    let mut tasks: Vec<TaskView> = task_refs(&home::tasks_root(home))
        .into_iter()
        .map(|issue| view(home, &issue))
        .collect();
    tasks.sort_by_key(|task| std::cmp::Reverse(task.updated));
    tasks
}

fn view(home: &Path, issue: &GitHubUrl) -> TaskView {
    let mut view = TaskView {
        issue: issue.to_string(),
        repo: issue.repo_slug(),
        number: issue.number.to_string(),
        title: None,
        unreadable: true,
        step: 0,
        steps: Step::route().count(),
        agent: None,
        running: false,
        stuck: false,
        retry_at: None,
        last_failure: None,
        since: 0,
        updated: 0,
    };
    let Ok(records) = workflow::read_log(&home::events_path(&home::task_dir(home, issue))) else {
        return view;
    };
    let state = TaskState::replay(&records);
    view.unreadable = false;
    view.title.clone_from(&state.title);
    // PR feedback rounds repeat after the route is done; count one more step then.
    view.steps += usize::from(state.current() == Some(Step::DevelopPrFeedback));
    view.step = state.current().map_or(view.steps, |step| {
        Step::route().take_while(|done| *done != step).count()
    });
    view.agent = state.current().map(|step| step.role().agent());
    view.running = state.turn.is_some();
    view.stuck = state.is_stuck();
    view.retry_at = state.retry_at();
    view.last_failure = state.last_failure.as_deref().map(summary);
    view.since = state.step_since;
    view.updated = state.updated;
    view
}

/// `/tasks/<owner>/<repo>/<number>` → the issue, validated so it is safe as a path.
fn task_ref(rest: &str) -> Option<GitHubUrl> {
    let mut parts = rest.split('/');
    let (Some(owner), Some(repo), Some(number), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    format!("https://github.com/{owner}/{repo}/issues/{number}")
        .parse()
        .ok()
}

/// First line only, bounded: failure reasons can quote tool output.
fn summary(reason: &str) -> String {
    reason
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(MAX_REASON)
        .collect()
}

/// Every `tasks/<owner>/<repo>/<number>` that names a valid issue.
fn task_refs(root: &Path) -> Vec<GitHubUrl> {
    let mut found = Vec::new();
    for owner in subdirs(root) {
        for repo in subdirs(&owner) {
            for number in subdirs(&repo) {
                let rest = [&owner, &repo, &number]
                    .map(|dir| {
                        dir.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    })
                    .join("/");
                found.extend(task_ref(&rest));
            }
        }
    }
    found
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}
