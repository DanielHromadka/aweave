//! Coordination state shared by every agent: workspaces, intents and the event log. SQLite in WAL mode, so
//! many agent processes can read and write `.aweave/state.db` at the same time without a daemon.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspaces (
    name    TEXT PRIMARY KEY,
    agent   TEXT NOT NULL,
    branch  TEXT NOT NULL,
    path    TEXT NOT NULL,
    base    TEXT NOT NULL,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS intents (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent       TEXT NOT NULL,
    workspace   TEXT,
    description TEXT NOT NULL,
    scope       TEXT NOT NULL,
    created     INTEGER NOT NULL,
    expires     INTEGER NOT NULL,
    released    INTEGER
);
CREATE TABLE IF NOT EXISTS events (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    ts        INTEGER NOT NULL,
    kind      TEXT NOT NULL,
    agent     TEXT NOT NULL,
    workspace TEXT,
    data      TEXT NOT NULL
);
";

#[derive(Serialize, Debug, Clone)]
pub struct Workspace {
    pub name: String,
    pub agent: String,
    pub branch: String,
    pub path: String,
    pub base: String,
    pub created: i64,
}

#[derive(Serialize, Debug, Clone)]
pub struct Intent {
    pub id: i64,
    pub agent: String,
    pub workspace: Option<String>,
    pub description: String,
    pub scope: Vec<String>,
    pub created: i64,
    pub expires: i64,
    pub released: Option<i64>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Event {
    pub id: i64,
    pub ts: i64,
    pub kind: String,
    pub agent: String,
    pub workspace: Option<String>,
    pub data: serde_json::Value,
}

pub struct Store {
    conn: Connection,
}

fn workspace(row: &Row) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        name: row.get(0)?,
        agent: row.get(1)?,
        branch: row.get(2)?,
        path: row.get(3)?,
        base: row.get(4)?,
        created: row.get(5)?,
    })
}

fn intent(row: &Row) -> rusqlite::Result<Intent> {
    let scope: String = row.get(4)?;
    Ok(Intent {
        id: row.get(0)?,
        agent: row.get(1)?,
        workspace: row.get(2)?,
        description: row.get(3)?,
        scope: serde_json::from_str(&scope).unwrap_or_default(),
        created: row.get(5)?,
        expires: row.get(6)?,
        released: row.get(7)?,
    })
}

const WS_COLS: &str = "name, agent, branch, path, base, created";
const INTENT_COLS: &str = "id, agent, workspace, description, scope, created, expires, released";

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        let conn = Connection::open(dir.join("state.db"))?;
        conn.busy_timeout(Duration::from_secs(30))?;
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    pub fn get_config(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM config WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_config(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO config (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    pub fn insert_workspace(&self, ws: &Workspace) -> Result<()> {
        self.conn.execute(
            "INSERT INTO workspaces (name, agent, branch, path, base, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![ws.name, ws.agent, ws.branch, ws.path, ws.base, ws.created],
        )?;
        Ok(())
    }

    pub fn get_workspace(&self, name: &str) -> Result<Option<Workspace>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {WS_COLS} FROM workspaces WHERE name = ?1"), [name], workspace)
            .optional()?)
    }

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {WS_COLS} FROM workspaces ORDER BY created, name"))?;
        let rows = stmt.query_map([], workspace)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn delete_workspace(&self, name: &str) -> Result<()> {
        self.conn.execute("DELETE FROM workspaces WHERE name = ?1", [name])?;
        Ok(())
    }

    pub fn insert_intent(
        &self,
        agent: &str,
        workspace: Option<&str>,
        description: &str,
        scope: &[String],
        now: i64,
        ttl: i64,
    ) -> Result<Intent> {
        self.conn.execute(
            "INSERT INTO intents (agent, workspace, description, scope, created, expires) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![agent, workspace, description, serde_json::to_string(scope)?, now, now + ttl],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(self.get_intent(id)?.expect("just inserted"))
    }

    pub fn get_intent(&self, id: i64) -> Result<Option<Intent>> {
        Ok(self.conn.query_row(&format!("SELECT {INTENT_COLS} FROM intents WHERE id = ?1"), [id], intent).optional()?)
    }

    /// Intents that are neither released nor expired.
    pub fn active_intents(&self, now: i64) -> Result<Vec<Intent>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {INTENT_COLS} FROM intents WHERE released IS NULL AND expires > ?1 ORDER BY id"
        ))?;
        let rows = stmt.query_map([now], intent)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn recent_intents(&self, limit: i64) -> Result<Vec<Intent>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {INTENT_COLS} FROM intents ORDER BY id DESC LIMIT ?1"))?;
        let mut rows = stmt.query_map([limit], intent)?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Releases one intent; false when it was already released or does not exist.
    pub fn release_intent(&self, id: i64, now: i64) -> Result<bool> {
        let n = self.conn.execute("UPDATE intents SET released = ?2 WHERE id = ?1 AND released IS NULL", [id, now])?;
        Ok(n > 0)
    }

    /// Releases every open intent of a workspace and returns their ids.
    pub fn release_workspace_intents(&self, workspace: &str, now: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("UPDATE intents SET released = ?2 WHERE workspace = ?1 AND released IS NULL RETURNING id")?;
        let ids = stmt.query_map(params![workspace, now], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }

    pub fn add_event(
        &self,
        now: i64,
        kind: &str,
        agent: &str,
        workspace: Option<&str>,
        data: &serde_json::Value,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO events (ts, kind, agent, workspace, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![now, kind, agent, workspace, data.to_string()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn events_since(&self, since: i64, limit: i64) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, ts, kind, agent, workspace, data FROM events WHERE id > ?1 ORDER BY id LIMIT ?2")?;
        let rows = stmt
            .query_map([since, limit], |r| {
                let data: String = r.get(5)?;
                Ok(Event {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    kind: r.get(2)?,
                    agent: r.get(3)?,
                    workspace: r.get(4)?,
                    data: serde_json::from_str(&data).unwrap_or(serde_json::Value::Null),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}
