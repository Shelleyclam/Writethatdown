//! Eval output: tables for the terminal and for `eval\RESULTS.md`, and a
//! per-case CSV to read side by side in Excel.

use super::{csv, CaseResult, Latency, Report, Summary};

fn percent(rate: f64) -> String {
    format!("{:.1}%", rate * 100.0)
}

fn latency(l: Option<Latency>) -> String {
    l.map(|l| format!("{:.0} / {:.0} ms", l.p50, l.p95)).unwrap_or_else(|| "-".into())
}

fn drift(s: &Summary) -> String {
    s.drift_flagged.map(|n| format!("{n} / {}", s.cases)).unwrap_or_else(|| "-".into())
}

/// A table as aligned text (`markdown: false`) or as a Markdown table.
fn table(headers: &[&str], rows: &[Vec<String>], markdown: bool) -> String {
    let widths: Vec<usize> = (0..headers.len())
        .map(|i| rows.iter().map(|r| r[i].chars().count()).chain([headers[i].chars().count()]).max().unwrap_or(0))
        .collect();
    let line = |cells: Vec<String>| {
        let padded: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let pad = " ".repeat(widths[i] - c.chars().count());
                // Text left, numbers right.
                if i == 0 { format!("{c}{pad}") } else { format!("{pad}{c}") }
            })
            .collect();
        if markdown { format!("| {} |\n", padded.join(" | ")) } else { format!("  {}\n", padded.join("   ")) }
    };
    let mut out = line(headers.iter().map(|h| h.to_string()).collect());
    if markdown {
        let rule: Vec<String> = widths
            .iter()
            .enumerate()
            .map(|(i, w)| if i == 0 { "-".repeat(*w) } else { format!("{}:", "-".repeat(w - 1)) })
            .collect();
        out.push_str(&format!("| {} |\n", rule.join(" | ")));
    }
    for r in rows {
        out.push_str(&line(r.clone()));
    }
    out
}

const SUMMARY_HEADERS: [&str; 8] =
    ["Group", "Cases", "Raw WER", "Final WER", "Drift", "Transcribe p50 / p95", "Cleanup p50 / p95", "Total p50 / p95"];

fn summary_rows(summary: &[Summary]) -> Vec<Vec<String>> {
    summary
        .iter()
        .map(|s| {
            vec![
                s.group.clone(),
                s.cases.to_string(),
                percent(s.raw_wer.rate()),
                percent(s.final_wer.rate()),
                drift(s),
                latency(s.transcribe_ms),
                latency(s.cleanup_ms),
                latency(s.total_ms),
            ]
        })
        .collect()
}

/// Variants side by side: the "all" row of each, then final WER per group.
fn comparison(report: &Report, markdown: bool) -> String {
    let overall: Vec<Vec<String>> = report
        .variants
        .iter()
        .map(|v| {
            let all = &v.summary[0];
            vec![v.variant.name.clone(), percent(all.final_wer.rate()), drift(all), latency(all.total_ms)]
        })
        .collect();
    let mut out = table(&["Variant", "Final WER", "Drift", "Total p50 / p95"], &overall, markdown);

    let mut headers = vec!["Final WER by group".to_string()];
    headers.extend(report.variants.iter().map(|v| v.variant.name.clone()));
    let rows: Vec<Vec<String>> = report.variants[0]
        .summary
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut row = vec![format!("{} ({})", s.group, s.cases)];
            row.extend(report.variants.iter().map(|v| percent(v.summary[i].final_wer.rate())));
            row
        })
        .collect();
    out.push('\n');
    let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
    out.push_str(&table(&headers, &rows, markdown));
    out
}

fn header_line(r: &Report) -> String {
    format!(
        "{} · Whisper {} ({} threads) · cleanup {} · {} cases, {:.1} min of audio · {}",
        r.generated_at,
        r.whisper_model,
        r.threads,
        r.cleanup_model.as_deref().unwrap_or("off"),
        r.cases,
        r.audio_secs / 60.0,
        r.cases_file
    )
}

/// For the terminal.
pub fn text(report: &Report) -> String {
    let mut out = format!("\n{}\n", header_line(report));
    for v in &report.variants {
        out.push_str(&format!("\n{}\n", v.variant.name));
        out.push_str(&table(&SUMMARY_HEADERS, &summary_rows(&v.summary), false));
    }
    if report.variants.len() > 1 {
        out.push_str("\nComparison\n");
        out.push_str(&comparison(report, false));
    }
    out
}

/// For `eval\RESULTS.md`.
pub fn markdown(report: &Report) -> String {
    let mut out = format!(
        "# Eval results\n\n{}\n\n\
         - **Raw WER**: word error rate of Whisper's output against what was said.\n\
         - **Final WER**: word error rate of the text that would be pasted against what was wanted.\n\
         - **Drift**: cases where cleanup added or dropped content words that the wanted text doesn't \
         explain (see `src/eval/metrics.rs`).\n\
         - **Times** are per dictation on the CPU, in milliseconds (p50 = median, p95 = 95th percentile; \
         with few cases, p95 is close to the slowest case).\n",
        header_line(report)
    );
    for v in &report.variants {
        out.push_str(&format!("\n## {}\n\n", v.variant.name));
        out.push_str(&table(&SUMMARY_HEADERS, &summary_rows(&v.summary), true));
    }
    if report.variants.len() > 1 {
        out.push_str("\n## Comparison\n\n");
        out.push_str(&comparison(report, true));
    }
    out
}

/// Every case with the expected and actual text next to each other. Starts
/// with a byte-order mark so Excel reads it as UTF-8.
pub fn cases_csv(results: &[CaseResult]) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(&csv::row(
        &[
            "file", "tags", "audio_s", "said", "raw", "raw_wer", "wanted", "final", "final_wer", "drift",
            "drift_words", "transcribe_ms", "cleanup_ms", "total_ms", "warning",
        ]
        .map(String::from),
    ));
    for r in results {
        let drift_words = r
            .drift
            .as_ref()
            .map(|d| {
                let mut parts: Vec<String> = d.dropped.iter().map(|w| format!("-{w}")).collect();
                parts.extend(d.added.iter().map(|w| format!("+{w}")));
                parts.join(" ")
            })
            .unwrap_or_default();
        out.push_str(&csv::row(&[
            r.file.clone(),
            r.tags.join(", "),
            format!("{:.1}", r.audio_secs),
            r.said.clone(),
            r.raw.clone(),
            percent(r.raw_wer.rate()),
            r.wanted.clone(),
            r.text.clone(),
            percent(r.final_wer.rate()),
            r.drift.as_ref().map(|d| if d.flagged { "yes" } else { "no" }).unwrap_or("-").to_string(),
            drift_words,
            format!("{:.0}", r.transcribe_ms),
            r.cleanup_ms.map(|ms| format!("{ms:.0}")).unwrap_or_default(),
            format!("{:.0}", r.total_ms),
            r.warning.clone().unwrap_or_default(),
        ]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_align_and_render_markdown() {
        let rows = vec![vec!["all".to_string(), "12".to_string()], vec!["names".to_string(), "3".to_string()]];
        assert_eq!(table(&["Group", "Cases"], &rows, false), "  Group   Cases\n  all        12\n  names       3\n");
        assert_eq!(
            table(&["Group", "Cases"], &rows, true),
            "| Group | Cases |\n| ----- | ----: |\n| all   |    12 |\n| names |     3 |\n"
        );
    }
}
