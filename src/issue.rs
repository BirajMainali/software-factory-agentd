//! Fetch a GitHub issue and render it as Markdown.

use crate::github::{
    FetchError, GitHubUrl, Label, User, author_login, gh, label_names, push_verbatim,
    write_markdown,
};
use serde::Deserialize;
use std::fmt::Write as _;
use std::path::Path;

const ISSUE_JSON_FIELDS: &str = "title,body,author,state,labels,createdAt,updatedAt,url,comments";
const ISSUE_FILE: &str = "issue.md";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Issue {
    title: String,
    body: Option<String>,
    url: String,
    author: Option<User>,
    state: String,
    #[serde(default)]
    labels: Vec<Label>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    comments: Vec<Comment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Comment {
    author: Option<User>,
    body: Option<String>,
    created_at: String,
}

/// Fetch `url` with `gh` and write `<dir>/issue.md`.
///
/// # Errors
///
/// Returns an error if the URL is not a GitHub issue, `gh` is missing or fails,
/// its JSON cannot be parsed, or the output cannot be written.
pub(crate) fn fetch_issue(url: &str, dir: &Path) -> Result<(), FetchError> {
    let issue_ref: GitHubUrl = url.parse()?;
    issue_ref.expect_issue()?;
    let issue_url = issue_ref.to_string();

    let bytes = gh(
        "issue view",
        &["issue", "view", &issue_url, "--json", ISSUE_JSON_FIELDS],
    )?;
    let issue: Issue = serde_json::from_slice(&bytes).map_err(FetchError::GhJson)?;

    // GitHub shares numbering between issues and pull requests, so an
    // /issues/<n> URL can resolve to a pull request.
    if is_pull_request(&issue.url) {
        return Err(FetchError::NotAnIssue(issue.url));
    }

    write_markdown(dir, ISSUE_FILE, &render_issue(&issue))
}

fn is_pull_request(url: &str) -> bool {
    url.split('/').nth(5) == Some("pull")
}

fn render_issue(issue: &Issue) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}", issue.title);
    out.push('\n');
    let _ = writeln!(out, "- URL: {}", issue.url);
    let _ = writeln!(out, "- Author: @{}", author_login(issue.author.as_ref()));
    let _ = writeln!(out, "- State: {}", issue.state);

    let labels = label_names(&issue.labels);
    if !labels.is_empty() {
        let _ = writeln!(out, "- Labels: {labels}");
    }

    let _ = writeln!(out, "- Created: {}", issue.created_at);
    let _ = writeln!(out, "- Updated: {}", issue.updated_at);
    out.push_str("\n## Body\n\n");
    push_verbatim(&mut out, issue.body.as_deref().unwrap_or(""));

    if !issue.comments.is_empty() {
        out.push_str("\n## Comments\n");
        for comment in &issue.comments {
            let _ = write!(
                out,
                "\n### @{} - {}\n\n",
                author_login(comment.author.as_ref()),
                comment.created_at
            );
            push_verbatim(&mut out, comment.body.as_deref().unwrap_or(""));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_pull_request_urls() {
        assert!(is_pull_request("https://github.com/acme/widgets/pull/42"));
        assert!(!is_pull_request(
            "https://github.com/acme/widgets/issues/42"
        ));
        assert!(!is_pull_request("not a url"));
    }

    #[test]
    fn renders_issue_and_comments() {
        let issue: Issue = serde_json::from_value(json!({
            "title": "Crash on start",
            "body": "Steps:\n1. run it",
            "url": "https://github.com/acme/widgets/issues/42",
            "author": {"login": "alice"},
            "state": "OPEN",
            "labels": [{"name": "bug"}, {"name": "runtime"}],
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-02T00:00:00Z",
            "comments": [{
                "author": {"login": "bob"},
                "body": "Same here",
                "createdAt": "2026-01-03T00:00:00Z"
            }]
        }))
        .unwrap();

        let expected = concat!(
            "# Crash on start\n",
            "\n",
            "- URL: https://github.com/acme/widgets/issues/42\n",
            "- Author: @alice\n",
            "- State: OPEN\n",
            "- Labels: bug, runtime\n",
            "- Created: 2026-01-01T00:00:00Z\n",
            "- Updated: 2026-01-02T00:00:00Z\n",
            "\n",
            "## Body\n",
            "\n",
            "Steps:\n",
            "1. run it\n",
            "\n",
            "## Comments\n",
            "\n",
            "### @bob - 2026-01-03T00:00:00Z\n",
            "\n",
            "Same here\n",
        );

        assert_eq!(render_issue(&issue), expected);
    }

    #[test]
    fn renders_issue_without_optional_fields() {
        let issue: Issue = serde_json::from_value(json!({
            "title": "Empty",
            "url": "https://github.com/acme/widgets/issues/1",
            "state": "CLOSED",
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
        }))
        .unwrap();

        let md = render_issue(&issue);
        assert!(md.contains("- Author: @ghost"));
        assert!(!md.contains("- Labels:"));
        assert!(!md.contains("## Comments"));
        assert!(md.contains("## Body\n\n\n"));
    }
}
