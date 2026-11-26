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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pull_request() -> PullRequest {
        serde_json::from_value(json!({
            "title": "Fix crash",
            "body": "Fixes it.\n",
            "html_url": "https://github.com/acme/widgets/pull/42",
            "user": {"login": "alice"},
            "state": "open",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-02T00:00:00Z",
            "labels": [{"name": "bug"}],
            "base": {"ref": "main", "sha": "base123"},
            "head": {"ref": "fix", "sha": "head456"},
            "merged": false
        }))
        .unwrap()
    }

    #[test]
    fn renders_pull_request_reviews_and_comments() {
        let reviews: Vec<Review> = serde_json::from_value(json!([{
            "user": {"login": "carol"},
            "body": "Looks good.\n",
            "state": "APPROVED",
            "submitted_at": "2026-01-03T00:00:00Z",
            "commit_id": "review789"
        }]))
        .unwrap();
        let review_comments: Vec<ReviewComment> = serde_json::from_value(json!([{
            "user": {"login": "dave"},
            "body": "Use a guard here.\n",
            "path": "src/main.rs",
            "line": 12,
            "original_line": 9,
            "diff_hunk": "@@ -10,3 +10,4 @@\n fn main() {\n+    guard();\n }",
            "created_at": "2026-01-04T00:00:00Z",
            "in_reply_to_id": null
        }]))
        .unwrap();
        let comments: Vec<IssueComment> = serde_json::from_value(json!([{
            "user": {"login": "erin"},
            "body": "Thanks.\n",
            "created_at": "2026-01-05T00:00:00Z"
        }]))
        .unwrap();

        let expected = concat!(
            "# Fix crash\n",
            "\n",
            "- URL: https://github.com/acme/widgets/pull/42\n",
            "- Author: @alice\n",
            "- State: open\n",
            "- Base: main (base123)\n",
            "- Head: fix (head456)\n",
            "- Labels: bug\n",
            "- Created: 2026-01-01T00:00:00Z\n",
            "- Updated: 2026-01-02T00:00:00Z\n",
            "- Latest feedback: #3 at 2026-01-05T00:00:00Z\n",
            "\n",
            "## Body\n",
            "\n",
            "Fixes it.\n",
            "\n",
            "## Feedback (3)\n",
            "\n",
            "### 1. 2026-01-03T00:00:00Z - review by @carol (APPROVED)\n",
            "Commit: review789\n",
            "\n",
            "Looks good.\n",
            "\n",
            "### 2. 2026-01-04T00:00:00Z - review comment by @dave on src/main.rs:12\n",
            "\n",
            "Use a guard here.\n",
            "\n",
            "```diff\n",
            "@@ -10,3 +10,4 @@\n",
            " fn main() {\n",
            "+    guard();\n",
            " }\n",
            "```\n",
            "\n",
            "### 3. 2026-01-05T00:00:00Z - comment by @erin\n",
            "\n",
            "Thanks.\n",
        );

        assert_eq!(
            render_pr(&pull_request(), &reviews, &review_comments, &comments),
            expected
        );
    }

    #[test]
    fn renders_empty_feedback() {
        let md = render_pr(&pull_request(), &[], &[], &[]);
        assert!(md.contains("## Feedback (0)"));
        assert!(md.contains("_No reviews or comments._"));
        assert!(!md.contains("- Latest feedback:"));
    }

    #[test]
    fn orders_feedback_by_timestamp_across_types() {
        let reviews: Vec<Review> = serde_json::from_value(json!([{
            "user": {"login": "alice"},
            "body": null,
            "state": "APPROVED",
            "submitted_at": "2026-01-02T00:00:00Z",
            "commit_id": null
        }]))
        .unwrap();
        let review_comments: Vec<ReviewComment> = serde_json::from_value(json!([{
            "user": {"login": "bob"},
            "body": "Early comment",
            "path": "src/lib.rs",
            "line": 1,
            "original_line": 1,
            "diff_hunk": null,
            "created_at": "2026-01-01T00:00:00Z",
            "in_reply_to_id": null
        }]))
        .unwrap();
        let comments: Vec<IssueComment> = serde_json::from_value(json!([{
            "user": {"login": "carol"},
            "body": "Late comment",
            "created_at": "2026-01-03T00:00:00Z"
        }]))
        .unwrap();

        let md = render_pr(&pull_request(), &reviews, &review_comments, &comments);
        let early = md.find("### 1. 2026-01-01T00:00:00Z").unwrap();
        let middle = md.find("### 2. 2026-01-02T00:00:00Z").unwrap();
        let late = md.find("### 3. 2026-01-03T00:00:00Z").unwrap();

        assert!(early < middle && middle < late);
        assert!(md.contains("- Latest feedback: #3 at 2026-01-03T00:00:00Z"));
    }

    #[test]
    fn orders_pending_reviews_last() {
        let reviews: Vec<Review> = serde_json::from_value(json!([
            {
                "user": {"login": "alice"},
                "body": "Draft",
                "state": "PENDING",
                "submitted_at": null,
                "commit_id": null
            },
            {
                "user": {"login": "bob"},
                "body": "Looks good",
                "state": "APPROVED",
                "submitted_at": "2026-01-01T00:00:00Z",
                "commit_id": null
            }
        ]))
        .unwrap();

        let md = render_pr(&pull_request(), &reviews, &[], &[]);
        let submitted = md.find("### 1. 2026-01-01T00:00:00Z").unwrap();
        let pending = md
            .find("### 2. pending - review by @alice (PENDING)")
            .unwrap();

        assert!(submitted < pending);
        assert!(md.contains("- Latest feedback: #1 at 2026-01-01T00:00:00Z"));
    }

    #[test]
    fn formats_review_comment_locations() {
        let comment = |line: Option<u32>, original_line: Option<u32>| ReviewComment {
            user: None,
            body: None,
            path: "src/main.rs".into(),
            line,
            original_line,
            diff_hunk: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            in_reply_to_id: None,
        };

        assert_eq!(
            review_location(&comment(Some(12), Some(9))),
            "src/main.rs:12"
        );
        assert_eq!(review_location(&comment(None, Some(9))), "src/main.rs:9");
        assert_eq!(review_location(&comment(None, None)), "src/main.rs");
    }
}
