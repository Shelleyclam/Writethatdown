//! `whisperflow eval`: runs recorded cases through the real pipeline (without
//! pasting) and measures accuracy, meaning drift, and speed.
//!
//! The cases come from a spreadsheet saved as CSV (`eval\cases.csv`) with
//! one row per recording: the file, what was said (written down while
//! listening back), what should have been pasted, and free-form tags.

pub mod csv;
pub mod judge;
pub mod metrics;
pub mod report;

use crate::audio::{Clip, WHISPER_RATE};
use crate::cleanup::Cleaner;
use crate::engine::Mode;
use crate::pipeline;
use crate::transcribe::Transcriber;
use anyhow::{bail, Context, Result};
use metrics::{Drift, Wer};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// One row of the cases sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// The recording, relative to the cases folder ("001.wav").
    pub file: String,
    /// Exactly what was said, fillers and false starts included.
    pub said: String,
    /// What should have been pasted.
    pub wanted: String,
    pub tags: Vec<String>,
}

/// Cases with this tag go through the note path (Right Alt: long-text
/// cleanup, "file this under" removed) instead of the paste path.
pub const NOTE_TAG: &str = "note";

/// Length tags added to every case, so results can be split by length
/// without tagging it by hand.
const LENGTH_TAGS: [&str; 3] = ["len:short", "len:medium", "len:long"];

fn length_tag(secs: f32) -> &'static str {
    match secs {
        s if s < 10.0 => LENGTH_TAGS[0],
        s if s <= 30.0 => LENGTH_TAGS[1],
        _ => LENGTH_TAGS[2],
    }
}

pub struct Loaded {
    pub cases: Vec<Case>,
    /// Rows left out, and why.
    pub skipped: Vec<String>,
}

/// Read the cases sheet. Columns are found by their header (any order,
/// extra columns ignored): `file`, `said`, `wanted`, and optionally `tags`.
/// Text in brackets is ignored, so "said (verbatim)" works too.
pub fn load_cases(path: &Path) -> Result<Loaded> {
    let bytes = std::fs::read(path).with_context(|| format!("Could not read {}", path.display()))?;
    let rows = csv::parse(&csv::decode(bytes));
    let Some((header, rows)) = rows.split_first() else { bail!("{} is empty", path.display()) };
    let column = |names: &[&str]| {
        header.iter().position(|h| {
            let h = h.split('(').next().unwrap_or("").trim().to_lowercase();
            names.contains(&h.as_str())
        })
    };
    let file = column(&["file", "audio", "file name", "filename"]).context("The sheet needs a 'file' column")?;
    let said = column(&["said", "reference", "reference_transcript"]).context("The sheet needs a 'said' column")?;
    let wanted = column(&["wanted", "intended", "intended_text"]).context("The sheet needs a 'wanted' column")?;
    let tags = column(&["tags", "tag"]);

    let mut cases: Vec<Case> = Vec::new();
    let mut skipped = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let get = |c: usize| row.get(c).map(|s| s.trim().to_string()).unwrap_or_default();
        let mut case = Case {
            file: get(file),
            said: get(said),
            wanted: get(wanted),
            tags: tags.map(|c| split_tags(&get(c))).unwrap_or_default(),
        };
        if case.file.is_empty() {
            skipped.push(format!("row {}: no file name", i + 2));
            continue;
        }
        if Path::new(&case.file).extension().is_none() {
            // Excel turns "001" into 1, so a plain number means the recorded clip.
            case.file = match case.file.parse::<u32>() {
                Ok(n) => format!("{n:03}.wav"),
                Err(_) => format!("{}.wav", case.file),
            };
        }
        if case.said.is_empty() || case.wanted.is_empty() {
            skipped.push(format!("{}: 'said' or 'wanted' is empty", case.file));
            continue;
        }
        if cases.iter().any(|c| c.file.eq_ignore_ascii_case(&case.file)) {
            bail!("{} appears twice in {}", case.file, path.display());
        }
        cases.push(case);
    }
    Ok(Loaded { cases, skipped })
}

/// "Names, accent:indian; filler" -> ["names", "accent:indian", "filler"]
fn split_tags(cell: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in cell.split([',', ';']).map(|t| t.trim().to_lowercase()) {
        if !tag.is_empty() && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

/// WAV files in the cases folder that no row mentions.
pub fn recordings_without_rows(dir: &Path, cases: &[Case]) -> Vec<String> {
    let listed: HashSet<String> = cases.iter().map(|c| c.file.to_lowercase()).collect();
    let mut unlisted: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_lowercase().ends_with(".wav") && !listed.contains(&n.to_lowercase()))
        .collect();
    unlisted.sort();
    unlisted
}

/// One way of running the pipeline, e.g. with and without cleanup.
#[derive(Debug, Clone, Serialize)]
pub struct Variant {
    pub name: String,
    pub cleanup: bool,
}

/// What one case produced under one variant.
#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    pub file: String,
    /// The case's tags plus its length tag.
    pub tags: Vec<String>,
    pub audio_secs: f32,
    pub said: String,
    /// Whisper's output.
    pub raw: String,
    /// `raw` against `said`.
    pub raw_wer: Wer,
    pub wanted: String,
    /// What would have been pasted.
    pub text: String,
    /// `text` against `wanted`.
    pub final_wer: Wer,
    /// `None` when cleanup didn't run.
    pub drift: Option<Drift>,
    pub transcribe_ms: f64,
    pub cleanup_ms: Option<f64>,
    pub total_ms: f64,
    /// Why cleanup kept the raw text, if it did.
    pub warning: Option<String>,
    /// The judge model's verdict (`--judge`, cleanup variants only).
    pub judge: Option<judge::Verdict>,
}

pub fn audio_path(dir: &Path, case: &Case) -> PathBuf {
    dir.join(&case.file)
}

/// Run every case under every variant. Each recording is transcribed once
/// and shared by the variants. `progress` is called after each case with
/// its results (one per variant). Returns the results per variant.
pub fn run(
    cases: &[Case],
    dir: &Path,
    transcriber: &Transcriber,
    cleaner: Option<&Cleaner>,
    variants: &[Variant],
    mut progress: impl FnMut(usize, &[CaseResult]),
) -> Result<Vec<Vec<CaseResult>>> {
    let missing: Vec<&str> =
        cases.iter().filter(|c| !audio_path(dir, c).is_file()).map(|c| c.file.as_str()).collect();
    if !missing.is_empty() {
        bail!("Missing recordings in {}: {}", dir.display(), missing.join(", "));
    }
    if variants.iter().any(|v| v.cleanup) && cleaner.is_none() {
        bail!("A variant needs cleanup but no cleanup model is loaded");
    }

    // The first transcription is slower (caches warming up), so don't time it.
    transcriber.transcribe(&Clip { samples: vec![0.0; WHISPER_RATE as usize] })?;

    let mut results: Vec<Vec<CaseResult>> = vec![Vec::new(); variants.len()];
    for (i, case) in cases.iter().enumerate() {
        let path = audio_path(dir, case);
        let clip = Clip::from_wav(&path)?;
        let started = Instant::now();
        let raw = transcriber.transcribe(&clip).with_context(|| format!("Transcribing {}", case.file))?;
        let transcribe_ms = started.elapsed().as_secs_f64() * 1000.0;

        let audio_secs = clip.duration_secs();
        let mut tags = case.tags.clone();
        tags.push(length_tag(audio_secs).to_string());
        let mode = if case.tags.iter().any(|t| t == NOTE_TAG) { Mode::Note } else { Mode::Paste };

        for (v, variant) in variants.iter().enumerate() {
            let c = if variant.cleanup { cleaner } else { None };
            let out = pipeline::process(&raw, mode, c, || {});
            let cleanup_ms = out.cleanup_secs.map(|s| s as f64 * 1000.0);
            results[v].push(CaseResult {
                file: case.file.clone(),
                tags: tags.clone(),
                audio_secs,
                said: case.said.clone(),
                raw: raw.clone(),
                raw_wer: metrics::wer(&case.said, &raw),
                wanted: case.wanted.clone(),
                final_wer: metrics::wer(&case.wanted, &out.text),
                drift: c.map(|_| metrics::drift(&raw, &out.text, &case.wanted)),
                text: out.text,
                transcribe_ms,
                cleanup_ms,
                total_ms: transcribe_ms + cleanup_ms.unwrap_or(0.0),
                warning: out.warning,
                judge: None,
            });
        }
        let latest: Vec<CaseResult> = results.iter().map(|r| r.last().unwrap().clone()).collect();
        progress(i, &latest);
    }
    Ok(results)
}

/// Ask the judge about every case that went through cleanup. A case the
/// judge can't answer keeps no verdict; `progress` gets each one's error.
pub fn judge_all(
    results: &mut [Vec<CaseResult>],
    variants: &[Variant],
    judge: &judge::Judge,
    mut progress: impl FnMut(&CaseResult, Result<&judge::Verdict, &anyhow::Error>),
) {
    for (rs, _) in results.iter_mut().zip(variants).filter(|(_, v)| v.cleanup) {
        for r in rs.iter_mut() {
            match judge.judge(&r.raw, &r.wanted, &r.text) {
                Ok(v) => {
                    r.judge = Some(v);
                    progress(r, Ok(r.judge.as_ref().unwrap()));
                }
                Err(e) => progress(r, Err(&e)),
            }
        }
    }
}

/// p50 and p95 in milliseconds.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Latency {
    pub p50: f64,
    pub p95: f64,
}

impl Latency {
    fn of(values: &[f64]) -> Option<Self> {
        Some(Self { p50: metrics::percentile(values, 50.0)?, p95: metrics::percentile(values, 95.0)? })
    }
}

/// Totals for a group of cases ("all", or everything with one tag).
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub group: String,
    pub cases: usize,
    pub audio_secs: f32,
    pub raw_wer: Wer,
    pub final_wer: Wer,
    /// Cases flagged for meaning drift; `None` when cleanup didn't run.
    pub drift_flagged: Option<usize>,
    /// Cases the judge said lost meaning, and how many it judged.
    pub judge_failed: Option<(usize, usize)>,
    pub transcribe_ms: Option<Latency>,
    pub cleanup_ms: Option<Latency>,
    pub total_ms: Option<Latency>,
}

/// One summary for all cases, then one per tag (your tags first, then the
/// length tags from short to long).
pub fn summarize(results: &[CaseResult]) -> Vec<Summary> {
    let mut tags: Vec<&str> = results
        .iter()
        .flat_map(|r| r.tags.iter().map(String::as_str))
        .filter(|t| !LENGTH_TAGS.contains(t))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    tags.extend(LENGTH_TAGS.iter().filter(|t| results.iter().any(|r| r.tags.iter().any(|x| x == *t))));

    let mut out = vec![summary("all", results.iter().collect())];
    for tag in tags {
        out.push(summary(tag, results.iter().filter(|r| r.tags.iter().any(|t| t == tag)).collect()));
    }
    out
}

fn summary(group: &str, rs: Vec<&CaseResult>) -> Summary {
    let cleaned: Vec<f64> = rs.iter().filter_map(|r| r.cleanup_ms).collect();
    let drifts: Vec<&Drift> = rs.iter().filter_map(|r| r.drift.as_ref()).collect();
    let verdicts: Vec<&judge::Verdict> = rs.iter().filter_map(|r| r.judge.as_ref()).collect();
    Summary {
        group: group.to_string(),
        cases: rs.len(),
        audio_secs: rs.iter().map(|r| r.audio_secs).sum(),
        raw_wer: rs.iter().map(|r| r.raw_wer).sum(),
        final_wer: rs.iter().map(|r| r.final_wer).sum(),
        drift_flagged: (!drifts.is_empty()).then(|| drifts.iter().filter(|d| d.flagged).count()),
        judge_failed: (!verdicts.is_empty())
            .then(|| (verdicts.iter().filter(|v| !v.preserved).count(), verdicts.len())),
        transcribe_ms: Latency::of(&rs.iter().map(|r| r.transcribe_ms).collect::<Vec<_>>()),
        cleanup_ms: Latency::of(&cleaned),
        total_ms: Latency::of(&rs.iter().map(|r| r.total_ms).collect::<Vec<_>>()),
    }
}

/// Everything about one eval run, as saved to the results JSON.
#[derive(Debug, Serialize)]
pub struct Report {
    pub generated_at: String,
    pub whisper_model: String,
    pub threads: i32,
    pub cleanup_model: Option<String>,
    pub cases_file: String,
    pub cases: usize,
    pub audio_secs: f32,
    pub variants: Vec<VariantReport>,
}

#[derive(Debug, Serialize)]
pub struct VariantReport {
    pub variant: Variant,
    pub summary: Vec<Summary>,
    pub cases: Vec<CaseResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_cases_by_header() {
        let dir = std::env::temp_dir().join(format!("wf-eval-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cases.csv");
        std::fs::write(
            &path,
            "Tags,File,Said (verbatim),Wanted,Notes\n\
             \"Names, filler\",001.wav,um send it to shelly,Send it to Shelley.,first try\n\
             ,2,hello there,Hello there.,\n\
             note,003.wav,still recording,,\n",
        )
        .unwrap();
        let loaded = load_cases(&path).unwrap();
        assert_eq!(loaded.cases.len(), 2);
        assert_eq!(loaded.cases[0].tags, vec!["names", "filler"]);
        assert_eq!(loaded.cases[1].file, "002.wav", "Excel's 2 means 002.wav");
        assert_eq!(loaded.skipped, vec!["003.wav: 'said' or 'wanted' is empty"]);

        std::fs::write(&path, "file,said,wanted\n001.wav,a,b\n001.WAV,c,d\n").unwrap();
        assert!(load_cases(&path).is_err(), "duplicate files");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn result(tags: &[&str], raw_errors: usize, drift: Option<bool>, total_ms: f64) -> CaseResult {
        CaseResult {
            file: "x.wav".into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            audio_secs: 2.0,
            said: String::new(),
            raw: String::new(),
            raw_wer: Wer { errors: raw_errors, words: 10 },
            wanted: String::new(),
            text: String::new(),
            final_wer: Wer { errors: 0, words: 10 },
            drift: drift.map(|flagged| Drift { flagged, ..Default::default() }),
            transcribe_ms: total_ms,
            cleanup_ms: None,
            total_ms,
            warning: None,
            judge: None,
        }
    }

    #[test]
    fn summaries_per_tag() {
        let rs = vec![
            result(&["names", "len:short"], 2, Some(true), 100.0),
            result(&["filler", "len:long"], 0, Some(false), 300.0),
            result(&["names", "len:short"], 1, Some(false), 200.0),
        ];
        let s = summarize(&rs);
        let groups: Vec<&str> = s.iter().map(|g| g.group.as_str()).collect();
        assert_eq!(groups, vec!["all", "filler", "names", "len:short", "len:long"]);
        assert_eq!(s[0].raw_wer, Wer { errors: 3, words: 30 });
        assert_eq!(s[0].drift_flagged, Some(1));
        assert_eq!(s[0].total_ms.unwrap().p50, 200.0);
        assert_eq!(s[2].cases, 2);
        assert!(s[0].cleanup_ms.is_none());
        let no_cleanup = summarize(&[result(&["len:short"], 0, None, 1.0)]);
        assert_eq!(no_cleanup[0].drift_flagged, None);
    }

    #[test]
    fn length_tags() {
        assert_eq!(length_tag(2.5), "len:short");
        assert_eq!(length_tag(30.0), "len:medium");
        assert_eq!(length_tag(95.0), "len:long");
    }
}
