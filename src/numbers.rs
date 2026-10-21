//! Numbers in text, whether spoken ("four thirty") or written ("4:30",
//! "4.30"). Shared by the cleanup checks and the eval scoring, so both treat
//! the same things as the same number.

use std::collections::BTreeSet;

const UNITS: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
    "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
];
const TENS: [&str; 8] = ["twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

/// Number words as digits: "twenty five" -> "25", "three" -> "3". Expects
/// lowercase words without punctuation. Up to 99, which covers times, dates,
/// and counts in dictation; bigger numbers are rare enough to leave.
pub fn spoken_to_digits(words: Vec<String>) -> Vec<String> {
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

/// Every number in `text`, each as its digit groups without leading zeros:
/// "4:30" and "4.30" give ["4", "30"], "530" gives ["530"], "29th" gives
/// ["29"], and "four thirty" gives ["4"] and ["30"].
fn numbers(text: &str) -> Vec<Vec<String>> {
    let groups = |chunk: &str| -> Vec<String> {
        chunk
            .split(|c: char| !c.is_ascii_digit())
            .filter(|d| !d.is_empty())
            .map(|d| match d.trim_start_matches('0') {
                "" => "0".to_string(),
                trimmed => trimmed.to_string(),
            })
            .collect()
    };
    let mut out = Vec::new();
    let mut spoken: Vec<String> = Vec::new();
    let flush_spoken =|spoken: &mut Vec<String>, out: &mut Vec<Vec<String>>| {
        for w in spoken_to_digits(std::mem::take(spoken)) {
            if w.chars().all(|c| c.is_ascii_digit()) {
                out.push(groups(&w));
            }
        }
    };
    for chunk in text.split(|c: char| c.is_whitespace() || matches!(c, '-' | '/' | '–' | '—')) {
        if chunk.chars().any(|c| c.is_ascii_digit()) {
            flush_spoken(&mut spoken, &mut out);
            out.push(groups(chunk));
        } else {
            let word: String = chunk.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
            if !word.is_empty() {
                spoken.push(word);
            }
        }
    }
    flush_spoken(&mut spoken, &mut out);
    out
}

/// Numbers in `output` that weren't in `input`, spoken or written. A
/// written number counts as said if its parts were ("four thirty" ->
/// "4:30") or all of it was ("530" -> "5:30").
pub fn unsaid_numbers(input: &str, output: &str) -> Vec<String> {
    let mut said = BTreeSet::new();
    for n in numbers(input) {
        said.insert(n.concat());
        said.extend(n);
    }
    numbers(output)
        .into_iter()
        .filter(|n| !said.contains(&n.concat()) && !n.iter().all(|g| said.contains(g)))
        .map(|n| n.join(":"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unsaid(input: &str, output: &str) -> Vec<String> {
        unsaid_numbers(input, output)
    }

    #[test]
    fn spoken_and_written_numbers_match() {
        assert!(unsaid("at four thirty", "At 4:30.").is_empty());
        assert!(unsaid("at 4.30", "at 4:30").is_empty());
        assert!(unsaid("by tomorrow 530", "by tomorrow 5:30").is_empty());
        assert!(unsaid("November twenty nine", "November 29th").is_empty());
        assert!(unsaid("four oh five", "4:05").is_empty());
        assert!(unsaid("someone, anyone, often", "Someone, anyone, often.").is_empty());
    }

    #[test]
    fn invented_numbers_are_found() {
        assert_eq!(unsaid("at 3.30, no, at 4.30", "at 4:30, no, at 5:30"), vec!["5:30"]);
        assert_eq!(unsaid("a couple of things", "2 things"), vec!["2"]);
        assert!(unsaid("at 4.30 or 5", "at 4:30").is_empty(), "dropping a number isn't this check's job");
    }
}
