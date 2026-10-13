//! Step 1 command-line tool.
//!
//!   whisperflow                     push-to-talk: hold Right Ctrl, speak, release
//!   whisperflow bench [clip.wav]    compare every downloaded model on one clip
//!   whisperflow file <clip.wav>     transcribe a WAV file
//!
//! Options:
//!   --model <name>    e.g. base.en or small.en (default: base.en if present)
//!   --threads <n>     CPU threads for Whisper (default: up to 8)
//!   --save-wav        keep each recording in recordings\ (for debugging)

use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;
use whisperflow::audio::{Clip, Recorder};
use whisperflow::hotkey::{self, KeyEvent};
use whisperflow::transcribe::{available_models, find_model, model_name, Transcriber};

const HELP: &str = "\
Usage:
  whisperflow                     push-to-talk: hold Right Ctrl, speak, release
  whisperflow bench [clip.wav]    compare every downloaded model on one clip
  whisperflow file <clip.wav>     transcribe a WAV file

Options:
  --model <name>    e.g. base.en or small.en (default: base.en if present)
  --threads <n>     CPU threads for Whisper (default: up to 8)
  --save-wav        keep each recording in recordings\\ (for debugging)";

/// Presses shorter than this are treated as accidental taps.
const MIN_CLIP_SECS: f32 = 0.3;
/// Below this loudness the clip is treated as silence (avoids Whisper
/// "hallucinating" text like "Thank you." from nothing).
const SILENCE_RMS: f32 = 0.002;

struct Options {
    command: String,
    file: Option<PathBuf>,
    model: Option<String>,
    threads: i32,
    save_wav: bool,
}

fn parse_args() -> Result<Options> {
    let mut args = std::env::args().skip(1);
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut o = Options {
        command: "run".into(),
        file: None,
        model: None,
        threads: cores.min(8) as i32,
        save_wav: false,
    };
    let mut positional = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--model" => o.model = Some(args.next().ok_or_else(|| anyhow!("--model needs a value"))?),
            "--threads" => {
                o.threads = args
                    .next()
                    .ok_or_else(|| anyhow!("--threads needs a value"))?
                    .parse()?
            }
            "--save-wav" => o.save_wav = true,
            "-h" | "--help" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            other if other.starts_with("--") => bail!("Unknown option {other}"),
            other => positional.push(other.to_string()),
        }
    }
    if let Some(cmd) = positional.first() {
        o.command = cmd.clone();
    }
    o.file = positional.get(1).map(PathBuf::from);
    Ok(o)
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let o = parse_args()?;
    match o.command.as_str() {
        "run" => run(&o),
        "bench" => bench(&o),
        "file" => {
            let path = o.file.as_deref().ok_or_else(|| anyhow!("Usage: whisperflow file <clip.wav>"))?;
            let t = load(&find_model(o.model.as_deref())?, o.threads)?;
            let clip = Clip::from_wav(path)?;
            report(&t, &clip)?;
            Ok(())
        }
        other => bail!("Unknown command '{other}'. Try: run, bench, file"),
    }
}

fn load(path: &Path, threads: i32) -> Result<Transcriber> {
    let started = Instant::now();
    let t = Transcriber::load(path, threads)?;
    println!(
        "Loaded {} in {:.1}s ({} threads)",
        t.name,
        started.elapsed().as_secs_f32(),
        threads
    );
    Ok(t)
}

/// Transcribe and print the text with timing. Returns seconds taken.
fn report(t: &Transcriber, clip: &Clip) -> Result<f32> {
    let started = Instant::now();
    let text = t.transcribe(clip)?;
    let took = started.elapsed().as_secs_f32();
    let audio = clip.duration_secs();
    if text.is_empty() {
        println!("  (no speech detected)");
    } else {
        println!("  \"{text}\"");
    }
    println!(
        "  {:.1}s of audio -> {:.2}s to transcribe  (speed factor {:.2}, lower is faster)",
        audio,
        took,
        took / audio.max(0.01)
    );
    Ok(took)
}

/// Wait for one press-and-release of the push-to-talk key and return the clip.
fn record_one(
    rec: &mut Recorder,
    keys: &std::sync::mpsc::Receiver<KeyEvent>,
) -> Result<Option<Clip>> {
    loop {
        match keys.recv()? {
            KeyEvent::Down if !rec.is_recording() => {
                rec.start()?;
                println!("\n● Listening...");
            }
            KeyEvent::Up if rec.is_recording() => {
                let clip = rec.stop();
                if clip.duration_secs() < MIN_CLIP_SECS {
                    println!("  (too short, ignored)");
                    return Ok(None);
                }
                if clip.rms() < SILENCE_RMS {
                    println!("  (only silence, ignored. Is the right microphone selected?)");
                    return Ok(None);
                }
                return Ok(Some(clip));
            }
            _ => {}
        }
    }
}

fn save_debug_wav(clip: &Clip) -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let path = PathBuf::from("recordings").join(format!("clip-{stamp}.wav"));
    clip.save_wav(&path)?;
    println!("  saved {}", path.display());
    Ok(())
}

fn run(o: &Options) -> Result<()> {
    let t = load(&find_model(o.model.as_deref())?, o.threads)?;
    let mut rec = Recorder::new()?;
    let keys = hotkey::listen()?;
    println!("Microphone: {}", rec.device_name());
    println!("{}  Ctrl+C to quit.", hotkey::describe());

    loop {
        let Some(clip) = record_one(&mut rec, &keys)? else { continue };
        if o.save_wav {
            save_debug_wav(&clip)?;
        }
        if let Err(e) = report(&t, &clip) {
            eprintln!("  {e:#}");
        }
    }
}

fn bench(o: &Options) -> Result<()> {
    let models = available_models();
    if models.is_empty() {
        // Produces the helpful "run download-models.ps1" error.
        find_model(None)?;
    }
    println!("Models found: {}", models.iter().map(|m| model_name(m)).collect::<Vec<_>>().join(", "));

    let clip = match &o.file {
        Some(path) => Clip::from_wav(path)?,
        None => {
            let mut rec = Recorder::new()?;
            let keys = hotkey::listen()?;
            println!("Microphone: {}", rec.device_name());
            println!("{} Speak for 10-15 seconds, the way you normally would.", hotkey::describe());
            loop {
                if let Some(c) = record_one(&mut rec, &keys)? {
                    break c;
                }
            }
        }
    };
    if o.save_wav {
        save_debug_wav(&clip)?;
    }

    let mut results = Vec::new();
    for path in &models {
        println!();
        let t = load(path, o.threads)?;
        // The first run warms up caches; time the second.
        t.transcribe(&clip)?;
        let took = report(&t, &clip)?;
        results.push((t.name.clone(), took / clip.duration_secs()));
    }

    println!("\nSummary (speed factor: 0.3 means 10 s of speech takes 3 s):");
    for (name, rtf) in &results {
        println!("  {name:<12} {rtf:.2}");
    }
    // Biggest model that keeps 10 s of speech under ~3 s of waiting.
    if let Some((name, _)) = results.iter().rev().find(|(_, rtf)| *rtf <= 0.3) {
        println!("\nRecommended default: {name}");
    } else {
        println!("\nNone are under 0.3. Use the fastest one, or try --threads with a different value.");
    }
    Ok(())
}
