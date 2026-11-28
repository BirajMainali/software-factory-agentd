# agentd

Agentd drives one GitHub issue from submission to a merged pull request. It routes the
issue through a fixed sequence of AI agents, records each step, watches the resulting
PR, and sends new review feedback back to the developer.

It is a router, not a controller. Agentd decides *which* agent runs next and *where* its
input and output files live. It never edits code, touches git, or talks to GitHub beyond
reading the issue and the PR. The agents do that work themselves.

## How it works

```
work --issue <url> --path <dir>
        │
        ▼
plan ─► develop ─► read the PR labels
                      ├─ factory-approved ──► squash-merge ──► exit
                      ├─ factory-ready ─────► PR review ──► watch
                      └─ neither ─█
                                  ▼
             security ─► code ─► ui/ux ─► develop ──────────────────────┐
                        ▲                                            │
                        └────────────────────────────────────────────┘

watch ──► new feedback ──► remove factory-ready ──► developer (resumes)
```

- **One task, one supervisor.** `work` starts a detached supervisor and returns
  immediately. The supervisor holds the task lock for the whole route and watch, so two
  runs for the same issue cannot overlap. `--foreground` keeps it in the current
  process.
- **Sessions.** Each task gets one main session titled `owner/repo#n <title>`. Every
  step runs in a sub-agent session under it. The router always decides the next step;
  the main session is stopped so opencode cannot advance the workflow on its own.
- **Artifacts.** Each step receives the absolute paths to read and the absolute path to
  write. Files are numbered per run and never overwritten, so a resumed task can see
  what already happened. A retry of the same attempt reuses its number instead of
  numbering another file.
- **State.** Every step appends to `$HOME_REPO_PATH/tasks/<owner>/<repo>/<n>/events.jsonl`.
  All state is replayed from that log, including after a crash. A run that was
  interrupted picks up its own unfinished step instead of starting over. A step
  completion records the step that ran next, so replay follows the route as it was,
  not as it is today.
- **PR labels.** The developer adds `factory-ready` when the work is ready;
  the route stops and the supervisor watches for feedback. A reviewer adds
  `factory-approved` and agentd squash-merges it once GitHub reports it mergeable. If
  it is not, both labels come off and the developer gets a merge-conflict note in the
  next round. New feedback removes the ready label before the developer is woken.
- **Failure handling.** A failed agent run is retried with backoff. After three
  consecutive failures the task is left for a human; the supervisor stays alive and
  keeps watching the PR.
- **Completion.** The task is done when its PR merges or closes, not when code is
  pushed. Until then it holds its lock and keeps watching.

## Requirements

- Rust 1.89 or newer (to build)
- [GitHub CLI](https://cli.github.com), authenticated with `gh auth login`
- The [opencode](https://opencode.ai) app running locally
- `git` on `PATH`

## Install

```sh
git clone https://github.com/BirajMainali/software-factory-agentd && cd software-factory-agentd
./install.sh
```

`install.sh` runs `cargo install --path . --locked --force`, putting `agentd` in
`~/.cargo/bin`. Confirm with `agentd help`. To update, pull and run it again.

With `HOME_REPO_PATH` set, it also installs and starts the dashboard as the systemd
user service `agentd-dashboard.service`, enabled at boot, and enables lingering so the
service runs without a login session. Without `HOME_REPO_PATH` it skips that step and
says so; rerun the script once the variable is set.

To install for all users:

```sh
cargo build --release
sudo install -m 755 target/release/agentd /usr/local/bin/agentd
```

## Configure

One variable is required; there is no default, and the directory must exist.

```sh
export HOME_REPO_PATH="$HOME/src/home-repo"
```

Agent sessions start in that directory, so opencode resolves its agents and project
instructions from there, and agentd keeps task state under
`$HOME_REPO_PATH/tasks/`.

`work --path <dir>` names where that issue's artifacts go. Agentd creates
`<dir>/#<n>/` for issue `n`, so a single `--path` can hold many issues.

Agentd reads the local opencode address and key from
`~/.local/state/opencode/service.json`. You never pass them; the key is sent over stdin
and never appears in argv or logs.

## Commands

```sh
agentd work --issue <url> --path <dir>       # own one issue until its PR closes
agentd dashboard [<addr>]                    # live view, default 127.0.0.1:7878
agentd fetch-issue <url> <dir>               # write <dir>/issue.md
agentd fetch-pr <pr-url> <dir>               # write <dir>/pr-feedback/<n>-pr-feedback.md
agentd spawn --prompt <text>                 # prompt an opencode session directly
```

`work` prints `{"issue":…,"pid":…}` and exits; the supervisor continues in the
background. Add `--foreground` to run it in the calling process.

```sh
agentd work --issue "https://github.com/org/app/issues/12" --path ~/work/app/issues
```

## Artifacts

For issue 12, `<--path>/#12/` contains:

```
issue.md                         the issue text, written at start
plan/<n>-plan.md                 the plan
development/<n>-development.md   the developer's summary, one per developer run
pr-feedback/<n>-pr-feedback.md   PR reviews/comments, one per feedback round
```

`<n>` counts writes of that file, so repeated runs never overwrite earlier ones.

## Steps

| Step | Agent | Session | Reads | Writes |
|---|---|---|---|---|
| plan | `planner` | new | `issue.md` | `plan/<n>-plan.md` |
| develop | `developer` | new | the plan, then the latest reviews | `development/<n>-development.md` |
| security review | `reviewer-security` | new | the pushed branch | `review/<n>-security.md` |
| code review | `reviewer-code` | new | the pushed branch | `review/<n>-code.md` |
| ui/ux review | `reviewer-uiux` | new | the pushed branch | `review/<n>-uiux.md` |
| PR feedback | `developer` | resumes | `pr-feedback/<n>-pr-feedback.md` | `development/<n>-development.md` |

After every developer pass the labels decide: `factory-approved` merges,
`factory-ready` waits for feedback, anything else loops through the three reviews and
develops again. When new feedback arrives, the watch removes `factory-ready` and
resumes the same developer session for that round.

Define the five agents wherever opencode resolves its agents. Dummy versions for local
testing are in this repo's `test/agents/`.

## Dashboard

`agentd dashboard` serves a read-only view over HTTP and SSE: every task with its issue
link, PR link, current step, and numbered artifacts. Clicking a task opens a drawer
listing its documents by type, numbered by run; each document is rendered from markdown.
It serves only those artifact files and nothing else from the repository.

`install.sh` runs it as `agentd-dashboard.service` at `http://127.0.0.1:7878`, restarting
on failure and starting at boot. Manage it like any user service:

```sh
systemctl --user status agentd-dashboard
systemctl --user restart agentd-dashboard
journalctl --user -u agentd-dashboard -f
```

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Lints are configured in `Cargo.toml`; the clippy command is the gate. There is no CI,
so run all three before pushing.
