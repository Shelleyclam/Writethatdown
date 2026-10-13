//! The dictation loop shared by the command-line tool and the desktop app.
//!
//! - Right Ctrl: record → transcribe → clean up → paste → save to history.
//! - Right Alt (when notes are on): record → transcribe → clean up → file as
//!   a note (category + search index). Nothing is pasted.
//!
//! [`run`] blocks and reports progress as [`Event`]s, which the CLI prints
//! and the app turns into the floating pill and window updates.

use crate::audio::{Clip, Recorder};
use crate::cleanup::Cleaner;
use crate::hotkey::{self, Key, KeyEvent};
use crate::notes::{split_file_under, NoteTaker};
use crate::paste;
use crate::storage::Storage;
use crate::transcribe::{find_model, Transcriber};
use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// Presses shorter than this are treated as accidental taps.
const MIN_CLIP_SECS: f32 = 0.3;
/// Below this loudness the clip is treated as silence (avoids Whisper
/// "hallucinating" text like "Thank you." from nothing).
const SILENCE_RMS: f32 = 0.002;
/// How often old history is pruned while running.
const PRUNE_EVERY: Duration = Duration::from_secs(60 * 60);

pub struct Config {
    /// Whisper model name or path; `None` picks the default.
    pub model: Option<String>,
    pub threads: i32,
    /// Ollama model for cleanup; `None` turns cleanup off.
    pub llm: Option<String>,
    pub paste: bool,
    pub save_wav: bool,
    /// Database for history and notes; `None` keeps neither.
    pub db: Option<PathBuf>,
    /// Enable the Right Alt note key (needs `db`).
    pub notes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Right Ctrl: paste into the focused app.
    Paste,
    /// Right Alt: save as a note.
    Note,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Models are loading.
    Loading,
    Ready { model: String, llm: Option<String>, microphone: String, notes: bool },
    /// A key is down and the mic is recording.
    Listening { mode: Mode },
    Transcribing { mode: Mode },
    CleaningUp { mode: Mode },
    /// Choosing a category and indexing a note.
    Filing,
    /// The clip was dropped (too short, or silence).
    Ignored { reason: String },
    /// Something went wrong but dictation continues (e.g. cleanup or paste failed).
    Warning { message: String },
    Done(Dictation),
    /// Notes or history changed outside a dictation (e.g. indexing caught up).
    NotesChanged,
}

#[derive(Debug, Clone, Serialize)]
pub struct Dictation {
    pub mode: Mode,
    pub raw_text: String,
    pub cleaned_text: String,
    pub source_app: Option<String>,
    pub pasted: bool,
    /// Row id in the history table (paste mode, when history is on).
    pub history_id: Option<i64>,
    /// The saved note (note mode).
    pub note_id: Option<i64>,
    pub category: Option<String>,
    pub audio_secs: f32,
    pub transcribe_secs: f32,
    /// `None` when cleanup was skipped.
    pub cleanup_secs: Option<f32>,
}

/// Run the dictation loop until a fatal error (no model, no microphone, ...).
pub fn run(config: Config, mut emit: impl FnMut(Event)) -> Result<()> {
    emit(Event::Loading);
    let transcriber = Transcriber::load(&find_model(config.model.as_deref())?, config.threads)?;

    // Cleanup is optional: if Ollama isn't up yet (e.g. right after login),
    // keep the cleaner and let each dictation try again.
    let cleaner = config.llm.as_deref().map(Cleaner::new);
    if let Some(c) = &cleaner {
        if let Err(e) = c.warm_up() {
            emit(Event::Warning { message: format!("Cleanup unavailable for now: {e:#}") });
        }
    }

    let storage = match &config.db {
        Some(path) => {
            let s = Storage::open(path)?;
            s.prune()?;
            Some(s)
        }
        None => None,
    };
    let note_taker = (config.notes && storage.is_some()).then(NoteTaker::default);
    let mut last_prune = Instant::now();

    let mut rec = Recorder::new()?;
    let keys = hotkey::listen(note_taker.is_some())?;
    emit(Event::Ready {
        model: transcriber.name.clone(),
        llm: cleaner.as_ref().map(|c| c.model.clone()),
        microphone: rec.device_name(),
        notes: note_taker.is_some(),
    });

    // Index notes saved while Ollama was unavailable.
    if let (Some(taker), Some(s)) = (&note_taker, &storage) {
        match taker.index_pending(s) {
            Ok(0) => {}
            Ok(_) => emit(Event::NotesChanged),
            Err(e) => emit(Event::Warning { message: format!("Some notes aren't searchable yet: {e:#}") }),
        }
    }

    loop {
        let Some((mode, clip)) = next_clip(&mut rec, &keys, &mut emit)? else { continue };
        // The app the user is dictating into; the paste goes there too.
        let source_app = paste::foreground_app();
        if config.save_wav {
            if let Err(e) = save_debug_wav(&clip) {
                emit(Event::Warning { message: format!("Could not save recording: {e:#}") });
            }
        }

        emit(Event::Transcribing { mode });
        let started = Instant::now();
        let raw = match transcriber.transcribe(&clip) {
            Ok(text) => text,
            Err(e) => {
                emit(Event::Warning { message: format!("{e:#}") });
                continue;
            }
        };
        let transcribe_secs = started.elapsed().as_secs_f32();
        if raw.is_empty() {
            emit(Event::Ignored { reason: "no speech detected".into() });
            continue;
        }

        let mut done = Dictation {
            mode,
            raw_text: raw.clone(),
            cleaned_text: raw.clone(),
            source_app,
            pasted: false,
            history_id: None,
            note_id: None,
            category: None,
            audio_secs: clip.duration_secs(),
            transcribe_secs,
            cleanup_secs: None,
        };

        match mode {
            Mode::Paste => {
                if let Some(c) = &cleaner {
                    emit(Event::CleaningUp { mode });
                    let started = Instant::now();
                    match c.clean(&raw) {
                        Ok(text) => {
                            done.cleaned_text = text;
                            done.cleanup_secs = Some(started.elapsed().as_secs_f32());
                        }
                        Err(e) => emit(Event::Warning { message: format!("{e:#}") }),
                    }
                }
                if config.paste {
                    match paste::paste(&done.cleaned_text) {
                        Ok(()) => done.pasted = true,
                        Err(e) => emit(Event::Warning { message: format!("Could not paste: {e:#}") }),
                    }
                }
                if let Some(s) = &storage {
                    match s.add(&raw, &done.cleaned_text, done.source_app.as_deref()) {
                        Ok(entry) => done.history_id = Some(entry.id),
                        Err(e) => emit(Event::Warning { message: format!("Could not save to history: {e:#}") }),
                    }
                }
            }
            Mode::Note => {
                // next_clip only returns notes when both exist.
                let (Some(taker), Some(s)) = (&note_taker, &storage) else { continue };
                let (body, category) = split_file_under(&raw);
                done.cleaned_text = body.clone();
                if let Some(c) = &cleaner {
                    emit(Event::CleaningUp { mode });
                    let started = Instant::now();
                    let (text, failed) = c.clean_long(&body);
                    if failed > 0 {
                        emit(Event::Warning {
                            message: format!("Cleanup kept the raw text for {failed} part(s) of the note"),
                        });
                    }
                    done.cleaned_text = text;
                    done.cleanup_secs = Some(started.elapsed().as_secs_f32());
                }
                emit(Event::Filing);
                match taker.file(s, &raw, &done.cleaned_text, done.source_app.as_deref(), category) {
                    Ok(filed) => {
                        for message in filed.warnings {
                            emit(Event::Warning { message });
                        }
                        done.note_id = Some(filed.note.id);
                        done.category = filed.note.category;
                    }
                    Err(e) => {
                        emit(Event::Warning { message: format!("Could not save the note: {e:#}") });
                        continue;
                    }
                }
            }
        }

        if let Some(s) = &storage {
            if last_prune.elapsed() > PRUNE_EVERY {
                let _ = s.prune();
                last_prune = Instant::now();
            }
        }
        emit(Event::Done(done));
    }
}

/// Wait for one press-and-release of a push-to-talk key and return the
/// recording with the mode it was made in, or `None` if it was too short or
/// silent. While one key is recording, the other key is ignored.
pub fn next_clip(
    rec: &mut Recorder,
    keys: &Receiver<KeyEvent>,
    emit: &mut impl FnMut(Event),
) -> Result<Option<(Mode, Clip)>> {
    let mut held: Option<Key> = None;
    loop {
        match keys.recv()? {
            KeyEvent::Down(key) if held.is_none() => {
                rec.start()?;
                held = Some(key);
                emit(Event::Listening { mode: mode_of(key) });
            }
            KeyEvent::Up(key) if held == Some(key) => {
                let clip = rec.stop();
                if clip.duration_secs() < MIN_CLIP_SECS {
                    emit(Event::Ignored { reason: "too short".into() });
                    return Ok(None);
                }
                if clip.rms() < SILENCE_RMS {
                    emit(Event::Ignored {
                        reason: "only silence. Is the right microphone selected?".into(),
                    });
                    return Ok(None);
                }
                return Ok(Some((mode_of(key), clip)));
            }
            _ => {}
        }
    }
}

fn mode_of(key: Key) -> Mode {
    match key {
        Key::Dictate => Mode::Paste,
        Key::Note => Mode::Note,
    }
}

pub fn save_debug_wav(clip: &Clip) -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let path = PathBuf::from("recordings").join(format!("clip-{stamp}.wav"));
    clip.save_wav(&path)?;
    Ok(path)
}
