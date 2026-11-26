//! Shared GitHub URL parsing, data types, and `gh` command helpers.

pub(crate) mod issue;
pub(crate) mod pr;

use serde::Deserialize;
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

/// Errors returned while resolving, fetching, or writing GitHub data.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchError {
    #[error(
        "expected a full URL like https://github.com/owner/repo/issues/123 \
         or https://github.com/owner/repo/pull/123"
    )]
    InvalidUrl,
    #[error("invalid GitHub owner or repository: {0}")]
    InvalidRepo(String),
    #[error("invalid issue or pull request number: {0}")]
    InvalidNumber(String),
    #[error("{0} is not an issue")]
    NotAnIssue(String),
    #[error("{0} is not a pull request")]
    NotAPullRequest(String),
    #[error("failed to run gh (is GitHub CLI installed?): {0}")]
    GhSpawn(#[source] std::io::Error),
    #[error("gh {command} failed: {message}")]
    GhFailed {
        command: &'static str,
        message: String,
    },
    #[error("gh returned invalid JSON: {0}")]
    GhJson(#[source] serde_json::Error),
    #[error("cannot create {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct User {
    pub(crate) login: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Label {
    pub(crate) name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Issue,
    Pull,
}

/// A validated GitHub issue or pull request URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitHubUrl {
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) number: NonZeroU64,
    pub(crate) kind: Kind,
}

impl GitHubUrl {
    pub(crate) fn expect_issue(&self) -> Result<(), FetchError> {
        if self.kind == Kind::Issue {
            Ok(())
        } else {
            Err(FetchError::NotAnIssue(self.to_string()))
        }
    }

    pub(crate) fn expect_pull(&self) -> Result<(), FetchError> {
        if self.kind == Kind::Pull {
            Ok(())
        } else {
            Err(FetchError::NotAPullRequest(self.to_string()))
        }
    }

    pub(crate) fn repo_path(&self, suffix: &str) -> String {
        format!("repos/{}/{}/{}", self.owner, self.repo, suffix)
    }
}

impl FromStr for GitHubUrl {
    type Err = FetchError;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        let Some(rest) = url.strip_prefix("https://github.com/") else {
            return Err(FetchError::InvalidUrl);
        };
        let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
        if parts.len() != 4 {
            return Err(FetchError::InvalidUrl);
        }

        let kind = match parts[2] {
            "issues" => Kind::Issue,
            "pull" => Kind::Pull,
            _ => return Err(FetchError::InvalidUrl),
        };
        let (owner, repo, number) = (parts[0], parts[1], parts[3]);
        for part in [owner, repo] {
            let valid = !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
            if !valid {
                return Err(FetchError::InvalidRepo(part.to_string()));
            }
        }

        let Ok(number) = number.parse::<NonZeroU64>() else {
            return Err(FetchError::InvalidNumber(number.to_string()));
        };

        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            number,
            kind,
        })
    }
}

impl fmt::Display for GitHubUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let resource = match self.kind {
            Kind::Issue => "issues",
            Kind::Pull => "pull",
        };
        write!(
            f,
            "https://github.com/{}/{}/{resource}/{}",
            self.owner, self.repo, self.number
        )
    }
}

/// Run `gh` with a non-interactive environment and return its stdout.
pub(crate) fn gh(label: &'static str, args: &[&str]) -> Result<Vec<u8>, FetchError> {
    let output = Command::new("gh")
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .output()
        .map_err(FetchError::GhSpawn)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        return Err(FetchError::GhFailed {
            command: label,
            message: if message.is_empty() {
                format!("exit status {}", output.status)
            } else {
                message.to_string()
            },
        });
    }

    Ok(output.stdout)
}

pub(crate) fn author_login(user: Option<&User>) -> &str {
    user.map_or("ghost", |user| user.login.as_str())
}

pub(crate) fn label_names(labels: &[Label]) -> String {
    labels
        .iter()
        .map(|label| label.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn push_verbatim(out: &mut String, text: &str) {
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
}

/// Create `dir` if needed and write `markdown` to `<dir>/<file_name>`.
pub(crate) fn write_markdown(
    dir: &Path,
    file_name: &str,
    markdown: &str,
) -> Result<(), FetchError> {
    if let Err(source) = fs::create_dir_all(dir) {
        return Err(FetchError::CreateDir {
            path: dir.to_path_buf(),
            source,
        });
    }

    let path = dir.join(file_name);
    if let Err(source) = fs::write(&path, markdown) {
        return Err(FetchError::Write { path, source });
    }

    // Status output; a closed pipe must not turn a successful write into a panic.
    let _ = writeln!(std::io::stdout(), "wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_issue_and_pull_urls() {
        let issue: GitHubUrl = "https://github.com/acme/widgets/issues/42".parse().unwrap();
        assert_eq!(issue.kind, Kind::Issue);
        assert_eq!(issue.owner, "acme");
        assert_eq!(issue.repo, "widgets");
        assert_eq!(issue.number.get(), 42);
        assert_eq!(
            issue.to_string(),
            "https://github.com/acme/widgets/issues/42"
        );

        let pull: GitHubUrl = "https://github.com/acme/widgets/pull/42".parse().unwrap();
        assert_eq!(pull.kind, Kind::Pull);
        assert_eq!(pull.to_string(), "https://github.com/acme/widgets/pull/42");

        let trailing: GitHubUrl = "https://github.com/acme/widgets/issues/42/"
            .parse()
            .unwrap();
        assert_eq!(trailing.number.get(), 42);
    }

    #[test]
    fn rejects_bad_urls() {
        for url in [
            "https://github.com/acme/widgets/issues/42/extra",
            "https://github.com/acme/widgets/issues/nope",
            "https://github.com/acme/widgets/issues/0",
            "https://github.com/acme/widgets/issues/42?x=1",
            "https://github.com/acme/widgets/issues/42#x",
            "https://github.com/acme/widgets/pulls/42",
            "acme/widgets#42",
            "http://github.com/acme/widgets/issues/42",
            "https://github.com/ac me/widgets/issues/42",
            "https://github.com//widgets/issues/42",
            "https://github.com/acme//issues/42",
        ] {
            assert!(
                url.parse::<GitHubUrl>().is_err(),
                "{url} should be rejected"
            );
        }
    }

    #[test]
    fn kind_checks_reject_the_wrong_resource() {
        let issue: GitHubUrl = "https://github.com/acme/widgets/issues/1".parse().unwrap();
        let pull: GitHubUrl = "https://github.com/acme/widgets/pull/1".parse().unwrap();

        assert!(issue.expect_issue().is_ok());
        assert!(pull.expect_pull().is_ok());
        assert_eq!(
            issue.expect_pull().unwrap_err().to_string(),
            "https://github.com/acme/widgets/issues/1 is not a pull request"
        );
        assert_eq!(
            pull.expect_issue().unwrap_err().to_string(),
            "https://github.com/acme/widgets/pull/1 is not an issue"
        );
        assert_eq!(issue.repo_path("pulls/1"), "repos/acme/widgets/pulls/1");
    }
}
