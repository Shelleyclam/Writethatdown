//! The dictation loop shared by the command-line tool and the desktop app.
//!
//! - Right Ctrl: record → transcribe → clean up → paste → save to history.
//! - Right Alt (when notes are on): record → transcribe → clean up → file as
//!   a note (category + search index). Nothing is pasted.
//!
//! Either key works two ways: hold it to talk (push-to-talk), or tap it once
//! to start hands-free recording and tap it again to stop. Esc cancels a
//! recording; hands-free recordings stop by themselves after
//! [`MAX_RECORDING`].
//!
//! [`run`] blocks and reports progress as [`Event`]s, which the CLI prints
//! and the app turns into the floating pill and window updates.

use crate::audio::{Clip, Recorder};
use crate::cleanup::Cleaner;
use crate::hotkey::{self, Key, KeyEvent, Press};
use crate::notes::NoteTaker;
use crate::paste;
use crate::pipeline;
use crate::storage::Storage;
use crate::transcribe::{find_model, Transcriber};
use anyhow::{bail, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Presses shorter than this are treated as accidental taps.
const MIN_CLIP_SECS: f32 = 0.3;
/// Below this loudness the clip is treated as silence (avoids Whisper
/// "hallucinating" text like "Thank you." from nothing).
const SILENCE_RMS: f32 = 0.002;
/// How often old history is pruned while running.
const PRUNE_EVERY: Duration = Duration::from_secs(60 * 60);
/// A forgotten hands-free recording stops (and is transcribed) after this.
pub const MAX_RECORDING: Duration = Duration::from_secs(10 * 60);

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
    /// The mic is recording. `hands_free` is false while the key is held,
    /// and true after a tap (another tap stops it).
    Listening { mode: Mode, hands_free: bool },
    Transcribing { mode: Mode },
    CleaningUp { mode: Mode },
    /// Choosing a category and indexing a note.
    Filing,
    /// The recording was dropped (too short, silence, cancelled, or the key
    /// was part of a shortcut).
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

        let processed = pipeline::process(&raw, mode, cleaner.as_ref(), || emit(Event::CleaningUp { mode }));
        done.cleaned_text = processed.text;
        done.cleanup_secs = processed.cleanup_secs;
        if let Some(message) = processed.warning {
            // A pasted dictation that fell back to raw reports no cleanup time.
            if mode == Mode::Paste {
                done.cleanup_secs = None;
            }
            emit(Event::Warning { message });
        }

        match mode {
            Mode::Paste => {
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
                emit(Event::Filing);
                match taker.file(s, &raw, &done.cleaned_text, done.source_app.as_deref(), processed.category) {
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

/// Wait for one recording (held, or tapped on and off) and return it with
/// the mode it was made in, or `None` if it was cancelled, part of a
/// shortcut, too short, or silent.
pub fn next_clip(
    rec: &mut Recorder,
    keys: &Receiver<KeyEvent>,
    emit: &mut impl FnMut(Event),
) -> Result<Option<(Mode, Clip)>> {
    let mut trigger = Trigger::default();
    let mut started = Instant::now();
    loop {
        // Wake up regularly to enforce the time limit.
        let step = match keys.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => trigger.on_key(event),
            Err(RecvTimeoutError::Timeout) if trigger.is_active() && started.elapsed() > MAX_RECORDING => {
                emit(Event::Warning { message: "Recording stopped after 10 minutes".into() });
                trigger.finish()
            }
            Err(RecvTimeoutError::Timeout) => Step::Nothing,
            Err(RecvTimeoutError::Disconnected) => bail!("The keyboard hook stopped"),
        };
        match step {
            Step::Nothing => {}
            Step::Start(mode) => {
                rec.start()?;
                hotkey::set_recording(true);
                started = Instant::now();
                emit(Event::Listening { mode, hands_free: false });
            }
            Step::HandsFree(mode) => emit(Event::Listening { mode, hands_free: true }),
            Step::Discard(reason) => {
                rec.stop();
                hotkey::set_recording(false);
                emit(Event::Ignored { reason: reason.into() });
                return Ok(None);
            }
            Step::Finish(mode) => {
                let clip = rec.stop();
                hotkey::set_recording(false);
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
                return Ok(Some((mode, clip)));
            }
        }
    }
}

/// What to do with the microphone after a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Nothing,
    /// Start recording.
    Start(Mode),
    /// Keep recording after the key is released (it was tapped).
    HandsFree(Mode),
    /// Stop and transcribe.
    Finish(Mode),
    /// Stop and throw the recording away.
    Discard(&'static str),
}

/// Turns key events into recording steps. Kept free of audio and timing so
/// it can be tested; the keyboard hook can't be driven with simulated keys.
#[derive(Default)]
struct Trigger {
    /// The key that started the current recording, and whether it switched
    /// to hands-free.
    active: Option<(Key, bool)>,
}

impl Trigger {
    fn is_active(&self) -> bool {
        self.active.is_some()
    }

    fn on_key(&mut self, event: KeyEvent) -> Step {
        let Some((key, hands_free)) = self.active else {
            return match event {
                KeyEvent::Down(key) => {
                    self.active = Some((key, false));
                    Step::Start(mode_of(key))
                }
                _ => Step::Nothing,
            };
        };
        match event {
            KeyEvent::Cancel => {
                self.active = None;
                Step::Discard("cancelled")
            }
            // Holding: the release ends it, unless it was a quick tap.
            KeyEvent::Up(k, press) if k == key && !hands_free => match press {
                Press::Tap => {
                    self.active = Some((key, true));
                    Step::HandsFree(mode_of(key))
                }
                Press::Hold => self.finish(),
                Press::Combo => {
                    self.active = None;
                    Step::Discard("the key was part of a shortcut")
                }
            },
            // Hands-free: the next press of the same key ends it, but not
            // when it's used in a shortcut.
            KeyEvent::Up(k, Press::Tap | Press::Hold) if k == key && hands_free => self.finish(),
            // The other key, and the next press's key-down, change nothing.
            _ => Step::Nothing,
        }
    }

    fn finish(&mut self) -> Step {
        match self.active.take() {
            Some((key, _)) => Step::Finish(mode_of(key)),
            None => Step::Nothing,
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

#[cfg(test)]
mod tests {
    use super::*;
    use KeyEvent::{Cancel, Down, Up};

    fn run(events: &[KeyEvent]) -> Vec<Step> {
        let mut t = Trigger::default();
        events.iter().map(|e| t.on_key(*e)).collect()
    }

    #[test]
    fn hold_to_talk() {
        assert_eq!(
            run(&[Down(Key::Dictate), Up(Key::Dictate, Press::Hold)]),
            vec![Step::Start(Mode::Paste), Step::Finish(Mode::Paste)]
        );
    }

    #[test]
    fn tap_on_tap_off() {
        let steps = run(&[
            Down(Key::Note),
            Up(Key::Note, Press::Tap),
            Down(Key::Note),
            Up(Key::Note, Press::Tap),
        ]);
        assert_eq!(
            steps,
            vec![Step::Start(Mode::Note), Step::HandsFree(Mode::Note), Step::Nothing, Step::Finish(Mode::Note)]
        );
    }

    #[test]
    fn shortcuts_are_not_dictation() {
        assert_eq!(
            run(&[Down(Key::Dictate), Up(Key::Dictate, Press::Combo)]),
            vec![Step::Start(Mode::Paste), Step::Discard("the key was part of a shortcut")]
        );
        // A shortcut during hands-free recording doesn't stop it.
        let steps = run(&[
            Down(Key::Dictate),
            Up(Key::Dictate, Press::Tap),
            Down(Key::Dictate),
            Up(Key::Dictate, Press::Combo),
            Down(Key::Dictate),
            Up(Key::Dictate, Press::Tap),
        ]);
        assert_eq!(steps.last(), Some(&Step::Finish(Mode::Paste)));
        assert!(!steps[..5].iter().any(|s| matches!(s, Step::Finish(_) | Step::Discard(_))));
    }

    #[test]
    fn escape_cancels_either_way() {
        assert_eq!(
            run(&[Down(Key::Dictate), Cancel]),
            vec![Step::Start(Mode::Paste), Step::Discard("cancelled")]
        );
        assert_eq!(
            run(&[Down(Key::Note), Up(Key::Note, Press::Tap), Cancel]).last(),
            Some(&Step::Discard("cancelled"))
        );
        assert_eq!(run(&[Cancel]), vec![Step::Nothing], "Esc does nothing when not recording");
    }

    #[test]
    fn other_key_is_ignored_while_recording() {
        let steps = run(&[
            Down(Key::Dictate),
            Down(Key::Note),
            Up(Key::Note, Press::Hold),
            Up(Key::Dictate, Press::Hold),
        ]);
        assert_eq!(
            steps,
            vec![Step::Start(Mode::Paste), Step::Nothing, Step::Nothing, Step::Finish(Mode::Paste)]
        );
    }

    #[test]
    fn time_limit_finishes() {
        let mut t = Trigger::default();
        t.on_key(Down(Key::Note));
        t.on_key(Up(Key::Note, Press::Tap));
        assert_eq!(t.finish(), Step::Finish(Mode::Note));
        assert!(!t.is_active());
    }
}
