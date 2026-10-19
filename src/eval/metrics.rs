//! Scoring: word error rate, meaning drift, and latency percentiles.

use serde::ser::{Serialize, SerializeStruct, Serializer};
use std::collections::BTreeSet;

/// Words for scoring: lowercase, punctuation removed, curly apostrophes
/// straightened (Excel turns ' into ’ as you type), and number words as
/// digits, because "said" is written the way it was spoken ("three thirty")
/// while Whisper writes digits ("3:30").
/// "Don't," -> "don't", "3:30" -> "3", "30", "twenty-five" -> "25".
pub fn words(text: &str) -> Vec<String> {
    let words = text
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '/' | ':' | '–' | '—'))
        .map(|w| {
            w.chars()
                .map(|c| if matches!(c, '’' | '‘') { '\'' } else { c })
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .collect::<String>()
                .trim_matches('\'')
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    numbers_to_digits(words)
}

const UNITS: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
    "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
];
const TENS: [&str; 8] = ["twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

/// "twenty five" -> "25", "three" -> "3". Up to 99, which covers times,
/// dates, and counts in dictation; bigger numbers are rare enough to leave.
fn numbers_to_digits(words: Vec<String>) -> Vec<String> {
    let unit = |w: &str| UNITS.iter().position(|u| *u == w);
    let mut out = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        if let Some(t) = TENS.iter().position(|x| *x == w) {
            let tens = (t + 2) * 10;
            match words.get(i + 1).and_then(|n| unit(n)).filter(|u| (1..10).contains(u)) {
                Some(u) => {
                    out.push((tens + u).to_string());
                    i += 1;
                }
                None => out.push(tens.to_string()),
            }
        } else if let Some(u) = unit(w) {
            out.push(u.to_string());
        } else {
            out.push(words[i].clone());
        }
        i += 1;
    }
    out
}

/// Word error rate as counts, so cases can be added up: the rate over a
/// group is total errors / total reference words, not an average of rates
/// (which would let a 3-word clip weigh as much as a 3-minute one).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Wer {
    /// Substitutions + deletions + insertions.
    pub errors: usize,
    /// Words in the reference.
    pub words: usize,
}

impl Wer {
    pub fn rate(&self) -> f64 {
        match self.words {
            0 if self.errors == 0 => 0.0,
            0 => 1.0,
            n => self.errors as f64 / n as f64,
        }
    }
}

impl std::ops::Add for Wer {
    type Output = Wer;
    fn add(self, other: Wer) -> Wer {
        Wer { errors: self.errors + other.errors, words: self.words + other.words }
    }
}

impl std::iter::Sum for Wer {
    fn sum<I: Iterator<Item = Wer>>(iter: I) -> Wer {
        iter.fold(Wer::default(), |a, b| a + b)
    }
}

impl Serialize for Wer {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Wer", 3)?;
        st.serialize_field("rate", &self.rate())?;
        st.serialize_field("errors", &self.errors)?;
        st.serialize_field("words", &self.words)?;
        st.end()
    }
}

/// Word error rate of `hypothesis` against `reference`.
pub fn wer(reference: &str, hypothesis: &str) -> Wer {
    let r = words(reference);
    let h = words(hypothesis);
    Wer { errors: edit_distance(&r, &h), words: r.len() }
}

/// Levenshtein distance between two word sequences.
fn edit_distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let substitute = prev[j] + usize::from(x != y);
            cur[j + 1] = substitute.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Words that carry little meaning on their own. Cleanup adding or dropping
/// these isn't drift. Negations ("no", "not", "never") and numbers are
/// deliberately not here: losing them changes the meaning.
const FUNCTION_WORDS: &[&str] = &[
    "um", "uh", "er", "ah", "hmm", "mm", "oh", "a", "an", "the", "and", "or", "but", "so", "to", "of",
    "in", "on", "at", "for", "with", "from", "by", "as", "is", "are", "was", "were", "be", "been",
    "am", "it", "it's", "its", "this", "that", "these", "those", "i", "i'm", "me", "my", "we",
    "our", "you", "your", "he", "she", "they", "them", "his", "her", "their", "do", "does", "did",
    "have", "has", "had", "will", "would", "can", "could", "should", "just", "like", "okay", "ok",
    "yeah", "well", "actually", "basically", "really", "kind", "sort", "know", "mean", "then",
];

/// Cleanup changes flagged as drift: more than this many content words, or
/// more than [`DRIFT_MAX_SHARE`] of them, whichever is smaller.
pub const DRIFT_MIN_WORDS: usize = 2;
pub const DRIFT_MAX_SHARE: f64 = 0.10;

/// Content words that cleanup added or dropped without the wanted text
/// explaining it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Drift {
    /// Said and wanted, but missing after cleanup.
    pub dropped: Vec<String>,
    /// Not said and not wanted, but present after cleanup.
    pub added: Vec<String>,
    /// Content words in the cleanup input.
    pub content_words: usize,
    pub flagged: bool,
}

fn content_words(text: &str) -> BTreeSet<String> {
    words(text).into_iter().filter(|w| !FUNCTION_WORDS.contains(&w.as_str())).collect()
}

/// Compare cleanup's input with its output. Changes that the wanted text
/// explains are expected: dropping "Tuesday" from "Tuesday, no wait,
/// Wednesday" is the point of cleanup, and so is "three" becoming "3" when
/// you wanted "3". What's left is cleanup changing what you meant.
pub fn drift(input: &str, output: &str, wanted: &str) -> Drift {
    let before = content_words(input);
    let after = content_words(output);
    let wanted = content_words(wanted);
    let dropped: Vec<String> = before.difference(&after).filter(|w| wanted.contains(*w)).cloned().collect();
    let added: Vec<String> = after.difference(&before).filter(|w| !wanted.contains(*w)).cloned().collect();
    let changed = dropped.len() + added.len();
    let flagged = changed >= DRIFT_MIN_WORDS || changed as f64 > before.len() as f64 * DRIFT_MAX_SHARE;
    Drift { dropped, added, content_words: before.len(), flagged }
}

/// Nearest-rank percentile (`p` in 0..=100). With few values, p95 is
/// simply the slowest case.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_words() {
        assert_eq!(words("Don’t, STOP! e-mail"), vec!["don't", "stop", "e", "mail"]);
        assert!(words(" ... -- ").is_empty());
    }

    #[test]
    fn spoken_numbers_match_digits() {
        assert_eq!(words("at three thirty"), words("at 3:30"));
        assert_eq!(words("twenty-five people, twenty people"), vec!["25", "people", "20", "people"]);
        assert_eq!(words("ninety nine"), vec!["99"]);
        assert_eq!(wer("Wednesday at three.", "Wednesday at 3.").errors, 0);
    }

    #[test]
    fn wer_counts_each_kind_of_error() {
        assert_eq!(wer("the cat sat", "the cat sat"), Wer { errors: 0, words: 3 });
        assert_eq!(wer("the cat sat", "the bat sat").errors, 1, "substitution");
        assert_eq!(wer("the cat sat", "the sat").errors, 1, "deletion");
        assert_eq!(wer("the cat sat", "the cat sat down").errors, 1, "insertion");
        assert_eq!(wer("Send it to Shelley.", "send it to shelly").errors, 1, "case and punctuation ignored");
        assert_eq!(wer("", "").rate(), 0.0);
        assert_eq!(wer("", "hello").rate(), 1.0);
    }

    #[test]
    fn group_wer_weighs_by_words() {
        let total: Wer = [Wer { errors: 1, words: 2 }, Wer { errors: 0, words: 98 }].into_iter().sum();
        assert_eq!(total.rate(), 0.01);
    }

    #[test]
    fn expected_cleanup_changes_are_not_drift() {
        let d = drift(
            "Um, so we meet on Tuesday. No, wait, Wednesday at three.",
            "We meet on Wednesday at 3.",
            "We meet on Wednesday at 3.",
        );
        assert_eq!(d, Drift { dropped: vec![], added: vec![], content_words: 6, flagged: false });
    }

    #[test]
    fn dropped_and_invented_words_are_drift() {
        let d = drift(
            "I don't want to push the launch to April",
            "I want to push the launch to May",
            "I don't want to push the launch to April.",
        );
        assert_eq!(d.dropped, vec!["april", "don't"]);
        assert_eq!(d.added, vec!["may"]);
        assert!(d.flagged);
    }

    #[test]
    fn one_change_in_a_long_text_is_tolerated() {
        let said = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen";
        let d = drift(said, &said.replace("seven", "heaven"), said);
        assert_eq!(d.dropped.len() + d.added.len(), 2, "seven dropped, heaven added");
        assert!(d.flagged, "two changed words reach the minimum");
        let d = drift(said, &said.replace(" seven", ""), said);
        assert!(!d.flagged, "one of fifteen is under 10%");
        let short = drift("call mom tonight", "call tonight", "call mom tonight");
        assert!(short.flagged, "one of three is over 10%");
    }

    #[test]
    fn percentiles() {
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), Some(10.0));
        assert_eq!(percentile(&v, 95.0), Some(19.0));
        assert_eq!(percentile(&[5.0, 1.0, 3.0], 95.0), Some(5.0));
        assert_eq!(percentile(&[], 50.0), None);
    }
}
