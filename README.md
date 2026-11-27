# agentd

Agentd owns one GitHub issue from submit to merged pull request. You tell it what to
work on; it routes the issue through AI agents in a fixed order, watches the PR, and
sends new review feedback back to the developer.

Agentd does not pick issues and does not read what agents write. It hands each agent
the path to read and the path to write. Agents own the code, git, and GitHub.

## Workflow

```
                    agentd work --issue <url> --path <dir>
                                     │
                                     ▼
   ┌─────────────────────────────────────────────────────────────┐
   │ plan                                                        │
   │ develop        implement, commit, push, open a draft PR     │
   │ security       review the pushed branch                     │
   │ code           review the pushed branch                     │
   │ ui/ux          review the pushed branch                     │
   │ developer      address all three reviews, push              │
   │ developer      mark the PR ready                            │
   └─────────────────────────────────────────────────────────────┘
                                     │
                                     ▼
   ┌─────────────────────────────────────────────────────────────┐
   │ watch the PR                                                │
   │   new review/comment ──► developer fixes it ──► watch again │
   │   merged or closed   ──► exit                                │
   └─────────────────────────────────────────────────────────────┘
```

One task is one supervisor process. It holds the task lock for the whole route and
watch, so two `work` calls for the same issue never overlap. A failed agent run is
retried with backoff; after three failures the task is left for a human.

## Install

```sh
git clone https://github.com/BirajMainali/agentd && cd agentd
./install.sh
```

That runs `cargo install --path .` and puts `agentd` in `~/.cargo/bin`. Check with
`agentd help`. To update, `git pull` and run it again.

For all users:

```sh
cargo build --release
sudo install -m 755 target/release/agentd /usr/local/bin/agentd
```

Requirements: Rust 1.89+, [GitHub CLI](https://cli.github.com) logged in
(`gh auth login`), the [opencode](https://opencode.ai) app running, and `git` on
`PATH`.

## Configure

Both variables are required; there are no defaults.

```sh
export PROJECT_PATH="$HOME/src/proj-a"   # the repo agents read, edit, and commit in
export FACTORY_HOME="$HOME/agentd"       # where task state lives
```

`work --path <dir>` names where that issue's artifacts go. Agentd creates
`<dir>/#<n>/` for issue `n`, so one `--path` can hold many issues.

Agentd reads the local opencode address and key from
`~/.local/state/opencode/service.json`; you never pass them. Install the agents it
starts (`planner`, `developer`, `reviewer-security`, `reviewer-code`, `reviewer-uiux`)
under `~/.config/opencode/agents/`. Dummy versions for testing are in `test/agents/`.

## Use

```sh
agentd work --issue "https://github.com/org/app/issues/12" --path ~/work/app/issues
agentd dashboard                         # live view at 127.0.0.1:7878
agentd fetch-issue <issue-url> <dir>     # write <dir>/issue.md
agentd fetch-pr <pr-url> <dir>           # write <dir>/pr-feedback/<n>-pr-feedback.md
agentd spawn --prompt "..."              # prompt an opencode session directly
```

`work` is fire and forget: it starts a background supervisor and returns at once with
`{"issue":…,"pid":…}`. The supervisor routes the issue, then keeps watching its PR
until it merges or closes. `--foreground` runs it in the calling process instead.

Artifacts for issue 12:

```
<--path>/#12/
  issue.md
  plan/<n>-plan.md
  review/<n>-<kind>.md
  development/<n>-development.md
  pr-feedback/<n>-pr-feedback.md
```

`FACTORY_HOME` holds only the task log and lock:

```
$FACTORY_HOME/tasks/<owner>/<repo>/<n>/
  events.jsonl   append-only log; all state is replayed from it
  lock           held by the one supervisor
```

## Steps

| Step | Agent | Reads | Writes |
|---|---|---|---|
| plan | planner | `issue.md` | `plan/<n>-plan.md` |
| develop | developer | the plan | `development/<n>-development.md` |
| security review | reviewer-security | the pushed branch | `review/<n>-security.md` |
| code review | reviewer-code | the pushed branch | `review/<n>-code.md` |
| ui/ux review | reviewer-uiux | the pushed branch | `review/<n>-uiux.md` |
| address reviews | developer | the three reviews | `development/<n>-development.md` |
| PR ready | developer | — | `development/<n>-development.md` |
| PR feedback | developer | `pr-feedback/<n>-pr-feedback.md` | `development/<n>-development.md` |

Each file is numbered and never overwritten. The developer keeps one session from the
plan onward, so it implements, applies the reviews, and answers feedback.

The dashboard lists every task with its issue link, PR link, current step, and its
artifacts. It only serves those artifact files, nothing else from the repository.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Lints are set in `Cargo.toml`; the clippy command gates the policy. There is no CI, so
run these before pushing.
