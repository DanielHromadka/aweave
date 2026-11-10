//! aWeave: a coordination layer for many AI agents working on one git repository at the same time.
//!
//! Every agent gets its own **workspace** (a git worktree on branch `aweave/<name>`), declares **intents** (which
//! paths it is about to change) before it starts, and hands finished work to the **integration queue**, which
//! rebases it onto trunk, runs the verify command and fast-forwards trunk only when that passes. Overlaps between
//! intents and actual changes are reported the moment they appear, instead of as merge conflicts an hour later.
//!
//! State lives in `.aweave/state.db` (SQLite, WAL) next to `.git`, so agents in separate processes coordinate
//! without a daemon. See `docs/design.md` for the road map.

pub mod git;
pub mod mcp;
pub mod scope;
mod store;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use git::{conflicted, git, git_stdin, ident_args, run, split_z};
use scope::Scope;
use store::Store;
pub use store::{Event, Intent, Workspace};

/// Directory next to `.git` that holds the state, the workspaces and the integration worktree.
pub const DIR: &str = ".aweave";
/// Branch prefix of workspace branches.
pub const BRANCH_PREFIX: &str = "aweave/";
/// Default lifetime of an intent, in seconds.
pub const DEFAULT_TTL: i64 = 2 * 60 * 60;

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Two parties whose work touches the same paths. `a` and `b` are workspace names, or `@agent` for intents
/// declared outside a workspace.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Overlap {
    /// `change/change`: both changed the same files. `change/intent`: `a` changed files inside `b`'s intent.
    /// `intent/intent`: both declared they are going to change the same paths.
    pub kind: &'static str,
    pub a: String,
    pub b: String,
    pub paths: Vec<String>,
}

#[derive(Serialize, Debug)]
pub struct WorkspaceStatus {
    pub name: String,
    pub agent: String,
    pub branch: String,
    pub path: String,
    /// Commits on the workspace branch that trunk does not have yet.
    pub ahead: u64,
    /// Commits on trunk the workspace is not based on yet (run `sync`).
    pub behind: u64,
    /// Files changed against the merge base with trunk, committed or not.
    pub changed: Vec<String>,
}

#[derive(Serialize, Debug)]
pub struct Status {
    pub trunk: String,
    pub trunk_head: String,
    pub workspaces: Vec<WorkspaceStatus>,
    pub intents: Vec<Intent>,
    pub overlaps: Vec<Overlap>,
}

#[derive(Serialize, Debug)]
pub struct IntentReport {
    pub intent: Intent,
    pub overlaps: Vec<Overlap>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProposeOutcome {
    /// Trunk now contains the workspace's commits.
    Merged {
        trunk: String,
        head: String,
        commits: u64,
        released_intents: Vec<i64>,
        /// False when the workspace got new changes while the queue ran; run `sync` to catch up.
        workspace_updated: bool,
    },
    NothingToPropose,
    /// The workspace does not rebase cleanly onto trunk. Nothing changed; run `sync` and resolve.
    Conflict {
        files: Vec<String>,
    },
    /// The verify command failed on the rebased result. Nothing changed.
    Rejected {
        command: String,
        output: String,
    },
}

#[derive(Serialize, Debug)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SyncOutcome {
    UpToDate,
    Rebased {
        head: String,
    },
    /// The rebase stopped on conflicts and was rolled back; the workspace is as it was.
    Conflict {
        files: Vec<String>,
    },
}

pub struct Weave {
    pub root: PathBuf,
    pub dir: PathBuf,
    store: Store,
    agent: Option<String>,
}

impl Weave {
    /// Sets up `.aweave/` in the repository that contains `start`. Safe to run again (updates the settings).
    pub fn init(start: &Path, trunk: Option<String>, verify: Option<String>, agent: Option<String>) -> Result<Weave> {
        let root = git::discover_root(start)?;
        let dir = root.join(DIR);
        fs::create_dir_all(dir.join("ws"))?;
        exclude_dir(&root)?;
        let store = Store::open(&dir)?;
        let trunk = match trunk.or(store.get_config("trunk")?) {
            Some(t) => t,
            None => {
                let head = run(&root, &["symbolic-ref", "--short", "-q", "HEAD"])?;
                let name = head.stdout.trim();
                if head.ok && !name.is_empty() { name.to_string() } else { "main".to_string() }
            }
        };
        if !run(&root, &["rev-parse", "--verify", "-q", &format!("refs/heads/{trunk}")])?.ok {
            bail!("trunk branch `{trunk}` does not exist yet; make a first commit (or pass --trunk)");
        }
        store.set_config("trunk", &trunk)?;
        if let Some(v) = verify {
            store.set_config("verify", &v)?;
        }
        let weave = Weave { root, dir, store, agent };
        weave.event("init", None, json!({ "trunk": trunk }))?;
        Ok(weave)
    }

    /// Opens the aWeave state of the repository that contains `start`.
    pub fn open(start: &Path, agent: Option<String>) -> Result<Weave> {
        let root = git::discover_root(start)?;
        let dir = root.join(DIR);
        if !dir.join("state.db").exists() {
            bail!("aWeave is not set up in {} (run `aweave init`)", root.display());
        }
        let store = Store::open(&dir)?;
        Ok(Weave { root, dir, store, agent: agent.filter(|a| !a.trim().is_empty()) })
    }

    pub fn trunk(&self) -> Result<String> {
        Ok(self.store.get_config("trunk")?.unwrap_or_else(|| "main".into()))
    }

    pub fn config(&self, key: &str) -> Result<Option<String>> {
        self.store.get_config(key)
    }

    pub fn set_config(&self, key: &str, value: &str) -> Result<()> {
        const KEYS: [&str; 2] = ["trunk", "verify"];
        if !KEYS.contains(&key) {
            bail!("unknown setting `{key}` (known: {})", KEYS.join(", "));
        }
        self.store.set_config(key, value)?;
        self.event("config", None, json!({ "key": key, "value": value }))?;
        Ok(())
    }

    /// The acting agent: the explicit one, else the workspace owner, else the OS user.
    pub fn agent_for(&self, ws: Option<&Workspace>) -> String {
        self.agent
            .clone()
            .or_else(|| ws.map(|w| w.agent.clone()))
            .or_else(|| std::env::var("USER").ok())
            .or_else(|| std::env::var("USERNAME").ok())
            .unwrap_or_else(|| "unknown".into())
    }

    fn event(&self, kind: &str, ws: Option<&Workspace>, data: serde_json::Value) -> Result<i64> {
        let agent = self.agent_for(ws);
        self.store.add_event(now(), kind, &agent, ws.map(|w| w.name.as_str()), &data)
    }

    // ---- workspaces ------------------------------------------------------------------------------------

    /// The named workspace, or the one `cwd` is inside of, or none.
    pub fn resolve_workspace(&self, name: Option<&str>, cwd: &Path) -> Result<Option<Workspace>> {
        if let Some(name) = name.filter(|n| !n.is_empty()) {
            return match self.store.get_workspace(name)? {
                Some(ws) => Ok(Some(ws)),
                None => bail!("no workspace named `{name}` (see `aweave ws list`)"),
            };
        }
        let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let ws_dir = self.dir.join("ws");
        if let Ok(rel) = cwd.strip_prefix(&ws_dir)
            && let Some(first) = rel.components().next()
        {
            let name = first.as_os_str().to_string_lossy();
            return self.store.get_workspace(&name);
        }
        Ok(None)
    }

    pub fn require_workspace(&self, name: Option<&str>, cwd: &Path) -> Result<Workspace> {
        self.resolve_workspace(name, cwd)?
            .context("no workspace selected: pass --ws NAME, set AWEAVE_WORKSPACE, or run inside .aweave/ws/<name>")
    }

    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        self.store.list_workspaces()
    }

    /// Creates a workspace: a worktree in `.aweave/ws/<name>` on a new branch `aweave/<name>` from `base`
    /// (trunk by default).
    pub fn create_workspace(&self, name: &str, base: Option<&str>) -> Result<Workspace> {
        let valid = !name.is_empty()
            && name.len() <= 64
            && !name.starts_with(['.', '-'])
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !valid || name.contains("..") {
            bail!("workspace names use letters, digits, `-`, `_` and `.`, up to 64 characters");
        }
        if self.store.get_workspace(name)?.is_some() {
            bail!("workspace `{name}` already exists");
        }
        let trunk = self.trunk()?;
        let base = base.unwrap_or(&trunk);
        let base_sha = git(&self.root, &["rev-parse", "--verify", &format!("{base}^{{commit}}")])
            .with_context(|| format!("unknown base revision `{base}`"))?;
        let branch = format!("{BRANCH_PREFIX}{name}");
        let path = self.dir.join("ws").join(name);
        git(&self.root, &["worktree", "add", "-q", "-b", &branch, &path_str(&path), &base_sha])?;
        let ws = Workspace {
            name: name.to_string(),
            agent: self.agent_for(None),
            branch,
            path: path_str(&path),
            base: base_sha,
            created: now(),
        };
        if let Err(e) = self.store.insert_workspace(&ws) {
            // Another agent won the race for this name: undo the worktree.
            let _ = run(&self.root, &["worktree", "remove", "--force", &ws.path]);
            let _ = run(&self.root, &["branch", "-D", &ws.branch]);
            return Err(e);
        }
        self.event("workspace.created", Some(&ws), json!({ "base": ws.base, "path": ws.path }))?;
        Ok(ws)
    }

    /// Removes a workspace, its worktree and branch. Refuses when it holds work trunk does not have, unless
    /// `force`.
    pub fn remove_workspace(&self, ws: &Workspace, force: bool) -> Result<()> {
        let trunk = self.trunk()?;
        if !force {
            let st = self.workspace_status(ws, &trunk)?;
            if st.ahead > 0 || (Path::new(&ws.path).exists() && self.is_dirty(ws)?) {
                bail!(
                    "workspace `{}` has work that is not on {trunk} yet ({} commit(s), {} changed file(s)); propose it or pass --force",
                    ws.name,
                    st.ahead,
                    st.changed.len()
                );
            }
        }
        if Path::new(&ws.path).exists() {
            let mut args = vec!["worktree", "remove"];
            if force {
                args.push("--force");
            }
            args.push(&ws.path);
            git(&self.root, &args)?;
        } else {
            let _ = run(&self.root, &["worktree", "prune"]);
        }
        let _ = run(&self.root, &["branch", "-D", &ws.branch]);
        let released = self.store.release_workspace_intents(&ws.name, now())?;
        self.store.delete_workspace(&ws.name)?;
        self.event("workspace.removed", Some(ws), json!({ "released_intents": released }))?;
        Ok(())
    }

    fn is_dirty(&self, ws: &Workspace) -> Result<bool> {
        Ok(!git(Path::new(&ws.path), &["status", "--porcelain"])?.is_empty())
    }

    fn changed_files(&self, ws: &Workspace, trunk: &str) -> Result<Vec<String>> {
        let p = Path::new(&ws.path);
        if !p.exists() {
            return Ok(vec![]);
        }
        let mb = git(p, &["merge-base", trunk, "HEAD"])?;
        let mut set = BTreeSet::new();
        set.extend(split_z(&git(p, &["diff", "--name-only", "--no-renames", "-z", &mb])?).map(str::to_string));
        set.extend(split_z(&git(p, &["ls-files", "--others", "--exclude-standard", "-z"])?).map(str::to_string));
        Ok(set.into_iter().collect())
    }

    fn count(&self, range: &str) -> Result<u64> {
        Ok(git(&self.root, &["rev-list", "--count", range])?.trim().parse().unwrap_or(0))
    }

    pub fn workspace_status(&self, ws: &Workspace, trunk: &str) -> Result<WorkspaceStatus> {
        Ok(WorkspaceStatus {
            name: ws.name.clone(),
            agent: ws.agent.clone(),
            branch: ws.branch.clone(),
            path: ws.path.clone(),
            ahead: self.count(&format!("{trunk}..{}", ws.branch))?,
            behind: self.count(&format!("{}..{trunk}", ws.branch))?,
            changed: self.changed_files(ws, trunk)?,
        })
    }

    // ---- intents and the overlap radar -----------------------------------------------------------------

    /// Declares what an agent is about to change and returns the overlaps it causes right now.
    pub fn declare_intent(
        &self,
        ws: Option<&Workspace>,
        description: &str,
        scope: &[String],
        ttl: Option<i64>,
    ) -> Result<IntentReport> {
        let scope = scope::validate(scope)?;
        let ttl = ttl.unwrap_or(DEFAULT_TTL).max(1);
        let agent = self.agent_for(ws);
        let intent =
            self.store.insert_intent(&agent, ws.map(|w| w.name.as_str()), description.trim(), &scope, now(), ttl)?;
        self.event(
            "intent.declared",
            ws,
            json!({ "id": intent.id, "description": intent.description, "scope": intent.scope, "expires": intent.expires }),
        )?;
        let owner = owner(&intent);
        let overlaps: Vec<Overlap> =
            self.status()?.overlaps.into_iter().filter(|o| o.a == owner || o.b == owner).collect();
        if !overlaps.is_empty() {
            self.event("overlap", ws, json!({ "intent": intent.id, "overlaps": overlaps }))?;
        }
        Ok(IntentReport { intent, overlaps })
    }

    pub fn release_intent(&self, id: i64) -> Result<bool> {
        let released = self.store.release_intent(id, now())?;
        if released {
            let ws = match self.store.get_intent(id)?.and_then(|i| i.workspace) {
                Some(name) => self.store.get_workspace(&name)?,
                None => None,
            };
            self.event("intent.released", ws.as_ref(), json!({ "id": id }))?;
        }
        Ok(released)
    }

    pub fn active_intents(&self) -> Result<Vec<Intent>> {
        self.store.active_intents(now())
    }

    pub fn recent_intents(&self, limit: i64) -> Result<Vec<Intent>> {
        self.store.recent_intents(limit)
    }

    /// Workspaces, active intents and every overlap between them.
    pub fn status(&self) -> Result<Status> {
        let trunk = self.trunk()?;
        let trunk_head = git(&self.root, &["rev-parse", "--short", &format!("refs/heads/{trunk}")])?;
        let workspaces = self
            .store
            .list_workspaces()?
            .iter()
            .map(|ws| self.workspace_status(ws, &trunk))
            .collect::<Result<Vec<_>>>()?;
        let intents = self.active_intents()?;
        let tracked = split_z(&git(&self.root, &["ls-tree", "-r", "--name-only", "-z", &trunk])?)
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        let overlaps = radar(&workspaces, &intents, tracked)?;
        Ok(Status { trunk, trunk_head, workspaces, intents, overlaps })
    }

    // ---- snapshots ------------------------------------------------------------------------------------

    /// Commits everything in the workspace, with provenance trailers. `None` when there was nothing to commit.
    pub fn snapshot(&self, ws: &Workspace, message: &str, task: Option<&str>) -> Result<Option<String>> {
        let p = Path::new(&ws.path);
        git(p, &["add", "-A"])?;
        if run(p, &["diff", "--cached", "--quiet"])?.ok {
            return Ok(None);
        }
        let agent = self.agent_for(Some(ws));
        let intents: Vec<String> = self
            .active_intents()?
            .iter()
            .filter(|i| i.workspace.as_deref() == Some(&ws.name))
            .map(|i| i.id.to_string())
            .collect();
        let subject = message.trim();
        let mut msg = format!(
            "{}\n\nAweave-Agent: {agent}\nAweave-Workspace: {}\n",
            if subject.is_empty() { "checkpoint" } else { subject },
            ws.name
        );
        if let Some(task) = task.filter(|t| !t.trim().is_empty()) {
            msg.push_str(&format!("Aweave-Task: {}\n", task.trim()));
        }
        if !intents.is_empty() {
            msg.push_str(&format!("Aweave-Intents: {}\n", intents.join(", ")));
        }
        let mut args = ident_args(p);
        args.extend(["commit", "-q", "--no-verify", "-F", "-"]);
        git_stdin(p, &args, &msg)?;
        let sha = git(p, &["rev-parse", "HEAD"])?;
        self.event("snapshot", Some(ws), json!({ "commit": sha, "message": subject, "task": task }))?;
        Ok(Some(sha))
    }

    // ---- sync and the integration queue ---------------------------------------------------------------

    /// Rebases the workspace onto the current trunk. Uncommitted work is snapshotted first.
    pub fn sync(&self, ws: &Workspace) -> Result<SyncOutcome> {
        let trunk = self.trunk()?;
        let p = Path::new(&ws.path);
        self.snapshot(ws, "aweave: checkpoint before sync", None)?;
        let base = git(p, &["merge-base", &trunk, "HEAD"])?;
        let head = git(p, &["rev-parse", &format!("refs/heads/{trunk}")])?;
        if base == head {
            return Ok(SyncOutcome::UpToDate);
        }
        let mut args = ident_args(p);
        args.extend(["rebase", "-q", &trunk]);
        if !run(p, &args)?.ok {
            let files = conflicted(p);
            let _ = run(p, &["rebase", "--abort"]);
            self.event("sync.conflict", Some(ws), json!({ "files": files }))?;
            return Ok(SyncOutcome::Conflict { files });
        }
        let head = git(p, &["rev-parse", "HEAD"])?;
        self.event("sync", Some(ws), json!({ "head": head }))?;
        Ok(SyncOutcome::Rebased { head })
    }

    /// Hands the workspace to the integration queue: snapshot, rebase onto trunk in a separate worktree, run
    /// the verify command there, fast-forward trunk. One proposal runs at a time; trunk only ever moves to a
    /// commit that passed verification.
    pub fn propose(&self, ws: &Workspace, verify: bool, keep_intents: bool) -> Result<ProposeOutcome> {
        let _lock = self.queue_lock()?;
        let trunk = self.trunk()?;
        let p = Path::new(&ws.path);
        self.snapshot(ws, "aweave: checkpoint before propose", None)?;
        let old = git(&self.root, &["rev-parse", "--verify", &format!("refs/heads/{trunk}")])?;
        let commits = self.count(&format!("{old}..{}", ws.branch))?;
        if commits == 0 {
            return Ok(ProposeOutcome::NothingToPropose);
        }
        self.event("propose.started", Some(ws), json!({ "trunk": old, "commits": commits }))?;

        let int = self.integration_worktree(&old)?;
        git(&int, &["checkout", "-q", "--detach", &ws.branch])?;
        let mut args = ident_args(&int);
        args.extend(["rebase", "-q", &old]);
        if !run(&int, &args)?.ok {
            let files = conflicted(&int);
            let _ = run(&int, &["rebase", "--abort"]);
            self.event("propose.conflict", Some(ws), json!({ "files": files }))?;
            return Ok(ProposeOutcome::Conflict { files });
        }

        if verify && let Some(cmd) = self.store.get_config("verify")?.filter(|c| !c.trim().is_empty()) {
            let (ok, output) = run_shell(&int, &cmd, ws)?;
            if !ok {
                self.event("propose.rejected", Some(ws), json!({ "command": cmd, "output": output }))?;
                return Ok(ProposeOutcome::Rejected { command: cmd, output });
            }
        }

        let new = git(&int, &["rev-parse", "HEAD"])?;
        self.advance_trunk(&trunk, &old, &new)?;

        // Point the workspace at the rebased commits, unless the agent kept writing in the meantime.
        let workspace_updated = if p.exists() && !self.is_dirty(ws)? {
            git(p, &["reset", "-q", "--hard", &new])?;
            true
        } else {
            false
        };
        let released_intents =
            if keep_intents { vec![] } else { self.store.release_workspace_intents(&ws.name, now())? };
        self.event(
            "propose.merged",
            Some(ws),
            json!({ "trunk": trunk, "from": old, "to": new, "commits": commits, "released_intents": released_intents }),
        )?;
        Ok(ProposeOutcome::Merged { trunk, head: new, commits, released_intents, workspace_updated })
    }

    fn queue_lock(&self) -> Result<File> {
        let f = OpenOptions::new().create(true).truncate(false).write(true).open(self.dir.join("queue.lock"))?;
        f.lock().context("could not take the integration queue lock")?;
        Ok(f)
    }

    /// The detached worktree the queue rebases and verifies in, clean and at `at`.
    fn integration_worktree(&self, at: &str) -> Result<PathBuf> {
        let p = self.dir.join("integration");
        if p.join(".git").exists() {
            let _ = run(&p, &["rebase", "--abort"]);
            git(&p, &["reset", "-q", "--hard"])?;
            // Untracked files go, ignored build output stays, so verify runs incrementally.
            git(&p, &["clean", "-fdq"])?;
            git(&p, &["checkout", "-q", "--detach", at])?;
        } else {
            let _ = run(&self.root, &["worktree", "prune"]);
            git(&self.root, &["worktree", "add", "-q", "--detach", &path_str(&p), at])?;
        }
        Ok(p)
    }

    /// Moves trunk from `old` to `new` (a descendant). If trunk is checked out somewhere, that checkout is
    /// fast-forwarded too, so nobody's working tree silently goes stale.
    fn advance_trunk(&self, trunk: &str, old: &str, new: &str) -> Result<()> {
        match self.worktree_with_branch(trunk)? {
            Some(wt) => {
                if !git(&wt, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
                    bail!(
                        "`{trunk}` is checked out in {} with uncommitted changes; commit or stash them so the queue can fast-forward it",
                        wt.display()
                    );
                }
                let head = git(&wt, &["rev-parse", "HEAD"])?;
                if head != old {
                    bail!("`{trunk}` moved while the queue was running; propose again");
                }
                git(&wt, &["merge", "--ff-only", "-q", new])?;
            }
            None => {
                git(&self.root, &["update-ref", &format!("refs/heads/{trunk}"), new, old])
                    .context("trunk moved while the queue was running; propose again")?;
            }
        }
        Ok(())
    }

    fn worktree_with_branch(&self, branch: &str) -> Result<Option<PathBuf>> {
        let list = git(&self.root, &["worktree", "list", "--porcelain"])?;
        let want = format!("branch refs/heads/{branch}");
        let mut current = None;
        for line in list.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                current = Some(PathBuf::from(p));
            } else if line == want {
                return Ok(current);
            }
        }
        Ok(None)
    }

    // ---- events ---------------------------------------------------------------------------------------

    pub fn events_since(&self, since: i64, limit: i64) -> Result<Vec<Event>> {
        self.store.events_since(since, limit)
    }
}

/// Who an intent belongs to for overlap purposes: its workspace, or `@agent`.
fn owner(i: &Intent) -> String {
    i.workspace.clone().unwrap_or_else(|| format!("@{}", i.agent))
}

#[derive(Default)]
struct Footprint {
    changed: BTreeSet<String>,
    patterns: Vec<String>,
    scopes: Vec<Scope>,
}

/// Every pairwise overlap between the parties' changes and intents.
fn radar(workspaces: &[WorkspaceStatus], intents: &[Intent], mut known: BTreeSet<String>) -> Result<Vec<Overlap>> {
    let mut parties: BTreeMap<String, Footprint> = BTreeMap::new();
    for ws in workspaces {
        known.extend(ws.changed.iter().cloned());
        parties.entry(ws.name.clone()).or_default().changed = ws.changed.iter().cloned().collect();
    }
    for i in intents {
        let fp = parties.entry(owner(i)).or_default();
        fp.patterns.extend(i.scope.iter().cloned());
        fp.scopes.push(Scope::new(&i.scope)?);
    }
    let names: Vec<&String> = parties.keys().collect();
    let mut out = vec![];
    for (ia, a) in names.iter().enumerate() {
        for b in &names[ia + 1..] {
            let (fa, fb) = (&parties[*a], &parties[*b]);
            let both: Vec<String> = fa.changed.intersection(&fb.changed).cloned().collect();
            if !both.is_empty() {
                out.push(Overlap { kind: "change/change", a: a.to_string(), b: b.to_string(), paths: both });
            }
            for (x, fx, y, fy) in [(a, fa, b, fb), (b, fb, a, fa)] {
                let paths: Vec<String> =
                    fx.changed.iter().filter(|p| fy.scopes.iter().any(|s| s.matches(p))).cloned().collect();
                if !paths.is_empty() {
                    out.push(Overlap { kind: "change/intent", a: x.to_string(), b: y.to_string(), paths });
                }
            }
            if !fa.scopes.is_empty() && !fb.scopes.is_empty() {
                let in_a = |p: &str| fa.scopes.iter().any(|s| s.matches(p));
                let in_b = |p: &str| fb.scopes.iter().any(|s| s.matches(p));
                let mut paths: BTreeSet<String> = known.iter().filter(|p| in_a(p) && in_b(p)).cloned().collect();
                // Paths that do not exist yet: a pattern of one side that the other side covers.
                paths.extend(fa.patterns.iter().filter(|p| in_b(p) || fb.patterns.contains(p)).cloned());
                paths.extend(fb.patterns.iter().filter(|p| in_a(p)).cloned());
                if !paths.is_empty() {
                    out.push(Overlap {
                        kind: "intent/intent",
                        a: a.to_string(),
                        b: b.to_string(),
                        paths: paths.into_iter().collect(),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Runs the verify command in `dir`, returns whether it passed and the tail of its output.
fn run_shell(dir: &Path, cmd: &str, ws: &Workspace) -> Result<(bool, String)> {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(cmd);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    };
    let out = command
        .current_dir(dir)
        .env("AWEAVE_WORKSPACE", &ws.name)
        .env("AWEAVE_AGENT", &ws.agent)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("could not run verify command `{cmd}`"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(80)..].join("\n");
    Ok((out.status.success(), tail))
}

/// Keeps `.aweave/` out of `git status` in every worktree (info/exclude is shared by all of them).
fn exclude_dir(root: &Path) -> Result<()> {
    let common = PathBuf::from(git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?);
    let exclude = common.join("info").join("exclude");
    fs::create_dir_all(exclude.parent().expect("info dir"))?;
    let line = format!("/{DIR}/");
    let current = fs::read_to_string(&exclude).unwrap_or_default();
    if !current.lines().any(|l| l.trim() == line) {
        let mut f = OpenOptions::new().create(true).append(true).open(&exclude)?;
        if !current.is_empty() && !current.ends_with('\n') {
            writeln!(f)?;
        }
        writeln!(f, "{line}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(name: &str, changed: &[&str]) -> WorkspaceStatus {
        WorkspaceStatus {
            name: name.into(),
            agent: "a".into(),
            branch: format!("aweave/{name}"),
            path: String::new(),
            ahead: 0,
            behind: 0,
            changed: changed.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn intent(id: i64, ws: &str, scope: &[&str]) -> Intent {
        Intent {
            id,
            agent: "a".into(),
            workspace: Some(ws.into()),
            description: String::new(),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            created: 0,
            expires: i64::MAX,
            released: None,
        }
    }

    #[test]
    fn radar_finds_every_kind() {
        let known: BTreeSet<String> = ["src/a.rs", "src/b.rs", "docs/x.md"].iter().map(|s| s.to_string()).collect();
        let w = [ws("one", &["src/a.rs"]), ws("two", &["src/a.rs", "docs/x.md"])];
        let i = [intent(1, "one", &["docs"]), intent(2, "two", &["src/*.rs"])];
        let o = radar(&w, &i, known).unwrap();
        let kinds: Vec<&str> = o.iter().map(|o| o.kind).collect();
        assert!(kinds.contains(&"change/change"));
        assert!(o.iter().any(|o| o.kind == "change/intent" && o.a == "two" && o.paths == ["docs/x.md"]));
        assert!(o.iter().any(|o| o.kind == "change/intent" && o.a == "one" && o.paths == ["src/a.rs"]));
        assert!(!kinds.contains(&"intent/intent"));
    }

    #[test]
    fn intents_on_files_that_do_not_exist_yet_overlap() {
        let i = [intent(1, "one", &["src/new/**"]), intent(2, "two", &["src/new/**"])];
        let o = radar(&[], &i, BTreeSet::new()).unwrap();
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].kind, "intent/intent");
    }

    #[test]
    fn disjoint_work_is_quiet() {
        let w = [ws("one", &["a.txt"]), ws("two", &["b.txt"])];
        let i = [intent(1, "one", &["a.txt"]), intent(2, "two", &["b.txt"])];
        let known = ["a.txt", "b.txt"].iter().map(|s| s.to_string()).collect();
        assert!(radar(&w, &i, known).unwrap().is_empty());
    }
}
