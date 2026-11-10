//! End-to-end: several agents on one real git repository, driven through the CLI and the MCP server.

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_aweave");

struct Repo {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

/// Git with a clean, isolated configuration: no global hooks, signing or aliases from the machine.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env_remove("AWEAVE_AGENT")
        .env_remove("AWEAVE_WORKSPACE");
}

impl Repo {
    fn new() -> Repo {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let home = tmp.path().join("home");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("gitconfig"), "").unwrap();
        let repo = Repo { root: root.canonicalize().unwrap(), home, _tmp: tmp };
        repo.git(&["init", "-q", "-b", "main"]);
        fs::write(repo.root.join("README.md"), "# demo\n").unwrap();
        fs::write(repo.root.join("src/a.txt"), "a\n").unwrap();
        fs::write(repo.root.join("src/b.txt"), "b\n").unwrap();
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-q", "-m", "initial"]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        isolate(&mut cmd, &self.home);
        let out = cmd.arg("-C").arg(&self.root).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn cmd(&self, dir: &Path, agent: &str, args: &[&str]) -> Command {
        let mut cmd = Command::new(BIN);
        isolate(&mut cmd, &self.home);
        cmd.current_dir(dir).arg("--agent").arg(agent).arg("--json").args(args);
        cmd
    }

    fn run(&self, agent: &str, args: &[&str]) -> Output {
        self.cmd(&self.root, agent, args).output().unwrap()
    }

    /// Runs and parses JSON, asserting the exit code.
    fn ok(&self, agent: &str, args: &[&str]) -> Value {
        let out = self.run(agent, args);
        assert!(out.status.success(), "aweave {args:?} failed: {}{}", stdout(&out), stderr(&out));
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn ws_path(&self, name: &str) -> PathBuf {
        self.root.join(".aweave/ws").join(name)
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn agents_coordinate_through_intents_and_the_queue() {
    let r = Repo::new();
    let init = r.ok("lead", &["init", "--verify", "test -f src/a.txt"]);
    assert_eq!(init["trunk"], "main");
    assert_eq!(r.git(&["status", "--porcelain"]), "", ".aweave must not show up in git status");

    let alpha = r.ok("alpha", &["ws", "new", "alpha"]);
    assert_eq!(alpha["branch"], "aweave/alpha");
    r.ok("beta", &["ws", "new", "beta"]);

    // Alpha claims a.txt; beta then claims every top-level source file and hears about alpha.
    let ia = r.ok("alpha", &["--ws", "alpha", "intent", "add", "fix a", "--scope", "src/a.txt"]);
    assert_eq!(ia["overlaps"], json!([]));
    let ib = r.ok("beta", &["--ws", "beta", "intent", "add", "touch sources", "--scope", "src/*.txt"]);
    let o = &ib["overlaps"][0];
    assert_eq!(o["kind"], "intent/intent");
    assert_eq!(o["paths"], json!(["src/a.txt"]));

    // Work in the workspaces; the workspace is found from the current directory.
    fs::write(r.ws_path("alpha").join("src/a.txt"), "a fixed\n").unwrap();
    fs::write(r.ws_path("beta").join("src/b.txt"), "b changed\n").unwrap();
    fs::write(r.ws_path("beta").join("src/new.txt"), "new\n").unwrap();

    let check = r.run("lead", &["check"]);
    assert_eq!(check.status.code(), Some(2), "{}", stdout(&check));
    let overlaps: Value = serde_json::from_slice(&check.stdout).unwrap();
    assert!(
        overlaps
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["kind"] == "change/intent" && o["a"] == "alpha" && o["b"] == "beta"),
        "{overlaps}"
    );

    let out = r.cmd(&r.ws_path("alpha"), "alpha", &["snapshot", "-m", "Fix a", "--task", "T-1"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));

    // Alpha integrates first; trunk moves and alpha's intent is released.
    let p = r.ok("alpha", &["--ws", "alpha", "propose"]);
    assert_eq!(p["status"], "merged", "{p}");
    assert_eq!(p["commits"], 1);
    assert_eq!(p["released_intents"], json!([ia["intent"]["id"]]));
    assert_eq!(fs::read_to_string(r.root.join("src/a.txt")).unwrap(), "a fixed\n", "main checkout fast-forwarded");
    let msg = r.git(&["log", "-1", "--format=%B", "main"]);
    assert!(msg.contains("Aweave-Agent: alpha") && msg.contains("Aweave-Task: T-1"), "{msg}");
    assert!(msg.contains(&format!("Aweave-Intents: {}", ia["intent"]["id"])), "{msg}");

    // Beta is behind now; propose rebases it onto the new trunk on the way in.
    let p = r.ok("beta", &["--ws", "beta", "propose"]);
    assert_eq!(p["status"], "merged", "{p}");
    assert_eq!(r.git(&["log", "--format=%s", "-3", "main"]).lines().count(), 3);
    assert_eq!(fs::read_to_string(r.root.join("src/a.txt")).unwrap(), "a fixed\n");
    assert!(r.root.join("src/new.txt").exists());
    assert_eq!(r.ok("lead", &["propose", "--ws", "beta"])["status"], "nothing_to_propose");

    // Nothing is left to overlap.
    assert_eq!(r.ok("lead", &["status"])["overlaps"], json!([]));

    // Events tell the whole story in order.
    let ev = r.run("lead", &["events"]);
    let kinds: Vec<String> = stdout(&ev)
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["kind"].as_str().unwrap().to_string())
        .collect();
    for k in ["init", "workspace.created", "intent.declared", "overlap", "snapshot", "propose.merged"] {
        assert!(kinds.iter().any(|x| x == k), "missing {k} in {kinds:?}");
    }

    r.ok("lead", &["ws", "rm", "alpha"]);
    assert!(!r.ws_path("alpha").exists());
}

#[test]
fn conflicts_and_failed_verification_leave_trunk_alone() {
    let r = Repo::new();
    r.ok("lead", &["init"]);
    r.ok("g", &["ws", "new", "gamma"]);
    r.ok("d", &["ws", "new", "delta"]);
    fs::write(r.ws_path("gamma").join("README.md"), "# gamma\n").unwrap();
    fs::write(r.ws_path("delta").join("README.md"), "# delta\n").unwrap();
    assert_eq!(r.ok("g", &["--ws", "gamma", "propose"])["status"], "merged");
    let head = r.git(&["rev-parse", "main"]);

    let out = r.run("d", &["--ws", "delta", "propose"]);
    assert!(!out.status.success());
    let p: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(p["status"], "conflict");
    assert_eq!(p["files"], json!(["README.md"]));
    assert_eq!(r.git(&["rev-parse", "main"]), head);

    // Sync reports the same conflict and leaves the workspace as it was.
    let out = r.run("d", &["--ws", "delta", "sync"]);
    assert!(!out.status.success());
    assert_eq!(fs::read_to_string(r.ws_path("delta").join("README.md")).unwrap(), "# delta\n");

    // A failing verify command keeps trunk where it is and returns the output.
    r.ok("lead", &["config", "verify", "echo checking; exit 3"]);
    r.ok("e", &["ws", "new", "eps"]);
    fs::write(r.ws_path("eps").join("src/c.txt"), "c\n").unwrap();
    let out = r.run("e", &["--ws", "eps", "propose"]);
    let p: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(p["status"], "rejected");
    assert!(p["output"].as_str().unwrap().contains("checking"));
    assert_eq!(r.git(&["rev-parse", "main"]), head);

    // Removing a workspace with unmerged work needs --force.
    assert!(!r.run("e", &["ws", "rm", "eps"]).status.success());
    r.ok("e", &["ws", "rm", "eps", "--force"]);
}

#[test]
fn trunk_not_checked_out_moves_by_ref() {
    let r = Repo::new();
    r.git(&["checkout", "-q", "-b", "elsewhere"]);
    r.ok("lead", &["init", "--trunk", "main"]);
    r.ok("x", &["ws", "new", "x"]);
    fs::write(r.ws_path("x").join("src/x.txt"), "x\n").unwrap();
    assert_eq!(r.ok("x", &["--ws", "x", "propose"])["status"], "merged");
    assert!(r.git(&["ls-tree", "--name-only", "-r", "main"]).contains("src/x.txt"));
    assert!(!r.root.join("src/x.txt").exists(), "the checked-out branch is untouched");
}

#[test]
fn mcp_server_speaks_json_rpc() {
    let r = Repo::new();
    r.ok("lead", &["init"]);
    r.ok("m", &["ws", "new", "mcp"]);

    let mut child =
        r.cmd(&r.ws_path("mcp"), "m", &["mcp"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"aweave_intent_declare","arguments":{"description":"docs","scope":["README.md"]}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"aweave_status","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":6,"method":"bogus"}),
    ];
    {
        let mut stdin = child.stdin.take().unwrap();
        for q in &requests {
            writeln!(stdin, "{q}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let replies: Vec<Value> = stdout(&out).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 6, "the notification gets no reply");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "aweave");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(replies[1]["result"]["tools"].as_array().unwrap().len() >= 8);
    // The intent belongs to the workspace the server runs in.
    assert_eq!(replies[2]["result"]["isError"], false, "{}", replies[2]);
    assert_eq!(replies[2]["result"]["structuredContent"]["intent"]["workspace"], "mcp");
    assert_eq!(replies[3]["result"]["structuredContent"]["intents"][0]["agent"], "m");
    assert_eq!(replies[4]["result"]["isError"], true);
    assert_eq!(replies[5]["error"]["code"], -32601);
}

#[test]
fn parallel_proposals_queue_up() {
    let r = Repo::new();
    r.ok("lead", &["init"]);
    let names: Vec<String> = (0..6).map(|i| format!("w{i}")).collect();
    for n in &names {
        r.ok(n, &["ws", "new", n]);
        fs::write(r.ws_path(n).join(format!("src/{n}.txt")), n).unwrap();
    }
    let children: Vec<_> = names
        .iter()
        .map(|n| r.cmd(&r.root, n, &["--ws", n, "propose"]).stdout(Stdio::piped()).spawn().unwrap())
        .collect();
    for c in children {
        let out = c.wait_with_output().unwrap();
        let p: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(p["status"], "merged", "{p}");
    }
    let files = r.git(&["ls-tree", "--name-only", "-r", "main"]);
    for n in &names {
        assert!(files.contains(&format!("src/{n}.txt")), "{n} missing from main");
    }
    assert_eq!(r.git(&["rev-list", "--count", "main"]), "7");
}
