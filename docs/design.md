# aWeave design

[Česky](design.cs.md)

aWeave is version control for the case where 5 to 50 AI agents (and a few people) work on one repository at the
same time, not one person on one branch.

## The problem

| In git | With many agents |
|---|---|
| One working copy and index per checkout (`index.lock`) | Agents overwrite each other's files and fight over locks |
| Conflicts surface at merge time | Two agents spend an hour on the same function before anyone notices |
| Line-based merge | False conflicts (imports, formatting) and missed semantic clashes |
| A commit is a manual snapshot | An agent that crashes between commits loses its work |
| Metadata is author and message | No record of task, model, prompt, tools or test results |
| Push and pull | Agents cannot see what the others are doing |

## Concepts

1. **Workspaces.** One isolated working copy per agent, created instantly over a shared object store.
2. **Intents.** Before working, an agent declares what it will change (paths now, symbols later). Overlaps are
   reported immediately, so an orchestrator can reschedule before a conflict exists. Intents are advisory, expire,
   and are visible to everyone.
3. **Semantic merge.** Diffs and merges per function and type (tree-sitter); a conflict only when two changes touch
   the same symbol. Text merge as the fallback.
4. **Conflicts as data.** A conflict does not block; it is stored and can be assigned to another agent.
5. **Provenance.** Every change carries agent, task, intent, tools and test results, eventually signed. Revert
   everything one agent or task did in one command.
6. **Operation log and undo** for the whole repository.
7. **Integration queue.** Changes are proposed, rebased onto trunk, verified and merged one at a time. Trunk stays
   green.
8. **Live awareness.** An event stream instead of polling, and an MCP-first interface.

## Milestones

| | Scope | State |
|---|---|---|
| **M0** | Coordination on top of git: worktree workspaces, intents and the overlap radar, snapshots with trailers, integration queue, event log, MCP server. State in SQLite (WAL), no daemon. | done |
| **M1** | Semantic diff and merge (tree-sitter: Rust, TypeScript, C#, Python), symbol-level intents. | next |
| **M2** | Signed provenance, revert by agent or task, richer trailers (model, tools, tests). | |
| **M3** | Own content-addressed store and operation log (or jj-lib), copy-on-write workspaces, first-class conflicts, two-way git bridge. | |
| **Server** | Shared coordination for agents on several machines, push events. | |

## M0 decisions

- **No daemon.** SQLite in WAL mode with a busy timeout lets any number of agent processes read and write the same
  state. The integration queue is serialized with an OS file lock (`.aweave/queue.lock`).
- **Worktrees, not copies.** `git worktree add` shares the object store; checkouts are still full files. Copy-on-write
  clones come with M3.
- **The queue never touches the agent's checkout.** Rebase and verify run in `.aweave/integration`, a detached
  worktree that keeps ignored build output between runs. Trunk moves with a compare-and-swap (`update-ref new old`),
  or a fast-forward when trunk is checked out somewhere.
- **Provenance in trailers.** `Aweave-Agent`, `Aweave-Workspace`, `Aweave-Task`, `Aweave-Intents` survive every git
  host and are searchable with `git log --grep`.
- **Snapshots skip commit hooks.** Agents checkpoint often; the verify command is the gate.

## Prior art

- **Jujutsu (jj):** working copy as a commit, operation log, first-class conflicts, git backend. The closest relative;
  M3 may build on jj-lib.
- **Pijul:** patch theory, commuting changes.
- **Sapling:** stacks, monorepo scale.
- **GitButler:** virtual branches in one working copy.

None of them targets real-time coordination of many agents (intents, overlaps, events), agent provenance and an
MCP-first interface as the primary goal. That is aWeave's niche.

## Open questions

- Build M3 on jj-lib or a new core?
- Symbol-level intents need an index (tree-sitter or LSP): per agent or shared?
- Local state vs. a shared server for agents on several machines.
- Security: an agent must not be able to rewrite another agent's workspace or the log; identities and signatures.
