//! SQLite persistence for temporald.
//!
//! One database file holds the flat workspace records; the `payload` column is
//! the canonical wire JSON produced by the shared F# codec, so every consumer
//! (daemon logic, UI over IPC) sees byte-identical state. The vector index
//! (sqlite-vec) is added by the semantic engine milestone.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// A stored workspace. `payload_json` is authoritative; the other columns are
/// denormalized copies for querying/browsing without JSON parsing.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    pub captured_at_unix_ms: i64,
    pub summary: String,
    pub tags_json: String,
    pub payload_json: String,
}

pub struct Storage {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id        TEXT PRIMARY KEY,
    captured_at_unix_ms INTEGER NOT NULL,
    summary             TEXT NOT NULL,
    tags_json           TEXT NOT NULL,
    payload_json        TEXT NOT NULL
) STRICT;
";

impl Storage {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(dir) = db_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        info!(path = %db_path.display(), "storage opened");
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Inserts or replaces the record for this workspace id (flat overwrite
    /// semantics: no history is kept).
    pub fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()> {
        let conn = self.conn.lock().expect("storage mutex poisoned");
        conn.execute(
            "INSERT INTO workspaces
                 (workspace_id, captured_at_unix_ms, summary, tags_json, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(workspace_id) DO UPDATE SET
                 captured_at_unix_ms = excluded.captured_at_unix_ms,
                 summary = excluded.summary,
                 tags_json = excluded.tags_json,
                 payload_json = excluded.payload_json",
            params![
                record.workspace_id,
                record.captured_at_unix_ms,
                record.summary,
                record.tags_json,
                record.payload_json,
            ],
        )?;
        Ok(())
    }

    pub fn get_workspace(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>> {
        let conn = self.conn.lock().expect("storage mutex poisoned");
        let record = conn
            .query_row(
                "SELECT workspace_id, captured_at_unix_ms, summary, tags_json, payload_json
                 FROM workspaces WHERE workspace_id = ?1",
                params![workspace_id],
                row_to_record,
            )
            .optional()?;
        Ok(record)
    }

    /// All workspaces, most recently captured first.
    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>> {
        let conn = self.conn.lock().expect("storage mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT workspace_id, captured_at_unix_ms, summary, tags_json, payload_json
             FROM workspaces ORDER BY captured_at_unix_ms DESC",
        )?;
        let rows = stmt.query_map([], row_to_record)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    pub fn delete_workspace(&self, workspace_id: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("storage mutex poisoned");
        let n = conn.execute("DELETE FROM workspaces WHERE workspace_id = ?1", params![workspace_id])?;
        Ok(n > 0)
    }
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceRecord> {
    Ok(WorkspaceRecord {
        workspace_id: row.get(0)?,
        captured_at_unix_ms: row.get(1)?,
        summary: row.get(2)?,
        tags_json: row.get(3)?,
        payload_json: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, at: i64) -> WorkspaceRecord {
        WorkspaceRecord {
            workspace_id: id.to_string(),
            captured_at_unix_ms: at,
            summary: format!("summary {id}"),
            tags_json: r#"["a","b"]"#.to_string(),
            payload_json: format!(r#"{{"workspaceId":"{id}"}}"#),
        }
    }

    #[test]
    fn upsert_then_get_roundtrips() {
        let s = Storage::open_in_memory().unwrap();
        let r = record("ws-1", 100);
        s.upsert_workspace(&r).unwrap();
        assert_eq!(s.get_workspace("ws-1").unwrap(), Some(r));
    }

    #[test]
    fn upsert_overwrites_flat() {
        let s = Storage::open_in_memory().unwrap();
        s.upsert_workspace(&record("ws-1", 100)).unwrap();
        let mut updated = record("ws-1", 200);
        updated.summary = "new".to_string();
        s.upsert_workspace(&updated).unwrap();
        let got = s.get_workspace("ws-1").unwrap().unwrap();
        assert_eq!(got.captured_at_unix_ms, 200);
        assert_eq!(got.summary, "new");
        assert_eq!(s.list_workspaces().unwrap().len(), 1);
    }

    #[test]
    fn list_orders_by_recency() {
        let s = Storage::open_in_memory().unwrap();
        s.upsert_workspace(&record("old", 100)).unwrap();
        s.upsert_workspace(&record("new", 300)).unwrap();
        s.upsert_workspace(&record("mid", 200)).unwrap();
        let ids: Vec<String> =
            s.list_workspaces().unwrap().into_iter().map(|r| r.workspace_id).collect();
        assert_eq!(ids, vec!["new", "mid", "old"]);
    }

    #[test]
    fn delete_reports_presence() {
        let s = Storage::open_in_memory().unwrap();
        s.upsert_workspace(&record("ws-1", 100)).unwrap();
        assert!(s.delete_workspace("ws-1").unwrap());
        assert!(!s.delete_workspace("ws-1").unwrap());
        assert_eq!(s.get_workspace("ws-1").unwrap(), None);
    }
}
