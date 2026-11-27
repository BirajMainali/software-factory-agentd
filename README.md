# agentd

A router for autonomous software work. It takes a GitHub issue, hands it to one AI
agent at a time in a fixed order, and drives it to an open pull request — then keeps
routing new PR feedback back to the developer.

agentd does not decide what the work is. It only starts each agent, waits for
opencode to report the run ended, and moves to the next step. Agents own the code,
git, and GitHub. agentd never reads what they write — it hands each agent the path
to read and the path to write.

## Requirements

- Rust 1.89+
- [GitHub CLI](https://cli.github.com), logged in: `gh auth login`
- [opencode](https://opencode.ai) app running (agentd talks to its local service)
- `curl` and `git` on `PATH`

## Install

Clone the repo, then run its installer:

```sh
git clone https://github.com/BirajMainali/agentd && cd agentd
./install.sh
```

That runs `cargo install --path .` and puts `agentd` in `~/.cargo/bin` (no sudo;
rustup keeps it on `PATH`). Check with `agentd help`. To update, `git pull` and run
it again.

For all users of the machine:

```sh
cargo build --release
sudo install -m 755 target/release/agentd /usr/local/bin/agentd
```

Check with `agentd help`. Update by running the same command again.

## Configure

Both variables are required; no defaults. `PROJECT_PATH` is the project the agents
work in: the main session and every agent session run there. `FACTORY_HOME` holds
task state only.

| Variable | What | Example |
|---|---|---|
| `PROJECT_PATH` | Project the agents read, edit, build, and commit in | `~/src/proj-a` |
| `FACTORY_HOME` | Where agentd keeps task state | `~/agentd` |

```sh
export PROJECT_PATH="$HOME/src/proj-a"
export FACTORY_HOME="$HOME/agentd"
```

Artifacts are not configured here either. `agentd work --path <dir>` names the exact
folder per issue, so several issues of one project share `PROJECT_PATH` without
sharing artifacts.

### opencode access

You don't pass a key. The opencode app writes its local service address and a
generated access key to:

```
$XDG_STATE_HOME/opencode/service.json      (default ~/.local/state/opencode/service.json)
```

agentd reads `url` and `password` from that file when a worker starts. The key goes
to `curl` through stdin, never on the command line, in logs, or in events. If
opencode keeps its state elsewhere, point `XDG_STATE_HOME` at it.

### Agents

agentd starts these opencode agents: `solution-architect`, `developer`,
`reviewer-security`, `reviewer-code`, `reviewer-uiux`. They are global: install them
once per machine, and every project uses them. Dummy versions for testing are in
`test/agents/`; opencode only discovers agents under `~/.config/opencode/agents/`,
so link them there:

```sh
mkdir -p ~/.config/opencode/agents
ln -s "$PWD"/test/agents/*.md ~/.config/opencode/agents/
```

## Use

```sh
agentd work --issue <issue-url> --path <dir>               # start one issue in the background
agentd run <owner>/<repo>...                               # route open `factory` issues
agentd dashboard [127.0.0.1:7878]                          # live status page
agentd fetch-issue <issue-url> <dir>                       # write <dir>/issue.md
agentd fetch-pr <pr-url> <dir>                             # write <dir>/pr-feedback/<n>-pr-feedback.md
agentd spawn --prompt "..." [--session <id>] [--agent <id>] [--title <text>]
```

`work` starts a worker in the background and returns at once with
`{"issue":…,"pid":…}`, so a script can submit thousands of issues in a loop. The
sessions run in `PROJECT_PATH`. `--path` is a work root: agentd gives each issue its
own `<root>/#<n>` folder, so one project can hold many issues without collisions.

```sh
agentd work --issue "https://github.com/o/r/issues/12" --path "$HOME/work/proj-a"
# artifacts: $HOME/work/proj-a/#12/{issue.md,plan/,review/,development/,pr-feedback/}
```

It resumes from the task log, so a crashed or restarted worker picks up where it left
off, and a failure is recorded in `events.jsonl` (`worker_failed`) and shown on the
dashboard. Add `--foreground` to run the worker in the calling process instead of
detaching it; the scheduler does.

```sh
for n in 1 2 3; do
  agentd work --issue "https://github.com/o/r/issues/$n" --path "$HOME/work/r"
done
```

`spawn` is a utility to prompt an opencode session directly.

## How a task runs

Steps run in this order. `agentd run` picks open issues labelled `factory`; `agentd
work` routes any issue into `<--path>/#<n>`. Sessions run in `PROJECT_PATH`; paths
below are relative to that per-issue folder.

| Step | Agent | Reads | Writes |
|---|---|---|---|
| plan | solution-architect | `issue.md` | `plan/<n>-plan.md` |
| develop | developer | `plan/<n>-plan.md` | `development/<n>-development.md` |
| review ×3 (security → code → uiux) | reviewer-* | — | `review/<n>-<kind>.md` |
| develop ×3 | developer (same session) | `review/<n>-<kind>.md` | `development/<n>-development.md` |
| open PR | developer (same session) | — | `development/<n>-development.md` |
| PR feedback | developer (same session) | `pr-feedback/<n>-pr-feedback.md` | `development/<n>-development.md` |

- Every file is numbered and never overwritten: `plan` and each review kind have
  their own counter, `development` has one across all developer runs, and PR
  feedback rounds come from the watch.
- The developer gets a new session for the plan and keeps it for every later step,
  so the same agent implements, applies reviews, opens the PR, and answers feedback.
- To let the runtime find the PR, the open-PR prompt asks the developer to reference
  the issue (`Fixes #<n>`).
- A failed run fails the worker; it is retried with doubling backoff, and after 3
  failures the task is left for a human (`stuck` on the dashboard).
- `agentd run` runs at most 4 workers, one per task; the worker holds the task lock,
  so two workers never run the same issue.

### Task state on disk

`FACTORY_HOME` holds only the log, lock, and the default folder the scheduler gives a
task:

```
$FACTORY_HOME/tasks/<owner>/<repo>/<n>/
  events.jsonl        append-only task log; all state is replayed from it
  lock                held while one worker runs the task
  workspace/          path `agentd run` passes to `work` (a manual `work` names its own)
```

Wherever the root is, each issue gets its own folder: `<--path>/#<n>/`, containing:

```
issue.md
plan/<n>-plan.md
review/<n>-<kind>.md
development/<n>-development.md
pr-feedback/<n>-pr-feedback.md
```

To move the state, change `FACTORY_HOME`. Artifacts move with the `--path` you pass.

### PR feedback

The runtime watches each finished task's PR through the issue timeline. When the PR
has a review or comment the task has not seen, it records the round under the task
lock and runs the developer again on the next `pr-feedback` file. Closed and merged
PRs are ignored; edited comment bodies are not noticed.

### Dashboard

`agentd dashboard` serves a read-only page at `127.0.0.1:7878` (pass another address
to change it) and streams each task's state over server-sent events. Each row lists
the task's own artifacts — `issue.md`, plans, reviews, development summaries, PR
feedback — and every name links to that file's text at `/doc/…`.

The doc endpoint serves only the closed set of artifact names above, from the work
path recorded in the task log. Other files in that folder are listed nowhere and
return 404, symlinks may not escape the folder, and a file over 2 MiB is refused.
Nothing else from the repository is read.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Lints are set in `Cargo.toml` (unsafe forbidden, clippy correctness denied, pedantic
warned), so the clippy command gates the whole policy. There is no CI; run these
before pushing.
