//! Fetch a GitHub pull request with its reviews and comments as Markdown.

use crate::github::{
    FetchError, GitHubUrl, Label, User, author_login, gh, label_names, push_verbatim,
    write_markdown,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::cmp::Ordering;
use std::fmt::Write as _;
use std::path::Path;

const PR_FILE: &str = "pr.md";

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

/// Fetch `url` with `gh` and write `<dir>/pr.md`.
///
/// Includes the PR body, reviews, inline review comments, and conversation
/// comments. Bodies are written verbatim.
///
/// # Errors
///
/// Returns an error if the URL is not a GitHub pull request, `gh` is missing or
/// fails, its JSON cannot be parsed, or the output cannot be written.
pub(crate) fn fetch_pr(url: &str, dir: &Path) -> Result<(), FetchError> {
    let pr_ref: GitHubUrl = url.parse()?;
    pr_ref.expect_pull()?;

    let pr_path = pr_ref.repo_path(&format!("pulls/{}", pr_ref.number));
    let bytes = gh("api", &["api", &pr_path])?;
    let pr: PullRequest = serde_json::from_slice(&bytes).map_err(FetchError::GhJson)?;

    let reviews: Vec<Review> = fetch_pages(&pr_ref, &format!("pulls/{}/reviews", pr_ref.number))?;
    let review_comments: Vec<ReviewComment> =
        fetch_pages(&pr_ref, &format!("pulls/{}/comments", pr_ref.number))?;
    let comments: Vec<IssueComment> =
        fetch_pages(&pr_ref, &format!("issues/{}/comments", pr_ref.number))?;

    let markdown = render_pr(&pr, &reviews, &review_comments, &comments);
    write_markdown(dir, PR_FILE, &markdown)
}

fn fetch_pages<T: DeserializeOwned>(
    pr_ref: &GitHubUrl,
    endpoint: &str,
) -> Result<Vec<T>, FetchError> {
    let path = pr_ref.repo_path(endpoint);
    let bytes = gh("api", &["api", "--paginate", "--slurp", &path])?;
    let pages: Vec<Vec<T>> = serde_json::from_slice(&bytes).map_err(FetchError::GhJson)?;
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
) -> String {
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
    let latest = feedback
        .iter()
        .enumerate()
        .rev()
        .find(|(_, item)| item.timestamp().is_some());
    if let Some((index, item)) = latest {
        let _ = writeln!(
            out,
            "- Latest feedback: #{} at {}",
            index + 1,
            item.timestamp().unwrap_or("")
        );
    }

    out.push_str("\n## Body\n\n");
    push_verbatim(&mut out, pr.body.as_deref().unwrap_or(""));

    let _ = writeln!(out, "\n## Feedback ({})", feedback.len());
    if feedback.is_empty() {
        out.push_str("\n_No reviews or comments._\n");
    }

    for (index, item) in feedback.iter().enumerate() {
        let number = index + 1;
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
                push_body(&mut out, review.body.as_deref());
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
                push_body(&mut out, comment.body.as_deref());

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
                push_body(&mut out, comment.body.as_deref());
            }
        }
    }

    out
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
