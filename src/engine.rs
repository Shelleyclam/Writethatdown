//! The dictation loop shared by the command-line tool and the desktop app:
//! hold the key → record → transcribe → clean up → paste → save to history.
//!
//! [`run`] blocks and reports progress as [`Event`]s, which the CLI prints
//! and the app turns into the floating pill and history updates.

use crate::audio::{Clip, Recorder};
use crate::cleanup::Cleaner;
use crate::hotkey::{self, KeyEvent};
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
    /// Where to keep paste history; `None` keeps no history.
    pub history_db: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Models are loading.
    Loading,
    Ready { model: String, llm: Option<String>, microphone: String },
    /// The key is down and the mic is recording.
    Listening,
    Transcribing,
    CleaningUp,
    /// The clip was dropped (too short, or silence).
    Ignored { reason: String },
    /// Something went wrong but dictation continues (e.g. cleanup or paste failed).
    Warning { message: String },
    Done(Dictation),
}

#[derive(Debug, Clone, Serialize)]
pub struct Dictation {
    pub raw_text: String,
    pub cleaned_text: String,
    pub source_app: Option<String>,
    pub pasted: bool,
    /// Row id in the history database, when history is on.
    pub history_id: Option<i64>,
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

    let storage = match &config.history_db {
        Some(path) => {
            let s = Storage::open(path)?;
            s.prune()?;
            Some(s)
        }
        None => None,
    };
    let mut last_prune = Instant::now();

    let mut rec = Recorder::new()?;
    let keys = hotkey::listen()?;
    emit(Event::Ready {
        model: transcriber.name.clone(),
        llm: cleaner.as_ref().map(|c| c.model.clone()),
        microphone: rec.device_name(),
    });

    loop {
        let Some(clip) = next_clip(&mut rec, &keys, &mut emit)? else { continue };
        // The app the user is dictating into; the paste goes there too.
        let source_app = paste::foreground_app();
        if config.save_wav {
            if let Err(e) = save_debug_wav(&clip) {
                emit(Event::Warning { message: format!("Could not save recording: {e:#}") });
            }
        }

        emit(Event::Transcribing);
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

        let mut cleaned = raw.clone();
        let mut cleanup_secs = None;
        if let Some(c) = &cleaner {
            emit(Event::CleaningUp);
            let started = Instant::now();
            match c.clean(&raw) {
                Ok(text) => {
                    cleaned = text;
                    cleanup_secs = Some(started.elapsed().as_secs_f32());
                }
                Err(e) => emit(Event::Warning { message: format!("{e:#}") }),
            }
        }

        let mut pasted = false;
        if config.paste {
            match paste::paste(&cleaned) {
                Ok(()) => pasted = true,
                Err(e) => emit(Event::Warning { message: format!("Could not paste: {e:#}") }),
            }
        }

        let mut history_id = None;
        if let Some(s) = &storage {
            match s.add(&raw, &cleaned, source_app.as_deref()) {
                Ok(entry) => history_id = Some(entry.id),
                Err(e) => emit(Event::Warning { message: format!("Could not save to history: {e:#}") }),
            }
            if last_prune.elapsed() > PRUNE_EVERY {
                let _ = s.prune();
                last_prune = Instant::now();
            }
        }

        emit(Event::Done(Dictation {
            raw_text: raw,
            cleaned_text: cleaned,
            source_app,
            pasted,
            history_id,
            audio_secs: clip.duration_secs(),
            transcribe_secs,
            cleanup_secs,
        }));
    }
}

/// Wait for one press-and-release of the push-to-talk key and return the
/// recording, or `None` if it was too short or silent.
pub fn next_clip(
    rec: &mut Recorder,
    keys: &Receiver<KeyEvent>,
    emit: &mut impl FnMut(Event),
) -> Result<Option<Clip>> {
    loop {
        match keys.recv()? {
            KeyEvent::Down if !rec.is_recording() => {
                rec.start()?;
                emit(Event::Listening);
            }
            KeyEvent::Up if rec.is_recording() => {
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
                return Ok(Some(clip));
            }
            _ => {}
        }
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
