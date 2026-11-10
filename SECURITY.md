# Security

aWeave runs `git` and the configured verify command with the permissions of whoever starts it, and the MCP server
gives agents tools that commit, rebase and move the trunk branch. Please report vulnerabilities privately through
GitHub's [private vulnerability reporting](https://github.com/DanielHromadka/aweave/security/advisories/new), not in
public issues.

Only the latest release is supported; fixes ship as a regular release.

## Notes for operators

- The verify command (`aweave config verify`) is a shell command stored in `.aweave/state.db`. Anyone who can write
  that file can run code the next time the queue runs, so treat `.aweave/` like `.git/hooks`.
- Snapshots skip git commit hooks. Put linters and secret scanning into the verify command so they gate trunk.
- Agent names in events and trailers are claims, not identities. Signed provenance is on the road map (M2).
