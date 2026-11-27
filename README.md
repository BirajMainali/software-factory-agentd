# agentd

Routes a GitHub issue through AI agents in a fixed order:
plan → develop → security / code / ui-ux review (each followed by the developer) →
open PR → watch the PR for new feedback (each round goes back to the developer).
Agents do the work; agentd only starts them, waits for opencode to report each run
ended, and moves to the next step.

## Requirements

- Rust 1.89+
- [GitHub CLI](https://cli.github.com) logged in: `gh auth login`
- [opencode](https://opencode.ai) app running (agentd talks to its local service)
- `curl`, `git`

## Install

For your user (no sudo; `~/.cargo/bin` must be on `PATH`, which rustup sets up):

```sh
cargo install --path .
```

For all users of the machine:

```sh
cargo build --release
sudo install -m 755 target/release/agentd /usr/local/bin/agentd
```

Check: `agentd help`. To update, run the same command again.

## Configure

Both are required; there are no defaults.

| Variable | What | Example |
|---|---|---|
| `FACTORY_HOME` | Where agentd keeps task state and workspaces | `~/agentd` |
| `FACTORY_DIR` | Folder inside each workspace where agentd and agents exchange files (`issue.md`, `plan.md`, reviews). Relative path. | `factory` |

```sh
export FACTORY_HOME="$HOME/agentd"
export FACTORY_DIR=factory
```

### Working directory

Agents run in one directory per issue, created under `FACTORY_HOME`:

```
$FACTORY_HOME/tasks/<owner>/<repo>/<number>/
  events.jsonl       task log (agentd's state)
  lock               held while a worker runs the task
  workspace/         agents' working directory; they clone and branch here
    $FACTORY_DIR/    #<number>/{issue.md,plan/,review/,development/,pr-feedback/}
```

To move everything, change `FACTORY_HOME`.

### opencode password

You don't pass it. The opencode app writes its local service address and a generated
access key to:

```
$XDG_STATE_HOME/opencode/service.json      (default ~/.local/state/opencode/service.json)
```

agentd reads `url` and `password` from that file each time a worker starts, and sends
the key to `curl` through stdin, never on the command line or in logs. If opencode keeps
its state elsewhere, set `XDG_STATE_HOME` so the file is at
`$XDG_STATE_HOME/opencode/service.json`.

### Agents

agentd starts these opencode agents: `solution-architect`, `developer`,
`reviewer-security`, `reviewer-code`, `reviewer-uiux`. Examples are in `test/agents/`.
opencode finds agents in `~/.config/opencode/agents/`:

```sh
mkdir -p ~/.config/opencode/agents
ln -s "$PWD"/test/agents/*.md ~/.config/opencode/agents/
```

## Use

```sh
agentd work https://github.com/<owner>/<repo>/issues/<n>   # route one issue (resumes)
agentd run <owner>/<repo>...                               # route all open issues labelled `factory`
agentd dashboard [127.0.0.1:7878]                          # live status page
agentd fetch-issue <issue-url> <dir>                       # write <dir>/issue.md
agentd fetch-pr <pr-url> <dir>                             # write <dir>/pr-feedback/<n>-pr-feedback.md
agentd spawn --prompt "..." [--session <id>] [--agent <id>] [--title <text>]
```

`work` prints JSON lines: first the progress (completed steps, next step), then each
event. A crashed or stopped worker resumes where it left off.
