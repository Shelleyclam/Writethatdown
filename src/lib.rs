//! whisperflow core: microphone capture, push-to-talk key, local transcription,
//! LLM cleanup, pasting into the focused app, and history storage. `engine` ties
//! them into the dictation loop used by both the CLI and the desktop app.

pub mod audio;
pub mod cleanup;
pub mod engine;
pub mod hotkey;
pub mod paste;
pub mod storage;
pub mod transcribe;
