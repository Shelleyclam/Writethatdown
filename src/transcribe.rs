//! Local speech-to-text with whisper.cpp.

use crate::audio::{Clip, WHISPER_RATE};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// A Whisper model loaded into memory. Load once at startup and reuse it,
/// so each dictation only pays for transcription, not model loading.
pub struct Transcriber {
    ctx: WhisperContext,
    threads: i32,
    pub name: String,
}

impl Transcriber {
    pub fn load(path: &Path, threads: i32) -> Result<Self> {
        // Route whisper.cpp's console chatter away from our output.
        whisper_rs::install_logging_hooks();
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| anyhow!("Failed to load model {}: {e}", path.display()))?;
        Ok(Self { ctx, threads, name: model_name(path) })
    }

    pub fn transcribe(&self, clip: &Clip) -> Result<String> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow!("Could not create Whisper state: {e}"))?;

        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_n_threads(self.threads);
        p.set_language(Some("en"));
        p.set_print_special(false);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_timestamps(false);
        p.set_suppress_blank(true);
        p.set_no_context(true);
        p.set_no_speech_thold(0.6);

        // Whisper misbehaves on clips under ~1 s, so pad short ones with silence.
        let mut audio = clip.samples.clone();
        let min_len = WHISPER_RATE as usize * 11 / 10;
        if audio.len() < min_len {
            audio.resize(min_len, 0.0);
        }

        state
            .full(p, &audio)
            .map_err(|e| anyhow!("Transcription failed: {e}"))?;

        let mut text = String::new();
        for i in 0..state.full_n_segments() {
            let Some(seg) = state.get_segment(i) else { continue };
            let s = seg.to_str_lossy().map_err(|e| anyhow!("{e}"))?;
            if !is_non_speech_marker(&s) {
                text.push_str(&s);
            }
        }
        Ok(text.trim().to_string())
    }
}

/// Whisper emits markers like "[BLANK_AUDIO]" or "(wind blowing)" for non-speech.
fn is_non_speech_marker(segment: &str) -> bool {
    let t = segment.trim();
    t.is_empty()
        || (t.starts_with('[') && t.ends_with(']'))
        || (t.starts_with('(') && t.ends_with(')'))
}

/// "ggml-small.en.bin" -> "small.en"
pub fn model_name(path: &Path) -> String {
    let stem = path.file_name().and_then(|s| s.to_str()).unwrap_or("model");
    stem.trim_start_matches("ggml-").trim_end_matches(".bin").to_string()
}

/// Where model files live: a `models` folder next to the program, or in the
/// current directory (handy when running with `cargo run`).
pub fn models_dir() -> PathBuf {
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("models")));
    match beside_exe {
        Some(dir) if dir.is_dir() => dir,
        _ => PathBuf::from("models"),
    }
}

/// All `ggml-*.bin` models found, smallest first.
pub fn available_models() -> Vec<PathBuf> {
    let mut found: Vec<(u64, PathBuf)> = std::fs::read_dir(models_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            name.starts_with("ggml-") && name.ends_with(".bin") && !name.contains("silero")
        })
        .map(|p| (std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0), p))
        .collect();
    found.sort();
    found.into_iter().map(|(_, p)| p).collect()
}

/// Resolve a model by short name ("small.en") or file path. With no name,
/// prefer small.en, then base.en, then whatever is there.
pub fn find_model(name: Option<&str>) -> Result<PathBuf> {
    if let Some(n) = name {
        let as_path = PathBuf::from(n);
        if as_path.is_file() {
            return Ok(as_path);
        }
        let p = models_dir().join(format!("ggml-{n}.bin"));
        return if p.is_file() {
            Ok(p)
        } else {
            Err(anyhow!("Model '{n}' not found at {}", p.display()))
        };
    }
    let all = available_models();
    for preferred in ["small.en", "base.en"] {
        if let Some(p) = all.iter().find(|p| model_name(p) == preferred) {
            return Ok(p.clone());
        }
    }
    all.into_iter().next().ok_or_else(|| {
        anyhow!(
            "No Whisper models in {}. Run scripts\\download-models.ps1 first.",
            models_dir().display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(model_name(Path::new("models/ggml-small.en.bin")), "small.en");
    }

    #[test]
    fn markers() {
        assert!(is_non_speech_marker(" [BLANK_AUDIO]"));
        assert!(is_non_speech_marker("(music)"));
        assert!(!is_non_speech_marker(" Hello there."));
    }
}
