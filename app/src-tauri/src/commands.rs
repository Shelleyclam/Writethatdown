//! Commands the UI calls: history, notes, categories, and the notes Q&A.
//!
//! Anything that may call Ollama (filing, re-indexing, asking) runs on a
//! blocking thread so the UI stays responsive.

use crate::AppState;
use serde::Serialize;
use std::path::PathBuf;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};
use whisperflow::ask::{self, Source, Turn};
use whisperflow::notes::{split_file_under, NoteTaker};
use whisperflow::ollama::Ollama;
use whisperflow::storage::{Category, Entry, Note, NoteFilter, Storage};

const HISTORY_LIMIT: usize = 500;

type CmdResult<T> = Result<T, String>;

fn open(path: &PathBuf) -> CmdResult<Storage> {
    Storage::open(path).map_err(|e| format!("{e:#}"))
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// Run `f` on a blocking thread with its own database connection.
async fn blocking<T: Send + 'static>(
    path: PathBuf,
    f: impl FnOnce(&Storage) -> anyhow::Result<T> + Send + 'static,
) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(move || f(&open(&path)?).map_err(err))
        .await
        .map_err(|e| e.to_string())?
}

// ---------- History ----------

#[tauri::command]
pub async fn get_history(state: State<'_, AppState>) -> CmdResult<Vec<Entry>> {
    blocking(state.db_path.clone(), |s| {
        s.prune()?;
        s.recent(HISTORY_LIMIT)
    })
    .await
}

#[tauri::command]
pub async fn delete_entry(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    blocking(state.db_path.clone(), move |s| s.delete(id)).await
}

/// Turn a history entry into a note (categorized and indexed like a
/// dictated one). Returns the note and any warnings.
#[tauri::command]
pub async fn promote_to_note(app: AppHandle, state: State<'_, AppState>, id: i64) -> CmdResult<Saved> {
    let saved = blocking(state.db_path.clone(), move |s| {
        let entry = s.get(id)?.ok_or_else(|| anyhow::anyhow!("That history entry no longer exists"))?;
        let (text, category) = split_file_under(&entry.cleaned_text);
        let filed = NoteTaker::default().file(s, &entry.raw_text, &text, entry.source_app.as_deref(), category)?;
        Ok(Saved { note: filed.note, warnings: filed.warnings })
    })
    .await?;
    let _ = app.emit("notes-changed", ());
    Ok(saved)
}

#[derive(Serialize)]
pub struct Saved {
    note: Note,
    warnings: Vec<String>,
}

// ---------- Notes ----------

#[tauri::command]
pub async fn get_notes(state: State<'_, AppState>) -> CmdResult<Vec<Note>> {
    blocking(state.db_path.clone(), |s| s.notes(NoteFilter::All)).await
}

#[tauri::command]
pub async fn get_categories(state: State<'_, AppState>) -> CmdResult<Vec<Category>> {
    blocking(state.db_path.clone(), |s| s.categories()).await
}

/// Save edited text and re-index it. Returns a warning if the note was
/// saved but couldn't be re-indexed (it will be retried on next start).
#[tauri::command]
pub async fn update_note(state: State<'_, AppState>, id: i64, text: String) -> CmdResult<Option<String>> {
    blocking(state.db_path.clone(), move |s| {
        s.update_note_text(id, text.trim())?;
        let note = s.note(id)?.ok_or_else(|| anyhow::anyhow!("That note no longer exists"))?;
        Ok(NoteTaker::default()
            .index(s, &note)
            .err()
            .map(|e| format!("Saved, but not searchable yet: {e:#}")))
    })
    .await
}

#[tauri::command]
pub async fn set_note_category(state: State<'_, AppState>, id: i64, category: Option<String>) -> CmdResult<()> {
    blocking(state.db_path.clone(), move |s| {
        s.set_note_category(id, category.as_deref().map(str::trim).filter(|c| !c.is_empty()))
    })
    .await
}

#[tauri::command]
pub async fn delete_note(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    blocking(state.db_path.clone(), move |s| s.delete_note(id)).await
}

#[tauri::command]
pub async fn rename_category(state: State<'_, AppState>, id: i64, name: String) -> CmdResult<()> {
    blocking(state.db_path.clone(), move |s| s.rename_category(id, &name)).await
}

#[tauri::command]
pub async fn merge_categories(state: State<'_, AppState>, from: i64, into: i64) -> CmdResult<()> {
    blocking(state.db_path.clone(), move |s| s.merge_categories(from, into)).await
}

// ---------- Q&A ----------

/// Streamed to the Ask view while an answer is generated.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskEvent {
    /// The notes found for the question; sent before the answer starts.
    Sources { sources: Vec<Source> },
    Token { text: String },
}

/// Answer a question from the notes, streaming progress through `on_event`.
/// Returns the full answer.
#[tauri::command]
pub async fn ask(
    state: State<'_, AppState>,
    question: String,
    history: Vec<Turn>,
    on_event: Channel<AskEvent>,
) -> CmdResult<String> {
    blocking(state.db_path.clone(), move |s| {
        let taker = NoteTaker::default();
        // Catch up on notes saved while Ollama was down, so they can be found.
        taker.index_pending(s)?;
        let hits = taker.search(s, &question, ask::SEARCH_CHUNKS)?;
        let sources = ask::sources_from_hits(&hits);
        let _ = on_event.send(AskEvent::Sources { sources: sources.clone() });
        ask::answer(&Ollama::new(None), ask::CHAT_MODEL, &question, &sources, &history, |token| {
            let _ = on_event.send(AskEvent::Token { text: token.to_string() });
        })
    })
    .await
}
