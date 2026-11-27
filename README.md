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

For your user (no sudo; rustup puts `~/.cargo/bin` on `PATH`):

```sh
cargo install --path .
```

For all users:

```sh
cargo build --release
sudo install -m 755 target/release/agentd /usr/local/bin/agentd
```

Check with `agentd help`. Update by running the same command again.

## Configure

Both variables are required; there are no defaults.

| Variable | What | Example |
|---|---|---|
| `FACTORY_HOME` | Where agentd keeps task state and workspaces | `~/agentd` |
| `FACTORY_DIR` | Folder in each workspace where agentd and agents exchange files. Relative path. | `factory` |

```sh
export FACTORY_HOME="$HOME/agentd"
export FACTORY_DIR=factory
```

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
`reviewer-security`, `reviewer-code`, `reviewer-uiux`. Dummy versions for testing
are in `test/agents/`; opencode only discovers agents under
`~/.config/opencode/agents/`, so link them there:

```sh
mkdir -p ~/.config/opencode/agents
ln -s "$PWD"/test/agents/*.md ~/.config/opencode/agents/
```

## Use

```sh
agentd work https://github.com/<owner>/<repo>/issues/<n>   # route one issue (resumes)
agentd run <owner>/<repo>...                               # route open `factory` issues
agentd dashboard [127.0.0.1:7878]                          # live status page
agentd fetch-issue <issue-url> <dir>                       # write <dir>/issue.md
agentd fetch-pr <pr-url> <dir>                             # write <dir>/pr-feedback/<n>-pr-feedback.md
agentd spawn --prompt "..." [--session <id>] [--agent <id>] [--title <text>]
```

`work` prints JSON lines and resumes from the task log, so a crashed or stopped
worker picks up where it left off. `spawn` is a utility to prompt an opencode session
directly.

## How a task runs

Steps run in this order. `agentd run` picks open issues labelled `factory`; `agentd
work` routes any issue.

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

```
$FACTORY_HOME/tasks/<owner>/<repo>/<n>/
  events.jsonl        append-only task log; all state is replayed from it
  lock                held while one worker runs the task
  workspace/          agents' working directory; they clone and branch here
    $FACTORY_DIR/#<n>/
      issue.md
      plan/<n>-plan.md
      review/<n>-<kind>.md
      development/<n>-development.md
      pr-feedback/<n>-pr-feedback.md
```

To move everything, change `FACTORY_HOME`.

### PR feedback

The runtime watches each finished task's PR through the issue timeline. When the PR
has a review or comment the task has not seen, it records the round under the task
lock and runs the developer again on the next `pr-feedback` file. Closed and merged
PRs are ignored; edited comment bodies are not noticed.

### Dashboard

`agentd dashboard` serves a read-only page at `127.0.0.1:7878` (pass another address
to change it) and streams each task's state over server-sent events. It shows
status only: no agent output, feedback text, or repository content.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Lints are set in `Cargo.toml` (unsafe forbidden, clippy correctness denied, pedantic
warned), so the clippy command gates the whole policy. There is no CI; run these
before pushing.
