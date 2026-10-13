//! whisperflow core: microphone capture, push-to-talk key, local transcription.
//! Later steps (paste, cleanup, storage, the Tauri app) build on these modules.

pub mod audio;
pub mod hotkey;
pub mod transcribe;
