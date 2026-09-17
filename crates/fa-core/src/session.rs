//! Session store: SQLite, WAL mode, append-only event log (§4.8 subset).
//!
//! Tables: `sessions`, `events` (append-only; `seq` is the ordering source
//! of truth), `terminals` (projection of terminal lifecycle for audit),
//! `scope_kv` (session scope). Schema version via `PRAGMA user_version`.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde_json::Value;

use crate::{FaError, Result};

const SCHEMA_VERSION: i32 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions(
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    error_mode TEXT NOT NULL DEFAULT 'StopAndReport',
    backend TEXT NOT NULL DEFAULT 'host',
    manifest_version TEXT NOT NULL DEFAULT '',
    cwd TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS events(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    ts TEXT NOT NULL,
    type TEXT NOT NULL,
    v INTEGER NOT NULL DEFAULT 1,
    payload TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id, seq);
CREATE TABLE IF NOT EXISTS terminals(
    session_id TEXT NOT NULL,
    name TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT '{}',
    updated_seq INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(session_id, name)
);
CREATE TABLE IF NOT EXISTS scope_kv(
    session_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(session_id, key)
);
";

pub struct SessionDb {
    conn: Mutex<Connection>,
}

impl SessionDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let version: i32 = conn.query_row("PRAGMA user_version;", [], |r| r.get(0))?;
        if version == 0 {
            conn.execute_batch(SCHEMA)?;
            conn.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION};"))?;
        } else if version != SCHEMA_VERSION {
            return Err(FaError::Other(format!(
                "session DB schema v{version} unsupported by this binary (v{SCHEMA_VERSION}); refusing to misread"
            )));
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn now() -> String {
        chrono_stamp()
    }

    pub fn create_session(
        &self,
        id: &str,
        error_mode: &str,
        backend: &str,
        manifest_version: &str,
        cwd: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO sessions(id, created_at, error_mode, backend, manifest_version, cwd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, Self::now(), error_mode, backend, manifest_version, cwd],
        )?;
        Ok(())
    }

    /// Append one event. Returns the `seq` — the ordering source of truth.
    pub fn append_event(&self, session_id: &str, typ: &str, payload: &Value) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let payload_str = serde_json::to_string(payload)?;
        conn.execute(
            "INSERT INTO events(session_id, ts, type, payload) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, Self::now(), typ, payload_str],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn set_scope(&self, session_id: &str, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO scope_kv(session_id, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id, key) DO UPDATE SET value=excluded.value",
            params![session_id, key, value],
        )?;
        Ok(())
    }

    pub fn scope_all(&self, session_id: &str) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT key, value FROM scope_kv WHERE session_id=?1 ORDER BY key")?;
        let rows = stmt
            .query_map([session_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<(String, String)>, _>>()?;
        Ok(rows)
    }

    /// Projection of terminal lifecycle for audit. `state_json` is the
    /// tool-result JSON of New/Remove-FATerminal.
    pub fn upsert_terminal(&self, session_id: &str, name: &str, state_json: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO terminals(session_id, name, state) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id, name) DO UPDATE SET state=excluded.state",
            params![session_id, name, state_json],
        )?;
        Ok(())
    }

    pub fn remove_terminal(&self, session_id: &str, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM terminals WHERE session_id=?1 AND name=?2",
            params![session_id, name],
        )?;
        Ok(())
    }

    pub fn event_count(&self, session_id: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM events WHERE session_id=?1",
            [session_id],
            |r| r.get(0),
        )?)
    }

    /// Read the append-only event log back, in `seq` order.
    pub fn events(&self, session_id: &str) -> Result<Vec<EventRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT seq, type, payload FROM events WHERE session_id=?1 ORDER BY seq")?;
        let rows = stmt.query_map([session_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (seq, typ, payload_str) = r?;
            let payload: Value = serde_json::from_str(&payload_str).unwrap_or(Value::Null);
            out.push(EventRow { seq, typ, payload });
        }
        Ok(out)
    }
}

/// One row of the append-only event log.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub seq: i64,
    pub typ: String,
    pub payload: Value,
}

/// RFC 3339-ish UTC timestamp without pulling in chrono.
fn chrono_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days since epoch -> civil date (Howard Hinnant's algorithm).
    let z = secs / 86400 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        m,
        d,
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn memdb() -> SessionDb {
        let dir = std::env::temp_dir().join(format!("fa-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        SessionDb::open(&dir.join("s.db")).unwrap()
    }

    #[test]
    fn append_only_log_and_scopes() {
        let db = memdb();
        db.create_session("s1", "StopAndReport", "host", "0.1.0", "/tmp")
            .unwrap();
        let a = db
            .append_event("s1", "user.message", &json!({"text": "hi"}))
            .unwrap();
        let b = db
            .append_event("s1", "tool.call", &json!({"tool": "Read-FAFile"}))
            .unwrap();
        assert!(b > a, "seq is the ordering source of truth");
        assert_eq!(db.event_count("s1").unwrap(), 2);
        db.set_scope("s1", "focus", "demo").unwrap();
        assert_eq!(
            db.scope_all("s1").unwrap(),
            vec![("focus".to_string(), "demo".to_string())]
        );
        db.upsert_terminal("s1", "build", r#"{"State":"running"}"#)
            .unwrap();
        db.remove_terminal("s1", "build").unwrap();
    }
}
