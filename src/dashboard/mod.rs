//! Read-only dashboard. Replays every task log into its derived state and streams it
//! to browsers over server-sent events. Owns no state and writes nothing.
//! Shows task status and the task's own numbered artifacts; no other repository
//! content.

use crate::github::GitHubUrl;
use crate::home;
use crate::workflow::{self, TaskState};
use pulldown_cmark::{Event, Options, Parser, html};
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
const MAX_DOC_BYTES: u64 = 2 * 1024 * 1024;

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
#[allow(clippy::struct_excessive_bools)] // independent facts the page renders separately
#[derive(Serialize)]
struct TaskView {
    issue: String,
    repo: String,
    number: String,
    title: Option<String>,
    /// The log could not be read; nothing else is known.
    unreadable: bool,
    /// Stages that ran, in order; repeats show each pass through the route.
    stages: Vec<&'static str>,
    /// Index of the stage running now; `None` once the task is finished.
    current: Option<usize>,
    /// Agent for the current step; `None` once agents are done.
    agent: Option<&'static str>,
    /// The next step's agent is running now.
    running: bool,
    stuck: bool,
    retry_at: Option<u64>,
    last_failure: Option<String>,
    /// Where the task's artifacts are, once a worker recorded it.
    work_path: Option<String>,
    /// The task's pull request, once the supervisor found it.
    pr: Option<String>,
    /// The PR is merged or closed; the supervisor has exited.
    pr_closed: bool,
    /// The task's own numbered artifacts, relative to `work_path`.
    documents: Vec<String>,
    since: u64,
    updated: u64,
}

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
        doc if doc.starts_with("/doc/") => serve_document(&mut stream, home, &doc[5..]),
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
        stages: Vec::new(),
        current: None,
        agent: None,
        running: false,
        stuck: false,
        retry_at: None,
        last_failure: None,
        work_path: None,
        pr: None,
        pr_closed: false,
        documents: Vec::new(),
        since: 0,
        updated: 0,
    };
    let Ok(records) = workflow::read_log(&home::events_path(&home::task_dir(home, issue))) else {
        return view;
    };
    let state = TaskState::replay(&records);
    view.unreadable = false;
    view.title.clone_from(&state.title);
    let mut labels: Vec<&'static str> = Vec::new();
    for step in state.history.iter().chain(state.current().iter()) {
        let label = step.stage_label();
        if labels.last().copied() != Some(label) {
            labels.push(label);
        }
    }
    view.current = (!state.is_finished()).then(|| labels.len().saturating_sub(1));
    view.stages = labels;
    view.agent = state.current().map(|step| step.role().agent());
    view.running = state.turn.is_some();
    view.stuck = state.is_stuck();
    view.retry_at = state.retry_at();
    view.last_failure = state.last_failure.as_deref().map(summary);
    view.work_path.clone_from(&state.work_path);
    view.pr = state.pr().map(str::to_string);
    view.pr_closed = state.pr_closed;
    view.documents = state
        .work_path
        .as_deref()
        .map(|dir| documents(Path::new(dir)))
        .unwrap_or_default();
    view.since = state.step_since;
    view.updated = state.updated;
    view
}

/// `owner/repo/number` → the issue URL, validated so the parts are safe as paths.
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

/// The task's own numbered artifacts under `dir`, in step order.
fn documents(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if dir.join("issue.md").is_file() {
        names.push("issue.md".to_string());
    }
    for folder in ["plan", "review", "development", "pr-feedback"] {
        let mut found: Vec<String> = fs::read_dir(dir.join(folder))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().is_file())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| is_document(&format!("{folder}/{name}")))
                    .collect()
            })
            .unwrap_or_default();
        found.sort_by_key(|name| number_of(name));
        names.extend(found.into_iter().map(|name| format!("{folder}/{name}")));
    }
    names
}

/// Only the artifacts agentd names can be read; nothing else in the folder.
fn is_document(name: &str) -> bool {
    let Some((folder, file)) = name.split_once('/') else {
        return name == "issue.md";
    };
    let Some((number, rest)) = file.split_once('-') else {
        return false;
    };
    !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && match folder {
            "plan" => rest == "plan.md",
            "development" => rest == "development.md",
            "pr-feedback" => rest == "pr-feedback.md",
            "review" => matches!(rest, "security.md" | "code.md" | "uiux.md"),
            _ => false,
        }
}

fn number_of(name: &str) -> u32 {
    name.split_once('-')
        .and_then(|(number, _)| number.parse().ok())
        .unwrap_or(u32::MAX)
}

/// Serve one of the task's artifacts as plain text.
fn serve_document(stream: &mut TcpStream, home: &Path, rest: &str) -> io::Result<()> {
    let not_found =
        |stream: &mut TcpStream| respond(stream, "404 Not Found", "text/plain", "not found");
    let Some((issue, name)) = split_document(rest) else {
        return not_found(stream);
    };
    let task = home::task_dir(home, &issue);
    let Ok(records) = workflow::read_log(&home::events_path(&task)) else {
        return not_found(stream);
    };
    let Some(work_path) = TaskState::replay(&records).work_path else {
        return not_found(stream);
    };
    let Ok(root) = fs::canonicalize(&work_path) else {
        return not_found(stream);
    };
    let Ok(path) = fs::canonicalize(root.join(&name)) else {
        return not_found(stream);
    };
    // Belt and braces: a symlink inside the work path must not escape it.
    if !path.starts_with(&root) || !path.is_file() {
        return not_found(stream);
    }
    match fs::metadata(&path) {
        Ok(meta) if meta.len() <= MAX_DOC_BYTES => {}
        Ok(_) => return respond(stream, "413 Content Too Large", "text/plain", "too large"),
        Err(_) => return not_found(stream),
    }
    match fs::read_to_string(&path) {
        // Every artifact agentd writes is markdown; show it as written, not as source.
        Ok(text) => respond(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            &markdown_page(&text, &name),
        ),
        Err(_) => not_found(stream),
    }
}

/// A standalone page for one artifact. `name` is already constrained by
/// [`is_document`] to digits, dashes and known words, so it is safe in the title.
fn markdown_page(markdown: &str, name: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    // Artifacts quote issue text and tool output; raw HTML in them must not run.
    let parser = Parser::new_ext(markdown, options).map(|event| match event {
        Event::Html(text) | Event::InlineHtml(text) => Event::Text(text),
        other => other,
    });
    let mut body = String::new();
    html::push_html(&mut body, parser);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name}</title>
<style>
  :root {{ color-scheme: light dark; --text: #1f2328; --muted: #656d76; --line: #d8dee4; --panel: #f6f8fa; --accent: #0969da; }}
  @media (prefers-color-scheme: dark) {{ :root {{ --text: #e6edf3; --muted: #8d96a0; --line: #30363d; --panel: #161b22; --accent: #4493f8; }} }}
  body {{ font: 15px/1.6 system-ui, sans-serif; color: var(--text); margin: 0; padding: 2rem; max-width: 52rem; }}
  h1, h2, h3 {{ line-height: 1.25; margin: 1.5em 0 .5em; }}
  h1 {{ font-size: 1.5rem; }} h2 {{ font-size: 1.2rem; }} h3 {{ font-size: 1rem; }}
  p, ul, ol, table {{ margin: .75em 0; }}
  a {{ color: var(--accent); }}
  code {{ background: var(--panel); padding: .1em .3em; border-radius: 4px; font-size: .875em; }}
  pre {{ background: var(--panel); padding: 1rem; border-radius: 6px; overflow: auto; }}
  pre code {{ background: none; padding: 0; }}
  blockquote {{ margin: .75em 0; padding: 0 1em; color: var(--muted); border-left: 3px solid var(--line); }}
  table {{ border-collapse: collapse; }} th, td {{ padding: .35rem .6rem; border: 1px solid var(--line); text-align: left; }}
  li.task-list-item {{ list-style: none; }} li.task-list-item input {{ margin-right: .4em; }}
</style>
</head>
<body>
<article>{body}</article>
</body>
</html>
"#
    )
}

/// `/doc/<owner>/<repo>/<n>/issue.md` or `/doc/<owner>/<repo>/<n>/<folder>/<file>`.
fn split_document(rest: &str) -> Option<(GitHubUrl, String)> {
    let parts: Vec<&str> = rest.split('/').collect();
    let (owner, repo, number, name) = match parts.as_slice() {
        [owner, repo, number, file] => (*owner, *repo, *number, (*file).to_string()),
        [owner, repo, number, folder, file] => (*owner, *repo, *number, format!("{folder}/{file}")),
        _ => return None,
    };
    let issue: GitHubUrl = format!("https://github.com/{owner}/{repo}/issues/{number}")
        .parse()
        .ok()?;
    is_document(&name).then_some((issue, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_agentd_artifacts_are_readable() {
        for name in [
            "issue.md",
            "plan/1-plan.md",
            "review/2-code.md",
            "review/3-uiux.md",
            "development/4-development.md",
            "pr-feedback/5-pr-feedback.md",
        ] {
            assert!(is_document(name), "{name}");
        }
        for name in [
            "notes.md",
            "plan/notes.md",
            "plan/1-plan.md.bak",
            ".git/config",
            "review/1-code.txt",
            "../issue.md",
            "issue.md/1-plan.md",
        ] {
            assert!(!is_document(name), "{name}");
        }
    }

    #[test]
    fn markdown_pages_render_instead_of_showing_source() {
        let page = markdown_page(
            "# Plan\n\n- [x] done\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n<script>alert(1)</script>",
            "plan/1-plan.md",
        );
        assert!(page.contains("<h1>Plan</h1>"), "{page}");
        assert!(page.contains("type=\"checkbox\""), "{page}");
        assert!(page.contains("<table>"), "{page}");
        assert!(page.contains("&lt;script&gt;"), "{page}");
        assert!(!page.contains("<script>alert"), "{page}");
        assert!(page.contains("<title>plan/1-plan.md</title>"), "{page}");
    }

    #[test]
    fn documents_are_listed_in_order_without_extra_files() {
        let dir = std::env::temp_dir().join(format!("agentd-docs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("plan")).unwrap();
        fs::write(dir.join("issue.md"), "issue").unwrap();
        fs::write(dir.join("plan/2-plan.md"), "two").unwrap();
        fs::write(dir.join("plan/10-plan.md"), "ten").unwrap();
        fs::write(dir.join("plan/notes.md"), "skip").unwrap();
        assert_eq!(
            documents(&dir),
            ["issue.md", "plan/2-plan.md", "plan/10-plan.md"]
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
