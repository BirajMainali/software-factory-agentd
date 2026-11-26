//! Product clones and per-task git worktrees under the factory home.

use crate::github::GitHubUrl;
use crate::{home, process};
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const CLONE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const GIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Factory files inside a worktree (issue, plan); git ignores them so they never reach the PR.
pub(crate) const FACTORY_DIR: &str = ".factory";

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Home(#[from] home::Error),
    #[error(transparent)]
    Process(#[from] process::Error),
    #[error("{command} failed: {message}")]
    Command { command: String, message: String },
    #[error("cannot prepare {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// One task's isolated checkout on its own branch.
pub(crate) struct Worktree {
    pub(crate) path: PathBuf,
    pub(crate) branch: String,
}

impl Worktree {
    /// Clone or fetch the product repo, then create or reuse the task's worktree.
    /// Safe to repeat after a crash at any step.
    pub(crate) fn prepare(home: &Path, issue: &GitHubUrl) -> Result<Self, Error> {
        let repo_dir = home.join("repos").join(&issue.owner);
        let clone = repo_dir.join(&issue.repo);
        let path = home
            .join("worktrees")
            .join(&issue.owner)
            .join(&issue.repo)
            .join(issue.number.to_string());
        let branch = format!("factory/issue-{}", issue.number);

        // Workers of one repo share its clone; git refuses concurrent ref updates.
        let _clone_lock = home::lock(&repo_dir.join(format!("{}.lock", issue.repo)))?;
        if clone.join(".git").exists() {
            git(&clone, &["fetch", "--quiet", "--prune", "origin"])?;
        } else {
            clone_repo(issue, &clone)?;
        }
        ignore_factory_dir(&clone)?;

        if !path.join(".git").exists() {
            create_worktree(&clone, &path, &branch)?;
        }
        Ok(Self { path, branch })
    }

    pub(crate) fn head(&self) -> Result<String, Error> {
        git(&self.path, &["rev-parse", "HEAD"])
    }

    pub(crate) fn is_clean(&self) -> Result<bool, Error> {
        Ok(git(&self.path, &["status", "--porcelain"])?.is_empty())
    }

    /// Commit whatever the agent left uncommitted. No-op when clean.
    pub(crate) fn commit_all(&self, message: &str) -> Result<(), Error> {
        if self.is_clean()? {
            return Ok(());
        }
        git(&self.path, &["add", "--all"])?;
        git(&self.path, &["commit", "--quiet", "--message", message])?;
        Ok(())
    }

    /// Fast-forward push of the task branch; never forces.
    pub(crate) fn push(&self) -> Result<(), Error> {
        let refspec = format!("HEAD:refs/heads/{}", self.branch);
        git(&self.path, &["push", "--quiet", "origin", &refspec])?;
        Ok(())
    }

    pub(crate) fn factory_file(&self, name: &str) -> PathBuf {
        self.path.join(FACTORY_DIR).join(name)
    }
}

fn clone_repo(issue: &GitHubUrl, clone: &Path) -> Result<(), Error> {
    // Clone beside the target and rename, so a crash never leaves a half clone in place.
    let partial = clone.with_extension("partial");
    if partial.exists() {
        fs::remove_dir_all(&partial).map_err(|source| io_error(&partial, source))?;
    }
    let repo = format!("{}/{}", issue.owner, issue.repo);
    let mut command = Command::new("gh");
    command
        .args(["repo", "clone", &repo])
        .arg(&partial)
        .args(["--", "--quiet"])
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    check(&mut command, "gh repo clone", CLONE_TIMEOUT)?;
    fs::rename(&partial, clone).map_err(|source| io_error(clone, source))
}

fn create_worktree(clone: &Path, path: &Path, branch: &str) -> Result<(), Error> {
    // A crash inside `git worktree add` leaves a registered but broken worktree.
    git(clone, &["worktree", "prune"])?;
    if path.exists() {
        fs::remove_dir_all(path).map_err(|source| io_error(path, source))?;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    let path = path.to_string_lossy();
    let branch_ref = format!("refs/heads/{branch}");
    if git(clone, &["rev-parse", "--verify", "--quiet", &branch_ref]).is_ok() {
        git(clone, &["worktree", "add", "--quiet", &path, branch])?;
    } else {
        let base = git(
            clone,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        )?;
        git(
            clone,
            &["worktree", "add", "--quiet", "-b", branch, &path, &base],
        )?;
    }
    Ok(())
}

fn ignore_factory_dir(clone: &Path) -> Result<(), Error> {
    let exclude = clone.join(".git/info/exclude");
    let entry = format!("/{FACTORY_DIR}/");
    let current = match fs::read_to_string(&exclude) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(source) => return Err(io_error(&exclude, source)),
    };
    if current.lines().any(|line| line == entry) {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude)
        .map_err(|source| io_error(&exclude, source))?;
    let separator = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    writeln!(file, "{separator}{entry}").map_err(|source| io_error(&exclude, source))
}

fn git(dir: &Path, args: &[&str]) -> Result<String, Error> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    let label = format!("git {}", args.first().copied().unwrap_or_default());
    check(&mut command, &label, GIT_TIMEOUT)
}

fn check(command: &mut Command, label: &str, timeout: Duration) -> Result<String, Error> {
    let output = process::run(command, timeout)?;
    if !output.status.success() {
        return Err(Error::Command {
            command: label.to_string(),
            message: output.failure_message(),
        });
    }
    Ok(output.stdout.trim().to_string())
}

fn io_error(path: &Path, source: io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}
