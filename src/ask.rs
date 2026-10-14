//! Q&A over notes (retrieval-augmented generation): find the note chunks
//! closest to the question, then have a local LLM answer from those notes
//! only, citing them as [1], [2], ...

use crate::ollama::{Message, Ollama, Options};
use crate::storage::Hit;
use anyhow::Result;
use chrono::{Local, TimeZone};
use serde::Serialize;

/// Speed matters less here than for cleanup, so a 7B model is fine.
pub const CHAT_MODEL: &str = "qwen2.5:7b";
/// How long the 7B model stays loaded after use. It needs ~5 GB, and RAM is
/// shared with a browser that is always open, so give it back soon; a burst
/// of notes or follow-up questions still finds it loaded.
pub const CHAT_KEEP_ALIVE: &str = "2m";
/// How many chunks to retrieve, and at most how many notes to show the model.
/// Every extra note costs prompt-reading time on a CPU.
pub const SEARCH_CHUNKS: usize = 8;
const MAX_SOURCES: usize = 5;

/// A note the answer may cite.
#[derive(Debug, Clone, Serialize)]
pub struct Source {
    /// The [n] used in the answer.
    pub number: usize,
    pub note_id: i64,
    pub created_at: i64,
    pub category: Option<String>,
    /// The matching parts of the note.
    pub excerpt: String,
}

/// One earlier question and its answer, for follow-up questions.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Turn {
    pub question: String,
    pub answer: String,
}

/// Group search hits by note, best match first, and number them.
pub fn sources_from_hits(hits: &[Hit]) -> Vec<Source> {
    let mut sources: Vec<Source> = Vec::new();
    for hit in hits {
        if let Some(s) = sources.iter_mut().find(|s| s.note_id == hit.note_id) {
            s.excerpt.push_str(" … ");
            s.excerpt.push_str(&hit.text);
        } else if sources.len() < MAX_SOURCES {
            sources.push(Source {
                number: sources.len() + 1,
                note_id: hit.note_id,
                created_at: hit.note_created_at,
                category: hit.category.clone(),
                excerpt: hit.text.clone(),
            });
        }
    }
    sources
}

/// "Monday 12 October 2026"
pub fn format_date(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%A %-d %B %Y").to_string(),
        None => "unknown date".into(),
    }
}

/// Answer `question` from `sources`, streaming the reply through `on_token`.
pub fn answer(
    ollama: &Ollama,
    model: &str,
    question: &str,
    sources: &[Source],
    history: &[Turn],
    on_token: impl FnMut(&str),
) -> Result<String> {
    let system = format!(
        "You answer questions about the user's own voice notes. Today is {today}.\n\
         Rules:\n\
         - Use only the numbered notes you are given. Don't use outside knowledge or guess.\n\
         - Cite every note you use with its number in square brackets, like [2].\n\
         - The notes are dated. When timing matters, say when something was noted.\n\
         - If the notes don't answer the question, say you couldn't find it in the notes.\n\
         - Be brief and plain. Answer in a few sentences or a short list.",
        today = format_date(chrono::Utc::now().timestamp_millis()),
    );

    let mut messages = vec![Message::system(system)];
    // Earlier turns give context for follow-ups like "and what about Tuesday?".
    for turn in history.iter().rev().take(3).rev() {
        messages.push(Message::user(turn.question.clone()));
        messages.push(Message::assistant(turn.answer.clone()));
    }

    let notes = if sources.is_empty() {
        "(no matching notes)".to_string()
    } else {
        sources
            .iter()
            .map(|s| {
                let category = s.category.as_deref().map(|c| format!(", {c}")).unwrap_or_default();
                format!("[{}] ({}{category})\n{}", s.number, format_date(s.created_at), s.excerpt)
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    messages.push(Message::user(format!("Notes:\n\n{notes}\n\nQuestion: {question}")));

    let options = Options { temperature: 0.2, max_tokens: 400, keep_alive: CHAT_KEEP_ALIVE };
    ollama.chat_stream(model, &messages, &options, on_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(note_id: i64, text: &str) -> Hit {
        Hit { chunk_id: 0, note_id, text: text.into(), distance: 0.1, note_created_at: 0, category: None }
    }

    #[test]
    fn sources_group_chunks_by_note() {
        let hits = [hit(7, "a"), hit(3, "b"), hit(7, "c")];
        let sources = sources_from_hits(&hits);
        assert_eq!(sources.len(), 2);
        assert_eq!((sources[0].number, sources[0].note_id, sources[0].excerpt.as_str()), (1, 7, "a … c"));
        assert_eq!((sources[1].number, sources[1].note_id), (2, 3));
    }
}
