//! SQLite storage for dictation history and notes.
//!
//! History holds what was pasted (Right Ctrl). It is short-term: entries
//! older than [`HISTORY_DAYS`] are pruned. Notes (Right Alt) are kept until
//! deleted. Each note is split into chunks, and each chunk gets an embedding
//! vector (via sqlite-vec) so notes can be searched by meaning.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::Once;
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
    CREATE INDEX IF NOT EXISTS history_created_at ON history(created_at);

    CREATE TABLE IF NOT EXISTS categories (
        id   INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE COLLATE NOCASE
    );
    CREATE TABLE IF NOT EXISTS notes (
        id           INTEGER PRIMARY KEY,
        created_at   INTEGER NOT NULL,
        updated_at   INTEGER NOT NULL,
        raw_text     TEXT NOT NULL,
        cleaned_text TEXT NOT NULL,
        source_app   TEXT,
        category_id  INTEGER REFERENCES categories(id) ON DELETE SET NULL
    );
    CREATE INDEX IF NOT EXISTS notes_created_at ON notes(created_at);
    CREATE TABLE IF NOT EXISTS note_chunks (
        id       INTEGER PRIMARY KEY,
        note_id  INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        text     TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS note_chunks_note ON note_chunks(note_id);
    -- rowid = note_chunks.id
    CREATE VIRTUAL TABLE IF NOT EXISTS chunk_vectors
        USING vec0(embedding float[768] distance_metric=cosine);";

/// Size of the embedding vectors (nomic-embed-text).
pub const EMBEDDING_DIMS: usize = 768;

/// A saved note.
#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub id: i64,
    /// Unix time in milliseconds.
    pub created_at: i64,
    pub updated_at: i64,
    pub raw_text: String,
    /// The note as shown and searched (cleaned up, possibly edited).
    pub text: String,
    pub source_app: Option<String>,
    pub category_id: Option<i64>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub note_count: i64,
}

/// Which notes to list.
#[derive(Debug, Clone, Copy)]
pub enum NoteFilter {
    All,
    Category(i64),
    Uncategorized,
}

/// A chunk found by a similarity search.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub chunk_id: i64,
    pub note_id: i64,
    pub text: String,
    /// Cosine distance: 0 is identical, larger is less similar.
    pub distance: f64,
    pub note_created_at: i64,
    pub category: Option<String>,
}

const NOTE_COLUMNS: &str = "n.id, n.created_at, n.updated_at, n.raw_text, n.cleaned_text, n.source_app,
     n.category_id, c.name FROM notes n LEFT JOIN categories c ON c.id = n.category_id";

fn to_note(r: &rusqlite::Row) -> rusqlite::Result<Note> {
    Ok(Note {
        id: r.get(0)?,
        created_at: r.get(1)?,
        updated_at: r.get(2)?,
        raw_text: r.get(3)?,
        text: r.get(4)?,
        source_app: r.get(5)?,
        category_id: r.get(6)?,
        category: r.get(7)?,
    })
}

fn vector_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Make sqlite-vec available to every connection opened afterwards.
fn register_sqlite_vec() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        // SAFETY: sqlite3_vec_init has the signature SQLite expects of an
        // extension entry point; the crate just declares it without arguments.
        #[allow(clippy::missing_transmute_annotations)]
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

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

/// The database the desktop app uses: %APPDATA%\local.whisperflow\whisperflow.db
/// (the folder Tauri gives the app's identifier). `WHISPERFLOW_DB` overrides
/// it, e.g. to test against a scratch database.
pub fn default_path() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("WHISPERFLOW_DB") {
        return path.into();
    }
    let appdata = std::env::var_os("APPDATA").map(std::path::PathBuf::from).unwrap_or_default();
    appdata.join("local.whisperflow").join("whisperflow.db")
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
        register_sqlite_vec();
        let conn = Connection::open(path).with_context(|| format!("Could not open {}", path.display()))?;
        // WAL lets the app's windows read while the dictation thread writes.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Self::init(conn)
    }

    /// In-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        register_sqlite_vec();
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
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


/// Notes, categories, and the search index.
impl Storage {
    pub fn add_note(&self, raw: &str, text: &str, source_app: Option<&str>, category: Option<&str>) -> Result<Note> {
        let category_id = category.map(|c| self.ensure_category(c)).transpose()?;
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO notes (created_at, updated_at, raw_text, cleaned_text, source_app, category_id)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5)",
            params![now, raw, text, source_app, category_id],
        )?;
        Ok(self.note(self.conn.last_insert_rowid())?.expect("note just inserted"))
    }

    pub fn note(&self, id: i64) -> Result<Option<Note>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {NOTE_COLUMNS} WHERE n.id = ?1"), [id], to_note)
            .optional()?)
    }

    /// Newest first.
    pub fn notes(&self, filter: NoteFilter) -> Result<Vec<Note>> {
        let condition = match filter {
            NoteFilter::All => String::new(),
            NoteFilter::Category(id) => format!("WHERE n.category_id = {id}"),
            NoteFilter::Uncategorized => "WHERE n.category_id IS NULL".into(),
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {NOTE_COLUMNS} {condition} ORDER BY n.created_at DESC, n.id DESC"
        ))?;
        let rows = stmt.query_map([], to_note)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn update_note_text(&self, id: i64, text: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE notes SET cleaned_text = ?1, updated_at = ?2 WHERE id = ?3",
            params![text, now_ms(), id],
        )?;
        Ok(())
    }

    /// Move a note to a category (created if new), or out of all categories.
    pub fn set_note_category(&self, id: i64, category: Option<&str>) -> Result<()> {
        let category_id = category.map(|c| self.ensure_category(c)).transpose()?;
        self.conn.execute("UPDATE notes SET category_id = ?1 WHERE id = ?2", params![category_id, id])?;
        self.remove_empty_categories()
    }

    pub fn delete_note(&self, id: i64) -> Result<()> {
        self.delete_vectors_of(id)?;
        self.conn.execute("DELETE FROM notes WHERE id = ?1", [id])?;
        self.remove_empty_categories()
    }

    /// With note counts, by name.
    pub fn categories(&self) -> Result<Vec<Category>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.name, COUNT(n.id) FROM categories c
             LEFT JOIN notes n ON n.category_id = c.id
             GROUP BY c.id ORDER BY c.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| Ok(Category { id: r.get(0)?, name: r.get(1)?, note_count: r.get(2)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The category with this name (any letter case), created if missing.
    pub fn ensure_category(&self, name: &str) -> Result<i64> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "A category needs a name");
        self.conn.execute("INSERT OR IGNORE INTO categories (name) VALUES (?1)", [name])?;
        Ok(self.conn.query_row("SELECT id FROM categories WHERE name = ?1", [name], |r| r.get(0))?)
    }

    /// Rename a category. Renaming to the name of another category merges
    /// the two.
    pub fn rename_category(&self, id: i64, new_name: &str) -> Result<()> {
        let new_name = new_name.trim();
        anyhow::ensure!(!new_name.is_empty(), "A category needs a name");
        let existing: Option<i64> = self
            .conn
            .query_row("SELECT id FROM categories WHERE name = ?1", [new_name], |r| r.get(0))
            .optional()?;
        match existing {
            Some(other) if other != id => self.merge_categories(id, other),
            _ => {
                self.conn.execute("UPDATE categories SET name = ?1 WHERE id = ?2", params![new_name, id])?;
                Ok(())
            }
        }
    }

    /// Move every note from `from` into `into` and delete `from`.
    pub fn merge_categories(&self, from: i64, into: i64) -> Result<()> {
        if from == into {
            return Ok(());
        }
        self.conn.execute("UPDATE notes SET category_id = ?1 WHERE category_id = ?2", params![into, from])?;
        self.conn.execute("DELETE FROM categories WHERE id = ?1", [from])?;
        Ok(())
    }

    fn remove_empty_categories(&self) -> Result<()> {
        self.conn.execute(
            "DELETE FROM categories
             WHERE id NOT IN (SELECT DISTINCT category_id FROM notes WHERE category_id IS NOT NULL)",
            [],
        )?;
        Ok(())
    }

    /// Replace a note's chunks (and drop their vectors). Returns the new chunk ids.
    pub fn replace_chunks(&self, note_id: i64, chunks: &[String]) -> Result<Vec<i64>> {
        self.delete_vectors_of(note_id)?;
        self.conn.execute("DELETE FROM note_chunks WHERE note_id = ?1", [note_id])?;
        let mut ids = Vec::with_capacity(chunks.len());
        for (position, text) in chunks.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO note_chunks (note_id, position, text) VALUES (?1, ?2, ?3)",
                params![note_id, position as i64, text],
            )?;
            ids.push(self.conn.last_insert_rowid());
        }
        Ok(ids)
    }

    pub fn set_chunk_vector(&self, chunk_id: i64, vector: &[f32]) -> Result<()> {
        anyhow::ensure!(
            vector.len() == EMBEDDING_DIMS,
            "Expected {EMBEDDING_DIMS} numbers, got {}",
            vector.len()
        );
        self.conn.execute("DELETE FROM chunk_vectors WHERE rowid = ?1", [chunk_id])?;
        self.conn.execute(
            "INSERT INTO chunk_vectors (rowid, embedding) VALUES (?1, ?2)",
            params![chunk_id, vector_blob(vector)],
        )?;
        Ok(())
    }

    /// Notes that have no chunks yet (never indexed).
    pub fn notes_without_chunks(&self) -> Result<Vec<Note>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {NOTE_COLUMNS} WHERE n.id NOT IN (SELECT note_id FROM note_chunks) ORDER BY n.id"
        ))?;
        let rows = stmt.query_map([], to_note)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Chunks whose embedding is still missing (e.g. Ollama was down).
    pub fn chunks_without_vectors(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, text FROM note_chunks WHERE id NOT IN (SELECT rowid FROM chunk_vectors) ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The `k` chunks closest in meaning to `query`, closest first.
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<Hit>> {
        let mut stmt = self.conn.prepare(
            "SELECT ch.id, ch.note_id, ch.text, v.distance, n.created_at, c.name
             FROM (SELECT rowid, distance FROM chunk_vectors WHERE embedding MATCH ?1 AND k = ?2) v
             JOIN note_chunks ch ON ch.id = v.rowid
             JOIN notes n ON n.id = ch.note_id
             LEFT JOIN categories c ON c.id = n.category_id
             ORDER BY v.distance",
        )?;
        let rows = stmt.query_map(params![vector_blob(query), k as i64], |r| {
            Ok(Hit {
                chunk_id: r.get(0)?,
                note_id: r.get(1)?,
                text: r.get(2)?,
                distance: r.get(3)?,
                note_created_at: r.get(4)?,
                category: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn delete_vectors_of(&self, note_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM chunk_vectors WHERE rowid IN (SELECT id FROM note_chunks WHERE note_id = ?1)",
            [note_id],
        )?;
        Ok(())
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

    /// A unit vector along one axis, padded to the embedding size.
    fn axis(i: usize) -> Vec<f32> {
        let mut v = vec![0.0; EMBEDDING_DIMS];
        v[i] = 1.0;
        v
    }

    #[test]
    fn notes_and_categories() {
        let s = Storage::open_in_memory().unwrap();
        let a = s.add_note("raw a", "Buy milk.", None, Some("Groceries")).unwrap();
        let b = s.add_note("raw b", "Pitch idea.", Some("notepad.exe"), Some("groceries")).unwrap();
        assert_eq!(a.category_id, b.category_id, "category names match case-insensitively");
        let c = s.add_note("raw c", "Call Sam.", None, None).unwrap();
        assert_eq!(s.notes(NoteFilter::All).unwrap().len(), 3);
        let uncategorized: Vec<i64> = s.notes(NoteFilter::Uncategorized).unwrap().iter().map(|n| n.id).collect();
        assert_eq!(uncategorized, vec![c.id]);

        s.set_note_category(b.id, Some("Ideas")).unwrap();
        let cats = s.categories().unwrap();
        let summary: Vec<(&str, i64)> = cats.iter().map(|c| (c.name.as_str(), c.note_count)).collect();
        assert_eq!(summary, vec![("Groceries", 1), ("Ideas", 1)]);

        // Renaming onto an existing name merges.
        let ideas = cats.iter().find(|c| c.name == "Ideas").unwrap().id;
        s.rename_category(ideas, "groceries").unwrap();
        let cats = s.categories().unwrap();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].note_count, 2);
        assert_eq!(s.notes(NoteFilter::Category(cats[0].id)).unwrap().len(), 2);

        s.update_note_text(a.id, "Buy oat milk.").unwrap();
        assert_eq!(s.note(a.id).unwrap().unwrap().text, "Buy oat milk.");
        s.delete_note(a.id).unwrap();
        s.delete_note(b.id).unwrap();
        assert!(s.categories().unwrap().is_empty(), "empty categories are removed");
    }

    #[test]
    fn vector_search_finds_closest_chunk() {
        let s = Storage::open_in_memory().unwrap();
        let note = s.add_note("raw", "text", None, Some("Work")).unwrap();
        assert_eq!(s.notes_without_chunks().unwrap().len(), 1);
        let ids = s.replace_chunks(note.id, &["about cats".into(), "about taxes".into()]).unwrap();
        assert!(s.notes_without_chunks().unwrap().is_empty());
        assert_eq!(s.chunks_without_vectors().unwrap().len(), 2);
        s.set_chunk_vector(ids[0], &axis(0)).unwrap();
        s.set_chunk_vector(ids[1], &axis(1)).unwrap();
        assert!(s.chunks_without_vectors().unwrap().is_empty());

        let hits = s.search(&axis(1), 1).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "about taxes");
        assert_eq!(hits[0].category.as_deref(), Some("Work"));

        // Re-chunking drops the old vectors.
        s.replace_chunks(note.id, &["new text".into()]).unwrap();
        assert_eq!(s.chunks_without_vectors().unwrap().len(), 1);
        s.delete_note(note.id).unwrap();
        assert!(s.search(&axis(1), 5).unwrap().is_empty());
    }
}
