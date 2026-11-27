//! Fetch a GitHub issue and render it as Markdown.

use crate::github::{
    Error, GitHubUrl, Label, User, author_login, gh_json, label_names, push_verbatim,
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

pub(crate) struct FetchedIssue {
    pub(crate) title: String,
    pub(crate) markdown: String,
}

/// Fetch `url` with `gh` and write `<dir>/issue.md`.
///
/// # Errors
///
/// Returns an error if the URL is not a GitHub issue, `gh` is missing or fails,
/// its JSON cannot be parsed, or the output cannot be written.
pub(crate) fn fetch_issue(url: &str, dir: &Path) -> Result<(), Error> {
    let issue_ref: GitHubUrl = url.parse()?;
    write_markdown(dir, ISSUE_FILE, &fetch(&issue_ref)?.markdown)
}

/// Fetch an issue with `gh` and render it as Markdown.
///
/// # Errors
///
/// Returns an error if the reference is not an issue, `gh` fails, or its JSON is invalid.
pub(crate) fn fetch(issue_ref: &GitHubUrl) -> Result<FetchedIssue, Error> {
    issue_ref.expect_issue()?;
    let issue_url = issue_ref.to_string();
    let issue: Issue = gh_json(
        "issue view",
        &["issue", "view", &issue_url, "--json", ISSUE_JSON_FIELDS],
    )?;

    // GitHub shares numbering between issues and pull requests, so an
    // /issues/<n> URL can resolve to a pull request.
    if is_pull_request(&issue.url) {
        return Err(Error::NotAnIssue(issue.url));
    }

    Ok(FetchedIssue {
        markdown: render_issue(&issue),
        title: issue.title,
    })
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
