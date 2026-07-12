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

pub const EMBEDDING_DIM: usize = 384;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id        TEXT PRIMARY KEY,
    captured_at_unix_ms INTEGER NOT NULL,
    summary             TEXT NOT NULL,
    tags_json           TEXT NOT NULL,
    payload_json        TEXT NOT NULL
) STRICT;
CREATE VIRTUAL TABLE IF NOT EXISTS vec_workspaces USING vec0(
    embedding FLOAT[384] distance_metric=cosine
);
";

/// Registers sqlite-vec for every subsequent connection in this process.
/// Idempotent; safe to call from each `open`.
fn register_vec_extension() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    type ExtensionInit = unsafe extern "C" fn(
        *mut rusqlite::ffi::sqlite3,
        *mut *mut std::os::raw::c_char,
        *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::os::raw::c_int;
    ONCE.call_once(|| unsafe {
        // sqlite-vec declares its init without the standard extension-entry
        // signature; the C symbol conforms to it (this mirrors the crate's
        // own `sqlite3_vec_init` usage docs).
        let init: ExtensionInit = std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
        rusqlite::ffi::sqlite3_auto_extension(Some(init));
    });
}

impl Storage {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(dir) = db_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        register_vec_extension();
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        info!(path = %db_path.display(), "storage opened");
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_in_memory() -> Result<Self> {
        register_vec_extension();
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
        conn.execute(
            "DELETE FROM vec_workspaces WHERE rowid =
                 (SELECT rowid FROM workspaces WHERE workspace_id = ?1)",
            params![workspace_id],
        )?;
        let n = conn.execute("DELETE FROM workspaces WHERE workspace_id = ?1", params![workspace_id])?;
        Ok(n > 0)
    }

    /// Stores (replacing) the embedding for an existing workspace. The vector
    /// row shares the workspace row's rowid.
    pub fn upsert_embedding(&self, workspace_id: &str, embedding: &[f32]) -> Result<()> {
        assert_eq!(embedding.len(), EMBEDDING_DIM, "embedding dimension mismatch");
        let conn = self.conn.lock().expect("storage mutex poisoned");
        let rowid: i64 = conn.query_row(
            "SELECT rowid FROM workspaces WHERE workspace_id = ?1",
            params![workspace_id],
            |row| row.get(0),
        )?;
        let bytes = embedding_bytes(embedding);
        conn.execute("DELETE FROM vec_workspaces WHERE rowid = ?1", params![rowid])?;
        conn.execute(
            "INSERT INTO vec_workspaces (rowid, embedding) VALUES (?1, ?2)",
            params![rowid, bytes],
        )?;
        Ok(())
    }

    /// KNN over stored workspaces; returns (record, score) with score in
    /// [0, 1] (1 = identical direction; cosine metric).
    pub fn search_embeddings(&self, query: &[f32], limit: usize) -> Result<Vec<(WorkspaceRecord, f64)>> {
        assert_eq!(query.len(), EMBEDDING_DIM, "embedding dimension mismatch");
        let conn = self.conn.lock().expect("storage mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT w.workspace_id, w.captured_at_unix_ms, w.summary, w.tags_json,
                    w.payload_json, v.distance
             FROM vec_workspaces v
             JOIN workspaces w ON w.rowid = v.rowid
             WHERE v.embedding MATCH ?1 AND v.k = ?2
             ORDER BY v.distance",
        )?;
        let bytes = embedding_bytes(query);
        let rows = stmt.query_map(params![bytes, limit as i64], |row| {
            let record = row_to_record(row)?;
            let distance: f64 = row.get(5)?;
            Ok((record, (1.0 - distance).clamp(0.0, 1.0)))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// True once a workspace has a vector in the index.
    pub fn has_embedding(&self, workspace_id: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("storage mutex poisoned");
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM vec_workspaces WHERE rowid =
                 (SELECT rowid FROM workspaces WHERE workspace_id = ?1)",
            params![workspace_id],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }
}

fn embedding_bytes(embedding: &[f32]) -> Vec<u8> {
    use zerocopy::IntoBytes;
    embedding.as_bytes().to_vec()
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

    fn unit_vec(direction: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; EMBEDDING_DIM];
        v[direction] = 1.0;
        v
    }

    #[test]
    fn knn_ranks_by_cosine_similarity() {
        let s = Storage::open_in_memory().unwrap();
        s.upsert_workspace(&record("a", 1)).unwrap();
        s.upsert_workspace(&record("b", 2)).unwrap();
        s.upsert_embedding("a", &unit_vec(0)).unwrap();
        s.upsert_embedding("b", &unit_vec(1)).unwrap();

        // Query mostly along axis 0 with a little of axis 1.
        let mut q = vec![0.0f32; EMBEDDING_DIM];
        q[0] = 0.9;
        q[1] = 0.1;
        let results = s.search_embeddings(&q, 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0.workspace_id, "a");
        assert!(results[0].1 > results[1].1);
        assert!(results[0].1 > 0.9 && results[0].1 <= 1.0);
    }

    #[test]
    fn embedding_upsert_replaces_and_delete_cleans_up() {
        let s = Storage::open_in_memory().unwrap();
        s.upsert_workspace(&record("a", 1)).unwrap();
        assert!(!s.has_embedding("a").unwrap());
        s.upsert_embedding("a", &unit_vec(0)).unwrap();
        s.upsert_embedding("a", &unit_vec(5)).unwrap();
        assert!(s.has_embedding("a").unwrap());
        let results = s.search_embeddings(&unit_vec(5), 1).unwrap();
        assert_eq!(results[0].0.workspace_id, "a");
        s.delete_workspace("a").unwrap();
        assert!(s.search_embeddings(&unit_vec(5), 1).unwrap().is_empty());
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
