//! What happens to the text after Whisper: the cleanup pass, and for notes
//! the "file this under" instruction. Shared by the dictation loop and
//! `whisperflow eval`, so the eval measures exactly what gets pasted.
//!
//! Cleanup never loses the dictation: if it fails, the raw text is kept and
//! the reason is returned as a warning.

use crate::cleanup::Cleaner;
use crate::engine::Mode;
use crate::notes::split_file_under;
use std::time::Instant;

/// The text ready to paste (or file as a note).
#[derive(Debug, Clone)]
pub struct Processed {
    pub text: String,
    /// Time spent in cleanup, `None` when it didn't run.
    pub cleanup_secs: Option<f32>,
    /// Why some or all of the raw text was kept.
    pub warning: Option<String>,
    /// From "file this under X" (notes only).
    pub category: Option<String>,
}

/// Turn a raw transcript into the text to paste (`Mode::Paste`) or to file
/// as a note (`Mode::Note`). `on_cleanup` is called just before cleanup
/// starts, so the caller can show progress.
pub fn process(raw: &str, mode: Mode, cleaner: Option<&Cleaner>, on_cleanup: impl FnOnce()) -> Processed {
    match mode {
        Mode::Paste => {
            let mut out = Processed { text: raw.to_string(), cleanup_secs: None, warning: None, category: None };
            if let Some(c) = cleaner {
                on_cleanup();
                let started = Instant::now();
                match c.clean(raw) {
                    Ok(text) => out.text = text,
                    Err(e) => out.warning = Some(format!("{e:#}")),
                }
                out.cleanup_secs = Some(started.elapsed().as_secs_f32());
            }
            out
        }
        Mode::Note => {
            let (body, category) = split_file_under(raw);
            let mut out = Processed { text: body, cleanup_secs: None, warning: None, category };
            if let Some(c) = cleaner {
                on_cleanup();
                let started = Instant::now();
                let (text, failed) = c.clean_long(&out.text);
                if failed > 0 {
                    out.warning = Some(format!("Cleanup kept the raw text for {failed} part(s) of the note"));
                }
                out.text = text;
                out.cleanup_secs = Some(started.elapsed().as_secs_f32());
            }
            out
        }
    }
}
