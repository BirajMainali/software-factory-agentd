//! Open, inspect, and merge the task's pull request.

use crate::github::{Error, GitHubUrl, gh, gh_json};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PullRequest {
    pub(crate) number: u64,
    pub(crate) url: String,
}

/// Where the pull request stands on GitHub.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PullState {
    Open { head: String },
    Merged,
    Closed,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullView {
    state: String,
    head_ref_oid: String,
}

/// The open pull request for `branch`, created if none exists yet.
pub(crate) fn find_or_create(
    issue: &GitHubUrl,
    branch: &str,
    title: &str,
) -> Result<PullRequest, Error> {
    let repo = issue.repo_slug();
    let open: Vec<PullRequest> = gh_json(
        "pr list",
        &[
            "pr",
            "list",
            "-R",
            &repo,
            "--head",
            branch,
            "--state",
            "open",
            "--json",
            "number,url",
        ],
    )?;
    if let Some(pull) = open.into_iter().next() {
        return Ok(pull);
    }

    let body = format!(
        "Closes #{}\n\nOpened by Software Factory Runtime.",
        issue.number
    );
    let stdout = gh(
        "pr create",
        &[
            "pr", "create", "-R", &repo, "--head", branch, "--title", title, "--body", &body,
        ],
    )?;
    let url = String::from_utf8_lossy(&stdout).trim().to_string();
    let number = url
        .rsplit('/')
        .next()
        .and_then(|number| number.parse().ok())
        .ok_or_else(|| Error::GhFailed {
            command: "pr create",
            message: format!("unexpected output: {url}"),
        })?;
    Ok(PullRequest { number, url })
}

pub(crate) fn state(issue: &GitHubUrl, number: u64) -> Result<PullState, Error> {
    let view: PullView = gh_json(
        "pr view",
        &[
            "pr",
            "view",
            &number.to_string(),
            "-R",
            &issue.repo_slug(),
            "--json",
            "state,headRefOid",
        ],
    )?;
    Ok(match view.state.as_str() {
        "MERGED" => PullState::Merged,
        "CLOSED" => PullState::Closed,
        _ => PullState::Open {
            head: view.head_ref_oid,
        },
    })
}

/// Squash-merge, but only if the PR head is still the approved `head`.
pub(crate) fn merge(issue: &GitHubUrl, number: u64, head: &str) -> Result<(), Error> {
    gh(
        "pr merge",
        &[
            "pr",
            "merge",
            &number.to_string(),
            "-R",
            &issue.repo_slug(),
            "--squash",
            "--match-head-commit",
            head,
        ],
    )?;
    Ok(())
}

/// Comment on the pull request unless a comment already carries `marker`, so a
/// retried post never duplicates.
pub(crate) fn comment_once(
    issue: &GitHubUrl,
    number: u64,
    body: &str,
    marker: &str,
) -> Result<(), Error> {
    let path = issue.repo_path(&format!("issues/{number}/comments"));
    let bodies = gh("api", &["api", "--paginate", &path, "--jq", ".[].body"])?;
    if String::from_utf8_lossy(&bodies).contains(marker) {
        return Ok(());
    }
    let body = format!("{body}\n\n{marker}");
    gh(
        "pr comment",
        &[
            "pr",
            "comment",
            &number.to_string(),
            "-R",
            &issue.repo_slug(),
            "--body",
            &body,
        ],
    )?;
    Ok(())
}
