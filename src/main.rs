use anyhow::Result;
use aweave::{Intent, Overlap, ProposeOutcome, SyncOutcome, Weave, mcp};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::process::ExitCode;

/// Version control coordination for many AI agents working on one repository at once.
#[derive(Parser)]
#[command(name = "aweave", version, about, long_about = None)]
struct Cli {
    /// Who is acting (recorded in events and commit trailers). Defaults to the workspace owner, then $USER.
    #[arg(long, global = true, env = "AWEAVE_AGENT")]
    agent: Option<String>,
    /// Workspace to act on. Defaults to the workspace the current directory is in.
    #[arg(long = "ws", global = true, env = "AWEAVE_WORKSPACE")]
    workspace: Option<String>,
    /// Print machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Set up aWeave in the current git repository.
    Init {
        /// Branch the queue integrates into (default: the current branch).
        #[arg(long)]
        trunk: Option<String>,
        /// Command that must pass before trunk moves, e.g. "cargo test" or "npm test".
        #[arg(long)]
        verify: Option<String>,
    },
    /// Create, list and remove agent workspaces.
    #[command(subcommand)]
    Ws(WsCmd),
    /// Declare and release intents: the paths an agent is about to change.
    #[command(subcommand)]
    Intent(IntentCmd),
    /// Commit everything in the workspace as a checkpoint, with provenance trailers.
    Snapshot {
        #[arg(short, long, default_value = "checkpoint")]
        message: String,
        /// Ticket or task id, recorded as Aweave-Task.
        #[arg(long)]
        task: Option<String>,
    },
    /// Workspaces, intents and overlaps.
    Status,
    /// Like status, but only overlaps; exits with 2 when there are any.
    Check,
    /// Rebase the workspace onto the current trunk.
    Sync,
    /// Integrate the workspace: rebase onto trunk, verify, fast-forward trunk.
    Propose {
        /// Skip the verify command.
        #[arg(long)]
        no_verify: bool,
        /// Keep the workspace's intents active after merging.
        #[arg(long)]
        keep_intents: bool,
    },
    /// The event log: who did what, in order.
    Events {
        #[arg(long, default_value_t = 0)]
        since: i64,
        #[arg(long, default_value_t = 100)]
        limit: i64,
        /// Keep printing new events as they happen.
        #[arg(short, long)]
        follow: bool,
    },
    /// Read or change a setting (trunk, verify).
    Config { key: String, value: Option<String> },
    /// Run the MCP server on stdin/stdout.
    Mcp,
}

#[derive(Subcommand)]
enum WsCmd {
    /// Create a workspace and print its path.
    New {
        name: String,
        /// Revision to start from (default: trunk).
        #[arg(long)]
        base: Option<String>,
    },
    List,
    /// Remove a workspace, its worktree and branch.
    Rm {
        name: String,
        /// Also when it holds work that is not on trunk.
        #[arg(long)]
        force: bool,
    },
    /// Print the path of a workspace.
    Path {
        name: String,
    },
}

#[derive(Subcommand)]
enum IntentCmd {
    /// Declare an intent and report overlaps with other agents.
    Add {
        description: String,
        /// Paths or globs: src/auth, 'src/**/*.sql', README.md, . (everything).
        #[arg(short, long = "scope", required = true, num_args = 1..)]
        scope: Vec<String>,
        /// Lifetime in seconds.
        #[arg(long, default_value_t = aweave::DEFAULT_TTL)]
        ttl: i64,
    },
    /// Active intents (or the most recent ones with --all).
    List {
        #[arg(long)]
        all: bool,
    },
    Release {
        id: i64,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("aweave: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn print<T: Serialize>(json: bool, value: &T, human: impl FnOnce()) {
    if json {
        println!("{}", serde_json::to_string_pretty(value).expect("serializable"));
    } else {
        human();
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let json = cli.json;
    let ws_flag = cli.workspace.as_deref();

    if let Cmd::Init { trunk, verify } = cli.command {
        let w = Weave::init(&cwd, trunk, verify, cli.agent)?;
        let info = serde_json::json!({ "root": w.root, "trunk": w.trunk()?, "verify": w.config("verify")? });
        print(json, &info, || {
            println!("aWeave is set up in {}", w.root.display());
            println!("trunk:  {}", info["trunk"].as_str().unwrap_or(""));
            match info["verify"].as_str() {
                Some(v) => println!("verify: {v}"),
                None => {
                    println!("verify: (none; set one with `aweave config verify \"cargo test\"`)")
                }
            }
        });
        return Ok(ExitCode::SUCCESS);
    }
    if let Cmd::Mcp = cli.command {
        let server = mcp::Server::new(cwd, cli.agent, cli.workspace);
        server.serve(std::io::stdin().lock(), std::io::stdout().lock())?;
        return Ok(ExitCode::SUCCESS);
    }

    let w = Weave::open(&cwd, cli.agent.clone())?;
    match cli.command {
        Cmd::Init { .. } | Cmd::Mcp => unreachable!(),
        Cmd::Ws(WsCmd::New { name, base }) => {
            let ws = w.create_workspace(&name, base.as_deref())?;
            print(json, &ws, || println!("{}", ws.path));
        }
        Cmd::Ws(WsCmd::List) => {
            let st = w.status()?;
            print(json, &st.workspaces, || {
                if st.workspaces.is_empty() {
                    println!("no workspaces (create one with `aweave ws new <name>`)");
                }
                for s in &st.workspaces {
                    println!(
                        "{:<20} {:<16} +{} -{}  {} changed  {}",
                        s.name,
                        s.agent,
                        s.ahead,
                        s.behind,
                        s.changed.len(),
                        s.path
                    );
                }
            });
        }
        Cmd::Ws(WsCmd::Rm { name, force }) => {
            let ws = w.require_workspace(Some(&name), &cwd)?;
            w.remove_workspace(&ws, force)?;
            print(json, &serde_json::json!({ "removed": name }), || println!("removed {name}"));
        }
        Cmd::Ws(WsCmd::Path { name }) => {
            let ws = w.require_workspace(Some(&name), &cwd)?;
            print(json, &ws, || println!("{}", ws.path));
        }
        Cmd::Intent(IntentCmd::Add { description, scope, ttl }) => {
            let ws = w.resolve_workspace(ws_flag, &cwd)?;
            let r = w.declare_intent(ws.as_ref(), &description, &scope, Some(ttl))?;
            print(json, &r, || {
                println!("intent #{} declared: {}", r.intent.id, r.intent.scope.join(" "));
                if r.overlaps.is_empty() {
                    println!("no overlaps");
                } else {
                    println!("overlaps:");
                    print_overlaps(&r.overlaps);
                }
            });
        }
        Cmd::Intent(IntentCmd::List { all }) => {
            let list = if all { w.recent_intents(100)? } else { w.active_intents()? };
            print(json, &list, || print_intents(&list));
        }
        Cmd::Intent(IntentCmd::Release { id }) => {
            let released = w.release_intent(id)?;
            print(json, &serde_json::json!({ "id": id, "released": released }), || {
                if released { println!("released #{id}") } else { println!("#{id} was not active") }
            });
            if !released {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::Snapshot { message, task } => {
            let ws = w.require_workspace(ws_flag, &cwd)?;
            let commit = w.snapshot(&ws, &message, task.as_deref())?;
            print(json, &serde_json::json!({ "workspace": ws.name, "commit": commit }), || match &commit {
                Some(c) => println!("{} {}", &c[..12.min(c.len())], message),
                None => println!("nothing to snapshot"),
            });
        }
        Cmd::Status => {
            let st = w.status()?;
            print(json, &st, || {
                println!("trunk {} at {}", st.trunk, st.trunk_head);
                println!();
                println!("workspaces ({})", st.workspaces.len());
                for s in &st.workspaces {
                    println!(
                        "  {:<20} {:<16} +{} -{}  {} changed",
                        s.name,
                        s.agent,
                        s.ahead,
                        s.behind,
                        s.changed.len()
                    );
                }
                println!();
                println!("intents ({})", st.intents.len());
                print_intents(&st.intents);
                println!();
                if st.overlaps.is_empty() {
                    println!("no overlaps");
                } else {
                    println!("overlaps ({})", st.overlaps.len());
                    print_overlaps(&st.overlaps);
                }
            });
        }
        Cmd::Check => {
            let st = w.status()?;
            print(json, &st.overlaps, || {
                if st.overlaps.is_empty() { println!("no overlaps") } else { print_overlaps(&st.overlaps) }
            });
            if !st.overlaps.is_empty() {
                return Ok(ExitCode::from(2));
            }
        }
        Cmd::Sync => {
            let ws = w.require_workspace(ws_flag, &cwd)?;
            let out = w.sync(&ws)?;
            let failed = matches!(out, SyncOutcome::Conflict { .. });
            print(json, &out, || match &out {
                SyncOutcome::UpToDate => println!("{} is up to date with trunk", ws.name),
                SyncOutcome::Rebased { head } => {
                    println!("{} rebased onto trunk, now at {}", ws.name, short(head))
                }
                SyncOutcome::Conflict { files } => {
                    println!("conflict; the workspace is unchanged. Conflicting files:");
                    files.iter().for_each(|f| println!("  {f}"));
                }
            });
            if failed {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::Propose { no_verify, keep_intents } => {
            let ws = w.require_workspace(ws_flag, &cwd)?;
            let out = w.propose(&ws, !no_verify, keep_intents)?;
            let failed = matches!(out, ProposeOutcome::Conflict { .. } | ProposeOutcome::Rejected { .. });
            print(json, &out, || match &out {
                ProposeOutcome::Merged { trunk, head, commits, released_intents, workspace_updated } => {
                    println!("merged {commits} commit(s) from {} into {trunk}, now at {}", ws.name, short(head));
                    if !released_intents.is_empty() {
                        let ids: Vec<String> = released_intents.iter().map(|i| format!("#{i}")).collect();
                        println!("released intents {}", ids.join(" "));
                    }
                    if !workspace_updated {
                        println!("the workspace changed meanwhile; run `aweave sync`");
                    }
                }
                ProposeOutcome::NothingToPropose => {
                    println!("nothing to propose: {} has no commits trunk lacks", ws.name)
                }
                ProposeOutcome::Conflict { files } => {
                    println!(
                        "not merged: {} does not rebase cleanly onto trunk. Run `aweave sync` and resolve:",
                        ws.name
                    );
                    files.iter().for_each(|f| println!("  {f}"));
                }
                ProposeOutcome::Rejected { command, output } => {
                    println!("not merged: `{command}` failed\n{output}");
                }
            });
            if failed {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::Events { since, limit, follow } => {
            let mut last = since;
            loop {
                let events = w.events_since(last, limit)?;
                for e in &events {
                    if json {
                        println!("{}", serde_json::to_string(e)?);
                    } else {
                        let ws = e.workspace.as_deref().map(|w| format!(" [{w}]")).unwrap_or_default();
                        println!("{:>5}  {}  {:<18} {}{}  {}", e.id, e.ts, e.kind, e.agent, ws, e.data);
                    }
                    last = e.id;
                }
                if !follow {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
        Cmd::Config { key, value } => match value {
            Some(v) => {
                w.set_config(&key, &v)?;
                print(json, &serde_json::json!({ key.clone(): v }), || println!("{key} = {v}"));
            }
            None => {
                let v = w.config(&key)?;
                print(json, &serde_json::json!({ key.clone(): v }), || println!("{}", v.as_deref().unwrap_or("")));
            }
        },
    }
    Ok(ExitCode::SUCCESS)
}

fn short(sha: &str) -> &str {
    &sha[..12.min(sha.len())]
}

fn print_intents(list: &[Intent]) {
    if list.is_empty() {
        println!("  none");
    }
    for i in list {
        let owner = i.workspace.clone().unwrap_or_else(|| format!("@{}", i.agent));
        let state = if i.released.is_some() { " (released)" } else { "" };
        println!("  #{:<4} {:<20} {:<28} {}{}", i.id, owner, i.scope.join(" "), i.description, state);
    }
}

fn print_overlaps(list: &[Overlap]) {
    for o in list {
        let what = match o.kind {
            "change/change" => format!("{} and {} both changed", o.a, o.b),
            "change/intent" => format!("{} changed files {} intends to change", o.a, o.b),
            _ => format!("{} and {} intend to change the same paths", o.a, o.b),
        };
        let mut paths = o.paths.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
        if o.paths.len() > 8 {
            paths.push_str(&format!(" … (+{})", o.paths.len() - 8));
        }
        println!("  ! {what}: {paths}");
    }
}
