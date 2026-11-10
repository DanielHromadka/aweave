<div align="center">

<img src="website/img/icon.svg" alt="aWeave icon" width="104" height="104">

# aWeave

**Many agents. One repository. No pile-ups.**

Version control coordination for teams of AI agents working on one git repository at the same time: an isolated workspace per agent, intents that surface overlaps *before* anyone writes conflicting code, an integration queue that keeps trunk green, provenance on every commit, and an MCP server so agents use all of it as tools.

[![Latest release](https://img.shields.io/github/v/release/DanielHromadka/aweave?label=latest&color=5b3df5)](https://github.com/DanielHromadka/aweave/releases/latest)
[![CI](https://github.com/DanielHromadka/aweave/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/DanielHromadka/aweave/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/license-GPL--3.0-blue)](LICENSE)
<br>
[![Rust](https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![MCP](https://img.shields.io/badge/MCP-server-5b3df5)](#use-it-from-claude-code-codex-or-cursor)
[![git](https://img.shields.io/badge/works%20on-git-F05032?logo=git&logoColor=white)](#how-it-works)

[**Install**](#install) · [Website](https://aweave.pages.dev/) · [Design](docs/design.md) · [Report a bug](https://github.com/DanielHromadka/aweave/issues/new) · [Čeština](README.cs.md)

</div>

## Why

Git was built for people who work on one thing at a time and merge now and then. Ten agents on one repository break those assumptions:

- **One working copy per checkout.** Agents overwrite each other's files or fight over `index.lock`; hand-made worktrees leak.
- **Conflicts show up last.** Two agents spend an hour on the same function and find out at merge time.
- **Nobody knows who is doing what.** There is no place to say "I'm about to change `src/auth`".
- **Trunk breaks.** Whoever merges last merges onto something they never tested.
- **"Why" is lost.** A commit records an author, not the agent, task or intent behind it.

aWeave adds the missing coordination layer. Everything still lands in git, so GitHub, CI and your editor keep working.

## How it works

```console
$ aweave init --verify "cargo test"
$ aweave --agent claude-1 ws new auth-timeout          # isolated worktree on branch aweave/auth-timeout
$ aweave --ws auth-timeout intent add "Fix session refresh timeout" --scope src/auth/session.rs
intent #1 declared: src/auth/session.rs
no overlaps

$ aweave --ws login-ratelimit intent add "Rate-limit login attempts" --scope src/auth
intent #2 declared: src/auth
overlaps:
  ! auth-timeout and login-ratelimit intend to change the same paths: src/auth/session.rs
```

The second agent learns about the clash **before writing a line**, and can pick other work or coordinate. When work is done:

```console
$ aweave snapshot -m "Retry session refresh once on timeout" --task SHOP-142
$ aweave propose
merged 1 commit(s) from auth-timeout into main, now at ec6487f7a0da
released intents #1
```

```text
Retry session refresh once on timeout

Aweave-Agent: claude-1
Aweave-Workspace: auth-timeout
Aweave-Task: SHOP-142
Aweave-Intents: 1
```

| Concept | What it does |
|---|---|
| **Workspace** | A git worktree in `.aweave/ws/<name>` on branch `aweave/<name>`, created in one command. The workspace is picked up from the current directory. |
| **Intent** | "I'm about to change these paths" (files, directories or globs), with a lifetime. Released automatically when the workspace merges. |
| **Overlap radar** | Compares every pair of workspaces: files both changed (`change/change`), files one changed that another intends to change (`change/intent`), and colliding intents (`intent/intent`). `aweave check` exits with 2 when there are any. |
| **Snapshot** | Commits everything in the workspace with `Aweave-Agent`, `Aweave-Workspace`, `Aweave-Task` and `Aweave-Intents` trailers. |
| **Integration queue** | `propose` rebases the workspace onto trunk in a separate worktree, runs the verify command there and fast-forwards trunk only if it passes. One proposal at a time; a conflict or a failing test leaves trunk untouched and returns the files or output. |
| **Event log** | Every action in order; `aweave events --follow` or the `aweave_events` tool keeps agents aware of each other. |

State is one SQLite file (`.aweave/state.db`, WAL mode), so agents in separate processes coordinate without a daemon. `.aweave/` is added to `.git/info/exclude` and never shows up in `git status`.

## Install

Prebuilt binaries for macOS (Apple silicon, Intel), Linux (x64, arm64) and Windows (x64, experimental):

```bash
curl -fsSL https://aweave.pages.dev/install.sh | sh     # verifies the SHA-256 before installing
```

Or from source, with Rust 1.89 or newer:

```bash
cargo install --git https://github.com/DanielHromadka/aweave
```

Every download is on the [latest release](https://github.com/DanielHromadka/aweave/releases/latest) with a `.sha256` file next to it.

## Use it from Claude Code, Codex or Cursor

`aweave mcp` is a stdio MCP server. Start each agent in its own workspace and give it its own name:

```bash
cd "$(aweave --agent backend-1 ws new backend-1)"
claude mcp add aweave -- aweave mcp --agent backend-1
```

| Tool | |
|---|---|
| `aweave_status` | Workspaces, changed files, intents and overlaps |
| `aweave_workspace_create` | New isolated workspace for a task |
| `aweave_intent_declare` | Declare paths before changing them; returns overlaps |
| `aweave_intent_release` | Drop an intent |
| `aweave_snapshot` | Checkpoint commit with provenance |
| `aweave_sync` | Rebase onto the current trunk |
| `aweave_propose` | Integration queue: rebase, verify, merge |
| `aweave_events` | What happened since a given event |

A prompt line that works well: *"Before editing, call aweave_intent_declare for the files you will touch. If it reports overlaps, tell me instead of editing. Snapshot after each step and finish with aweave_propose."*

## Commands

```text
aweave init [--trunk main] [--verify "cmd"]   set up in the current repository
aweave ws new|list|rm|path                    workspaces
aweave intent add|list|release                intents
aweave snapshot [-m msg] [--task id]          checkpoint commit with trailers
aweave status | check                         workspaces, intents, overlaps
aweave sync                                   rebase the workspace onto trunk
aweave propose [--no-verify] [--keep-intents] integration queue
aweave events [--since id] [--follow]         event log
aweave config <key> [value]                   trunk, verify
aweave mcp                                    MCP server on stdio
```

Global flags: `--agent` (or `AWEAVE_AGENT`), `--ws` (or `AWEAVE_WORKSPACE`), `--json` for machine-readable output.

Snapshots skip git's commit hooks so agents can checkpoint often; the verify command in `propose` is the gate. Put your linters and secret scanning there.

## Road map

aWeave is at **M0**: a coordination layer on top of plain git. Next, as described in [docs/design.md](docs/design.md):

- **M1: semantic merge.** Diffs and conflicts per function and type (tree-sitter) instead of per line, so independent edits to one file merge on their own.
- **M2: signed provenance.** Model, prompt, tools and test results per change; revert everything one agent or task did in one command.
- **M3: own store and operation log.** Copy-on-write workspaces in milliseconds, first-class conflicts, `undo` for any operation, with a two-way bridge to git.
- **Shared server** for agents on several machines, with live events instead of polling.

## Building from source

```bash
git clone https://github.com/DanielHromadka/aweave.git
cd aweave
cargo build --release      # target/release/aweave
cargo test                 # unit tests and end-to-end tests against real git repositories
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Project structure

```
src/lib.rs       workspaces, intents, the overlap radar, snapshots, sync and the integration queue
src/store.rs     SQLite state: config, workspaces, intents, events
src/scope.rs     intent scopes (paths and globs)
src/git.rs       the only module that runs git
src/mcp.rs       MCP server (JSON-RPC 2.0 over stdio)
src/main.rs      CLI
tests/flow.rs    end-to-end: several agents, conflicts, verification, parallel proposals, MCP
website/         aweave.pages.dev
docs/            design notes
```

## Contributing

Bug reports, ideas and pull requests are welcome.

1. Open an [issue](https://github.com/DanielHromadka/aweave/issues) first for larger changes, so we can agree on the approach.
2. Keep pull requests focused; `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass.
3. Behaviour changes come with a test in `tests/flow.rs` that drives the real CLI against a real repository.

## Security

aWeave runs git and your verify command with your permissions. Please report vulnerabilities privately, as described in [SECURITY.md](SECURITY.md), not in public issues.

## License

aWeave is free software under the [GNU General Public License v3.0 or later](LICENSE).
