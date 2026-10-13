//! SQLite storage for dictation history.
//!
//! History holds what was pasted (Right Ctrl). It is short-term: entries
//! older than [`HISTORY_DAYS`] are pruned. Notes (step 5) will live in their
//! own tables and are kept forever.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Paste history is kept this long.
pub const HISTORY_DAYS: i64 = 7;

/// One dictation.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: i64,
    /// Unix time in milliseconds.
    pub created_at: i64,
    /// Exactly what Whisper heard.
    pub raw_text: String,
    /// After the cleanup pass (same as `raw_text` when cleanup was off or failed).
    pub cleaned_text: String,
    /// The app the text was dictated into, e.g. "chrome.exe".
    pub source_app: Option<String>,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS history (
        id           INTEGER PRIMARY KEY,
        created_at   INTEGER NOT NULL,
        raw_text     TEXT NOT NULL,
        cleaned_text TEXT NOT NULL,
        source_app   TEXT
    );
    CREATE INDEX IF NOT EXISTS history_created_at ON history(created_at);";

const COLUMNS: &str = "id, created_at, raw_text, cleaned_text, source_app";

fn to_entry(r: &rusqlite::Row) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: r.get(0)?,
        created_at: r.get(1)?,
        raw_text: r.get(2)?,
        cleaned_text: r.get(3)?,
        source_app: r.get(4)?,
    })
}

pub struct Storage {
    conn: Connection,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path).with_context(|| format!("Could not open {}", path.display()))?;
        // WAL lets the app's windows read while the dictation thread writes.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// In-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn add(&self, raw: &str, cleaned: &str, source_app: Option<&str>) -> Result<Entry> {
        let created_at = now_ms();
        self.conn.execute(
            "INSERT INTO history (created_at, raw_text, cleaned_text, source_app) VALUES (?1, ?2, ?3, ?4)",
            params![created_at, raw, cleaned, source_app],
        )?;
        Ok(Entry {
            id: self.conn.last_insert_rowid(),
            created_at,
            raw_text: raw.into(),
            cleaned_text: cleaned.into(),
            source_app: source_app.map(Into::into),
        })
    }

    /// Most recent first.
    pub fn recent(&self, limit: usize) -> Result<Vec<Entry>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM history ORDER BY created_at DESC, id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit as i64], to_entry)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get(&self, id: i64) -> Result<Option<Entry>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {COLUMNS} FROM history WHERE id = ?1"), [id], to_entry)
            .optional()?)
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM history WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete history older than [`HISTORY_DAYS`]. Returns how many were removed.
    pub fn prune(&self) -> Result<usize> {
        let cutoff = now_ms() - HISTORY_DAYS * 24 * 60 * 60 * 1000;
        Ok(self.conn.execute("DELETE FROM history WHERE created_at < ?1", [cutoff])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_delete() {
        let s = Storage::open_in_memory().unwrap();
        let a = s.add("um hi", "Hi.", Some("notepad.exe")).unwrap();
        let b = s.add("second", "Second.", None).unwrap();
        let recent = s.recent(10).unwrap();
        assert_eq!(recent.iter().map(|e| e.id).collect::<Vec<_>>(), vec![b.id, a.id]);
        assert_eq!(s.get(a.id).unwrap().unwrap().source_app.as_deref(), Some("notepad.exe"));
        s.delete(a.id).unwrap();
        assert!(s.get(a.id).unwrap().is_none());
    }

    #[test]
    fn prune_removes_only_old_entries() {
        let s = Storage::open_in_memory().unwrap();
        let fresh = s.add("new", "New.", None).unwrap();
        let old = s.add("old", "Old.", None).unwrap();
        let eight_days_ago = now_ms() - 8 * 24 * 60 * 60 * 1000;
        s.conn
            .execute("UPDATE history SET created_at = ?1 WHERE id = ?2", params![eight_days_ago, old.id])
            .unwrap();
        assert_eq!(s.prune().unwrap(), 1);
        assert!(s.get(fresh.id).unwrap().is_some());
        assert!(s.get(old.id).unwrap().is_none());
    }
}
