//! Shared GitHub URL parsing, data types, and `gh` command helpers.

pub(crate) mod issue;
pub(crate) mod pr;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;

const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// Errors returned while resolving, fetching, or writing GitHub data.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
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
    Gh(#[source] crate::process::Error),
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
    #[error("cannot read {path}: {source}")]
    ReadDir {
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
    pub(crate) fn expect_issue(&self) -> Result<(), Error> {
        if self.kind == Kind::Issue {
            Ok(())
        } else {
            Err(Error::NotAnIssue(self.to_string()))
        }
    }

    pub(crate) fn expect_pull(&self) -> Result<(), Error> {
        if self.kind == Kind::Pull {
            Ok(())
        } else {
            Err(Error::NotAPullRequest(self.to_string()))
        }
    }

    /// `owner/repo`, as `gh -R` expects.
    pub(crate) fn repo_slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    pub(crate) fn repo_path(&self, suffix: &str) -> String {
        format!("repos/{}/{}/{}", self.owner, self.repo, suffix)
    }
}

impl FromStr for GitHubUrl {
    type Err = Error;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        let Some(rest) = url.strip_prefix("https://github.com/") else {
            return Err(Error::InvalidUrl);
        };
        let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
        let [owner, repo, resource, number] = parts.as_slice() else {
            return Err(Error::InvalidUrl);
        };

        let kind = match *resource {
            "issues" => Kind::Issue,
            "pull" => Kind::Pull,
            _ => return Err(Error::InvalidUrl),
        };
        for part in [owner, repo] {
            if !is_valid_name(part) {
                return Err(Error::InvalidRepo(part.to_string()));
            }
        }

        let Ok(number) = number.parse::<NonZeroU64>() else {
            return Err(Error::InvalidNumber(number.to_string()));
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

/// A GitHub owner or repository name. Also safe as a path component: never `.` or `..`.
pub(crate) fn is_valid_name(part: &str) -> bool {
    !part.is_empty()
        && !matches!(part, "." | "..")
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Run `gh` with a non-interactive environment and return its stdout.
pub(crate) fn gh(label: &'static str, args: &[&str]) -> Result<Vec<u8>, Error> {
    let mut command = Command::new("gh");
    command
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1");
    let output = crate::process::run(&mut command, GH_TIMEOUT).map_err(Error::Gh)?;

    if !output.status.success() {
        return Err(Error::GhFailed {
            command: label,
            message: output.failure_message(),
        });
    }

    Ok(output.stdout.into_bytes())
}

/// Run `gh` and parse its stdout as JSON.
pub(crate) fn gh_json<T: DeserializeOwned>(label: &'static str, args: &[&str]) -> Result<T, Error> {
    serde_json::from_slice(&gh(label, args)?).map_err(Error::GhJson)
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
pub(crate) fn write_markdown(dir: &Path, file_name: &str, markdown: &str) -> Result<(), Error> {
    if let Err(source) = fs::create_dir_all(dir) {
        return Err(Error::CreateDir {
            path: dir.to_path_buf(),
            source,
        });
    }

    let path = dir.join(file_name);
    if let Err(source) = fs::write(&path, markdown) {
        return Err(Error::Write { path, source });
    }

    // Status output; a closed pipe must not turn a successful write into a panic.
    let _ = writeln!(std::io::stdout(), "wrote {}", path.display());
    Ok(())
}
