//! whisperflow core: microphone capture, push-to-talk key, local transcription,
//! LLM cleanup, pasting into the focused app, history and notes storage, note
//! filing and search, and Q&A over notes. `engine` ties them into the dictation
//! loop used by both the CLI and the desktop app.

pub mod ask;
pub mod audio;
pub mod cleanup;
pub mod engine;
pub mod hotkey;
pub mod notes;
pub mod ollama;
pub mod paste;
pub mod storage;
pub mod transcribe;
