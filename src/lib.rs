//! whisperflow core: microphone capture, push-to-talk key, local transcription,
//! pasting into the focused app. Later steps (cleanup, storage, the Tauri app) build on these modules.

pub mod audio;
pub mod hotkey;
pub mod paste;
pub mod transcribe;
