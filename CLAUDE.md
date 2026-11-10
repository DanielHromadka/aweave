# aWeave

Coordination layer for many AI agents working on one git repository at once. Rust, one binary (`aweave`), GPL-3.0.
User-facing docs: README.md (EN), README.cs.md (CS). Design and road map: docs/design.md (+ design.cs.md).

## Commands

```bash
cargo test                                   # unit + end-to-end (tests/flow.rs drives the real CLI on temp git repos)
cargo clippy --all-targets -- -D warnings    # CI gate
cargo fmt --check                            # rustfmt.toml: max_width 120
python3 -m http.server 8765 -d website       # website preview
```

## Architecture

- `src/lib.rs` — `Weave`: workspaces (git worktrees in `.aweave/ws/<name>`, branch `aweave/<name>`), intents, the
  overlap radar (`radar()`), snapshots with `Aweave-*` trailers, `sync`, and the integration queue (`propose`).
- `src/store.rs` — SQLite state in `.aweave/state.db` (WAL, 30 s busy timeout). Many agent processes share it; there is
  no daemon. Schema changes must stay backward compatible (`CREATE TABLE IF NOT EXISTS`, additive columns).
- `src/git.rs` — the only place that spawns git. Never prompts (GIT_EDITOR=true, GIT_TERMINAL_PROMPT=0).
- `src/mcp.rs` — MCP over stdio, hand-written JSON-RPC 2.0 (no SDK). Tool names `aweave_*`; results carry text and
  `structuredContent` (always an object).
- `src/main.rs` — clap CLI; every command supports `--json`.

## Invariants

- Trunk only moves in `advance_trunk`: compare-and-swap via `update-ref new old`, or `merge --ff-only` where trunk is
  checked out (refusing if that checkout is dirty). Never force.
- The queue (`propose`) holds `.aweave/queue.lock` (OS file lock) for its whole run and works in
  `.aweave/integration`, never in the agent's worktree, except the final `reset --hard` of a clean workspace.
- Failed rebases are always aborted; a conflict or a failed verify leaves trunk and the workspace untouched.
- `.aweave/` is listed in `.git/info/exclude` by `init`.

## Conventions

- Behaviour changes need an end-to-end test in `tests/flow.rs` (isolated git config via `isolate()`).
- Website copy uses real CLI output; regenerate it if the output format changes.
- Release: every push to main publishes `v<major.minor>.<run number>` with stable asset names
  (`aweave-<target>.tar.gz|zip` + `.sha256`); website/site.js and website/install.sh depend on those names.
