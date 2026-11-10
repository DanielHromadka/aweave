//! MCP server over stdio (newline-delimited JSON-RPC 2.0), so agents call aWeave as tools instead of parsing
//! CLI output. Start it with `aweave mcp --agent <name>` from inside a workspace.

use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::Weave;

const DEFAULT_PROTOCOL: &str = "2025-06-18";

pub struct Server {
    cwd: PathBuf,
    agent: Option<String>,
    workspace: Option<String>,
}

fn tools() -> Value {
    let ws = json!({ "type": "string", "description": "Workspace name. Defaults to the server's --ws or the workspace the server was started in." });
    json!([
        {
            "name": "aweave_status",
            "description": "Workspaces of all agents, their changed files, active intents and every overlap between them. Call before starting work and before proposing.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "aweave_workspace_create",
            "description": "Create an isolated workspace (git worktree on branch aweave/<name>) for one task. Returns its path; do the work there.",
            "inputSchema": { "type": "object", "properties": {
                "name": { "type": "string", "description": "Short name: letters, digits, - _ ." },
                "base": { "type": "string", "description": "Revision to start from (default: trunk)" }
            }, "required": ["name"] }
        },
        {
            "name": "aweave_intent_declare",
            "description": "Declare which paths you are about to change, before changing them. Returns overlaps with other agents' intents and changes, so you can pick other work or coordinate first.",
            "inputSchema": { "type": "object", "properties": {
                "description": { "type": "string", "description": "What you are going to do, one line" },
                "scope": { "type": "array", "items": { "type": "string" }, "description": "Paths or globs: src/auth, src/**/*.sql, README.md" },
                "ttl_secs": { "type": "integer", "description": "Lifetime, default 7200" },
                "workspace": ws
            }, "required": ["description", "scope"] }
        },
        {
            "name": "aweave_intent_release",
            "description": "Release an intent you no longer need. Intents of a workspace are released automatically when it is merged.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }
        },
        {
            "name": "aweave_snapshot",
            "description": "Commit everything in the workspace as a checkpoint, with agent, task and intents recorded as trailers.",
            "inputSchema": { "type": "object", "properties": {
                "message": { "type": "string" },
                "task": { "type": "string", "description": "Ticket or task id, recorded as Aweave-Task" },
                "workspace": ws
            }, "required": ["message"] }
        },
        {
            "name": "aweave_sync",
            "description": "Rebase the workspace onto the current trunk (uncommitted work is snapshotted first). On conflict nothing changes and the conflicting files are listed.",
            "inputSchema": { "type": "object", "properties": { "workspace": ws } }
        },
        {
            "name": "aweave_propose",
            "description": "Hand the finished workspace to the integration queue: rebase onto trunk, run the verify command, fast-forward trunk. Returns merged, conflict or rejected with the verify output.",
            "inputSchema": { "type": "object", "properties": {
                "keep_intents": { "type": "boolean", "description": "Keep intents active after merging (default false)" },
                "workspace": ws
            } }
        },
        {
            "name": "aweave_events",
            "description": "What happened since event id `since`: workspaces, intents, overlaps, snapshots, merges. Poll to stay aware of other agents.",
            "inputSchema": { "type": "object", "properties": {
                "since": { "type": "integer", "description": "Last event id you have seen (default 0)" },
                "limit": { "type": "integer", "description": "Default 100" }
            } }
        }
    ])
}

impl Server {
    pub fn new(cwd: PathBuf, agent: Option<String>, workspace: Option<String>) -> Self {
        Server { cwd, agent, workspace }
    }

    pub fn serve(&self, input: impl BufRead, mut output: impl Write) -> Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(msg) => self.handle(&msg),
                Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
            };
            if let Some(reply) = reply {
                writeln!(output, "{reply}")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// One JSON-RPC message in, at most one out (notifications get no reply).
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned()?;
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match msg.get("method").and_then(Value::as_str).unwrap_or("") {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(DEFAULT_PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "aweave", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "aWeave coordinates agents working on one repository. Work in your own workspace, declare an intent before changing files, check overlaps, snapshot often, and finish with aweave_propose."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(self.call(&params)),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, m)) => error(id, code, &m),
        })
    }

    fn call(&self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        match self.dispatch(name, &args) {
            Ok(v) => {
                // structuredContent has to be an object; lists (events) are wrapped.
                let structured = if v.is_object() { v.clone() } else { json!({ "items": v }) };
                json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }],
                    "structuredContent": structured,
                    "isError": false
                })
            }
            Err(e) => {
                json!({ "content": [{ "type": "text", "text": format!("{e:#}") }], "isError": true })
            }
        }
    }

    fn dispatch(&self, name: &str, args: &Value) -> Result<Value> {
        let weave = Weave::open(&self.cwd, self.agent.clone())?;
        let s = |k: &str| args.get(k).and_then(Value::as_str);
        let ws_name = s("workspace").or(self.workspace.as_deref());
        let v = match name {
            "aweave_status" => serde_json::to_value(weave.status()?)?,
            "aweave_workspace_create" => {
                let name = s("name").ok_or_else(|| anyhow!("`name` is required"))?;
                serde_json::to_value(weave.create_workspace(name, s("base"))?)?
            }
            "aweave_intent_declare" => {
                let description = s("description").ok_or_else(|| anyhow!("`description` is required"))?;
                let scope: Vec<String> = args
                    .get("scope")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let ws = weave.resolve_workspace(ws_name, &self.cwd)?;
                let ttl = args.get("ttl_secs").and_then(Value::as_i64);
                serde_json::to_value(weave.declare_intent(ws.as_ref(), description, &scope, ttl)?)?
            }
            "aweave_intent_release" => {
                let id = args.get("id").and_then(Value::as_i64).ok_or_else(|| anyhow!("`id` is required"))?;
                json!({ "id": id, "released": weave.release_intent(id)? })
            }
            "aweave_snapshot" => {
                let ws = weave.require_workspace(ws_name, &self.cwd)?;
                let commit = weave.snapshot(&ws, s("message").unwrap_or("checkpoint"), s("task"))?;
                json!({ "workspace": ws.name, "commit": commit })
            }
            "aweave_sync" => {
                let ws = weave.require_workspace(ws_name, &self.cwd)?;
                serde_json::to_value(weave.sync(&ws)?)?
            }
            "aweave_propose" => {
                let ws = weave.require_workspace(ws_name, &self.cwd)?;
                let keep = args.get("keep_intents").and_then(Value::as_bool).unwrap_or(false);
                serde_json::to_value(weave.propose(&ws, true, keep)?)?
            }
            "aweave_events" => {
                let since = args.get("since").and_then(Value::as_i64).unwrap_or(0);
                let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(100).clamp(1, 1000);
                serde_json::to_value(weave.events_since(since, limit)?)?
            }
            other => return Err(anyhow!("unknown tool `{other}`")),
        };
        Ok(v)
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
