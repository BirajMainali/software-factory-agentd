//! Fetch a GitHub pull request with its reviews and comments as Markdown.

use crate::github::{
    Error, GitHubUrl, Label, User, author_login, gh, gh_json, label_names, push_verbatim,
};
use serde::{Deserialize, de::DeserializeOwned};
use std::cmp::Ordering;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const PR_FEEDBACK_DIR: &str = "pr-feedback";
/// Every fetch writes the next numbered file, so earlier feedback is kept.
const PR_FEEDBACK_SUFFIX: &str = "-pr-feedback.md";

#[derive(Debug, Deserialize)]
struct PullRequest {
    title: String,
    body: Option<String>,
    html_url: String,
    user: Option<User>,
    state: String,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    labels: Vec<Label>,
    base: Branch,
    head: Branch,
    #[serde(default)]
    merged: bool,
}

#[derive(Debug, Deserialize)]
struct Branch {
    #[serde(rename = "ref")]
    name: String,
    sha: String,
}

#[derive(Debug, Deserialize)]
struct Review {
    user: Option<User>,
    body: Option<String>,
    state: String,
    submitted_at: Option<String>,
    commit_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewComment {
    user: Option<User>,
    body: Option<String>,
    path: String,
    line: Option<u32>,
    original_line: Option<u32>,
    diff_hunk: Option<String>,
    created_at: String,
    in_reply_to_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct IssueComment {
    user: Option<User>,
    body: Option<String>,
    created_at: String,
}

/// Fetch `url` with `gh` and write the next `<dir>/pr-feedback/<n>-pr-feedback.md`.
///
/// # Errors
///
/// Returns an error if the URL is not a GitHub pull request, `gh` is missing or
/// fails, its JSON cannot be parsed, or the output cannot be written.
pub(crate) fn fetch_pr(url: &str, dir: &Path) -> Result<(), Error> {
    let pr_ref: GitHubUrl = url.parse()?;
    let snapshot = fetch(&pr_ref)?;
    let path = write_next(&dir.join(PR_FEEDBACK_DIR), &snapshot.markdown)?;
    // Status output; a closed pipe must not turn a successful write into a panic.
    let _ = writeln!(std::io::stdout(), "wrote {}", path.display());
    Ok(())
}

/// Write `markdown` to the next number, claiming it exclusively so two fetches
/// racing for the same number can never overwrite one file.
fn write_next(dir: &Path, markdown: &str) -> Result<PathBuf, Error> {
    if let Err(source) = fs::create_dir_all(dir) {
        return Err(Error::CreateDir {
            path: dir.to_path_buf(),
            source,
        });
    }
    let mut n = next_number(dir)?;
    loop {
        let path = dir.join(format!("{n}{PR_FEEDBACK_SUFFIX}"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(markdown.as_bytes())
                    .map_err(|source| Error::Write {
                        path: path.clone(),
                        source,
                    })?;
                return Ok(path);
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => n += 1,
            Err(source) => return Err(Error::Write { path, source }),
        }
    }
}

/// One past the highest `<n>-pr-feedback.md` already in `dir`; 1 if there is none.
fn next_number(dir: &Path) -> Result<u32, Error> {
    let read_error = |source| Error::ReadDir {
        path: dir.to_path_buf(),
        source,
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(1),
        Err(source) => return Err(read_error(source)),
    };
    let mut highest = 0;
    for entry in entries {
        let name = entry.map_err(read_error)?.file_name();
        let n = name
            .to_str()
            .and_then(|name| name.strip_suffix(PR_FEEDBACK_SUFFIX))
            .and_then(|n| n.parse().ok());
        if let Some(n) = n {
            highest = u32::max(highest, n);
        }
    }
    Ok(highest + 1)
}

/// A pull request rendered as Markdown, plus a fingerprint that changes when a
/// review or comment is added.
#[derive(Debug)]
pub(crate) struct Snapshot {
    pub(crate) markdown: String,
    pub(crate) key: String,
    /// The PR is still open, so feedback can still be acted on.
    pub(crate) open: bool,
    /// Reviews and comments already on the PR.
    pub(crate) feedback: usize,
    /// Labels currently on the PR; the factory routes on two of them.
    pub(crate) labels: Vec<String>,
}

/// The developer adds this when the work is ready for review.
pub(crate) const READY_LABEL: &str = "factory-ready";
/// A reviewer adds this to approve; the factory merges the PR.
pub(crate) const APPROVED_LABEL: &str = "factory-approved";

impl Snapshot {
    pub(crate) fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|name| name == label)
    }
}

/// Add or remove a label on a PR.
pub(crate) fn label(pr_ref: &GitHubUrl, name: &str, on: bool) -> Result<(), Error> {
    let flag = if on { "--add-label" } else { "--remove-label" };
    gh("pr edit", &["pr", "edit", &pr_ref.to_string(), flag, name])?;
    Ok(())
}

/// Squash-merge the PR, matching how the repositories merge.
pub(crate) fn merge(pr_ref: &GitHubUrl) -> Result<(), Error> {
    gh(
        "pr merge",
        &["pr", "merge", &pr_ref.to_string(), "--squash"],
    )?;
    Ok(())
}

/// Fetch a pull request with `gh` and render it as Markdown: body, reviews, inline
/// review comments, and conversation comments, in time order. Bodies are verbatim.
///
/// # Errors
///
/// Returns an error if the reference is not a pull request, `gh` fails, or its JSON
/// is invalid.
pub(crate) fn fetch(pr_ref: &GitHubUrl) -> Result<Snapshot, Error> {
    fetch_since(pr_ref, None)
}

/// Like [`fetch`], but only renders feedback newer than `since` (an RFC3339 time).
/// Bounds a feedback round to what the developer has not been given yet.
pub(crate) fn fetch_since(pr_ref: &GitHubUrl, since: Option<&str>) -> Result<Snapshot, Error> {
    pr_ref.expect_pull()?;
    let pr: PullRequest = gh_json(
        "api",
        &[
            "api",
            &pr_ref.repo_path(&format!("pulls/{}", pr_ref.number)),
        ],
    )?;
    let reviews: Vec<Review> = fetch_pages(pr_ref, &format!("pulls/{}/reviews", pr_ref.number))?;
    let review_comments: Vec<ReviewComment> =
        fetch_pages(pr_ref, &format!("pulls/{}/comments", pr_ref.number))?;
    let comments: Vec<IssueComment> =
        fetch_pages(pr_ref, &format!("issues/{}/comments", pr_ref.number))?;
    Ok(render_pr(&pr, &reviews, &review_comments, &comments, since))
}

/// The newest pull request that cross-references `issue`, if any.
///
/// # Errors
///
/// Returns an error if `gh` fails or its JSON is invalid.
pub(crate) fn for_issue(issue: &GitHubUrl) -> Result<Option<GitHubUrl>, Error> {
    let path = issue.repo_path(&format!("issues/{}/timeline", issue.number));
    let pages: Vec<Vec<TimelineEvent>> = gh_json("api", &["api", "--paginate", "--slurp", &path])?;
    Ok(pages
        .into_iter()
        .flatten()
        .filter_map(|event| event.source)
        .filter(|source| source.issue.pull_request.is_some())
        .map(|source| source.issue.html_url)
        .next_back()
        .and_then(|url| url.parse().ok()))
}

#[derive(Debug, Deserialize)]
struct TimelineEvent {
    source: Option<CrossReference>,
}

#[derive(Debug, Deserialize)]
struct CrossReference {
    issue: LinkedIssue,
}

#[derive(Debug, Deserialize)]
struct LinkedIssue {
    html_url: String,
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

fn fetch_pages<T: DeserializeOwned>(pr_ref: &GitHubUrl, endpoint: &str) -> Result<Vec<T>, Error> {
    let path = pr_ref.repo_path(endpoint);
    let pages: Vec<Vec<T>> = gh_json("api", &["api", "--paginate", "--slurp", &path])?;
    Ok(pages.into_iter().flatten().collect())
}

enum Feedback<'a> {
    Review(&'a Review),
    ReviewComment(&'a ReviewComment),
    Comment(&'a IssueComment),
}

impl Feedback<'_> {
    fn timestamp(&self) -> Option<&str> {
        match self {
            Self::Review(review) => review.submitted_at.as_deref(),
            Self::ReviewComment(comment) => Some(comment.created_at.as_str()),
            Self::Comment(comment) => Some(comment.created_at.as_str()),
        }
    }
}

fn render_pr(
    pr: &PullRequest,
    reviews: &[Review],
    review_comments: &[ReviewComment],
    comments: &[IssueComment],
    since: Option<&str>,
) -> Snapshot {
    let mut feedback = Vec::with_capacity(reviews.len() + review_comments.len() + comments.len());
    feedback.extend(reviews.iter().map(Feedback::Review));
    feedback.extend(review_comments.iter().map(Feedback::ReviewComment));
    feedback.extend(comments.iter().map(Feedback::Comment));
    // GitHub timestamps are RFC3339 UTC, so string order is chronological.
    // Reviews without a submission time (pending drafts) sort last.
    feedback.sort_by(|a, b| match (a.timestamp(), b.timestamp()) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });

    let mut out = String::new();
    let _ = writeln!(out, "# {}", pr.title);
    out.push('\n');
    let _ = writeln!(out, "- URL: {}", pr.html_url);
    let _ = writeln!(out, "- Author: @{}", author_login(pr.user.as_ref()));
    let _ = writeln!(out, "- State: {}", pr.state);
    let _ = writeln!(out, "- Base: {} ({})", pr.base.name, pr.base.sha);
    let _ = writeln!(out, "- Head: {} ({})", pr.head.name, pr.head.sha);
    if pr.merged {
        let _ = writeln!(out, "- Merged: yes");
    }

    let labels = label_names(&pr.labels);
    if !labels.is_empty() {
        let _ = writeln!(out, "- Labels: {labels}");
    }

    let _ = writeln!(out, "- Created: {}", pr.created_at);
    let _ = writeln!(out, "- Updated: {}", pr.updated_at);

    // Sorted ascending, so the last item carrying a timestamp is the latest.
    if let Some(index) = feedback.iter().rposition(|item| item.timestamp().is_some()) {
        let _ = writeln!(
            out,
            "- Latest feedback: #{} at {}",
            index + 1,
            feedback[index].timestamp().unwrap_or("")
        );
    }

    out.push_str("\n## Body\n\n");
    push_verbatim(&mut out, pr.body.as_deref().unwrap_or(""));

    // One round only needs what the previous round did not show.
    let shown: Vec<&Feedback> = feedback
        .iter()
        .filter(|item| is_new(item.timestamp(), since))
        .collect();
    match since.filter(|ts| !ts.is_empty()) {
        Some(ts) => {
            let _ = writeln!(out, "\n## New feedback since {ts} ({})", shown.len());
        }
        None => {
            let _ = writeln!(out, "\n## Feedback ({})", shown.len());
        }
    }
    if shown.is_empty() {
        out.push_str("\n_No new reviews or comments._\n");
    }

    for (index, item) in shown.iter().enumerate() {
        push_item(&mut out, index + 1, item);
    }

    Snapshot {
        key: feedback_key(&feedback),
        markdown: out,
        open: pr.state == "open",
        feedback: feedback.len(),
        labels: pr.labels.iter().map(|label| label.name.clone()).collect(),
    }
}

/// Changes when a review or comment is added: how many there are, and when the
/// latest one was submitted.
// shortcut: edited bodies keep the same count and timestamp, so they do not wake
// the developer; watch bodies too if that must route back.
/// One review or comment, under its number in the list.
fn push_item(out: &mut String, number: usize, item: &Feedback<'_>) {
    let timestamp = item.timestamp().unwrap_or("pending");
    match item {
        Feedback::Review(review) => {
            let _ = writeln!(
                out,
                "\n### {number}. {timestamp} - review by @{} ({})",
                author_login(review.user.as_ref()),
                review.state
            );
            if let Some(commit) = &review.commit_id {
                let _ = writeln!(out, "Commit: {commit}");
            }
            out.push('\n');
            push_body(out, review.body.as_deref());
        }
        Feedback::ReviewComment(comment) => {
            let _ = writeln!(
                out,
                "\n### {number}. {timestamp} - review comment by @{} on {}",
                author_login(comment.user.as_ref()),
                review_location(comment)
            );
            if let Some(parent) = comment.in_reply_to_id {
                let _ = writeln!(out, "In reply to: {parent}");
            }
            out.push('\n');
            push_body(out, comment.body.as_deref());

            if let Some(hunk) = comment.diff_hunk.as_deref().filter(|hunk| !hunk.is_empty()) {
                out.push_str("\n```diff\n");
                out.push_str(hunk);
                if !hunk.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n");
            }
        }
        Feedback::Comment(comment) => {
            let _ = writeln!(
                out,
                "\n### {number}. {timestamp} - comment by @{}",
                author_login(comment.user.as_ref())
            );
            out.push('\n');
            push_body(out, comment.body.as_deref());
        }
    }
}

/// Whether feedback at `timestamp` came after the cutoff `since`.
fn is_new(timestamp: Option<&str>, since: Option<&str>) -> bool {
    match (since, timestamp) {
        (None, _) => true,
        (Some(since), Some(timestamp)) => timestamp > since,
        (Some(_), None) => false,
    }
}

fn feedback_key(feedback: &[Feedback<'_>]) -> String {
    let latest = feedback
        .iter()
        .rev()
        .find_map(|item| item.timestamp())
        .unwrap_or("");
    format!("{}:{latest}", feedback.len())
}

fn review_location(comment: &ReviewComment) -> String {
    match comment.line.or(comment.original_line) {
        Some(line) => format!("{}:{}", comment.path, line),
        None => comment.path.clone(),
    }
}

fn push_body(out: &mut String, body: Option<&str>) {
    match body {
        Some(body) if !body.trim().is_empty() => push_verbatim(out, body),
        _ => out.push_str("_(no body)_\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_feedback_is_numbered_per_fetch() {
        let dir = std::env::temp_dir().join(format!("agentd-pr-feedback-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(next_number(&dir).unwrap(), 1);

        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("1-pr-feedback.md"), "one").unwrap();
        fs::write(dir.join("3-pr-feedback.md"), "three").unwrap();
        fs::write(dir.join("notes.md"), "not feedback").unwrap();
        assert_eq!(next_number(&dir).unwrap(), 4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_feedback_after_the_cutoff_is_new() {
        // (timestamp, since)
        assert!(is_new(None, None));
        assert!(is_new(Some("2026-01-01T00:00:00Z"), None));
        assert!(!is_new(None, Some("2026-01-01T00:00:00Z")));
        assert!(is_new(
            Some("2026-01-02T00:00:00Z"),
            Some("2026-01-01T00:00:00Z")
        ));
        assert!(!is_new(
            Some("2026-01-01T00:00:00Z"),
            Some("2026-01-02T00:00:00Z")
        ));
        assert!(!is_new(
            Some("2026-01-02T00:00:00Z"),
            Some("2026-01-02T00:00:00Z")
        ));
    }

    #[test]
    fn feedback_key_changes_when_feedback_arrives() {
        let review = Review {
            user: None,
            body: None,
            state: "APPROVED".to_string(),
            submitted_at: Some("2026-01-01T00:00:00Z".to_string()),
            commit_id: None,
        };
        assert_ne!(
            feedback_key(&[]),
            feedback_key(&[Feedback::Review(&review)])
        );
        assert_eq!(
            feedback_key(&[Feedback::Review(&review)]),
            feedback_key(&[Feedback::Review(&review)])
        );
    }

    #[test]
    fn each_fetch_keeps_the_previous_file() {
        let dir = std::env::temp_dir().join(format!("agentd-pr-write-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_next(&dir, "first").unwrap();
        write_next(&dir, "second").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("1-pr-feedback.md")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_to_string(dir.join("2-pr-feedback.md")).unwrap(),
            "second"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
