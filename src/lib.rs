//! whisperflow core: microphone capture, push-to-talk key, local transcription,
//! LLM cleanup, and pasting into the focused app. Later steps (storage, the Tauri
//! app) build on these modules.

pub mod audio;
pub mod cleanup;
pub mod hotkey;
pub mod paste;
pub mod transcribe;
