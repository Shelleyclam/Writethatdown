//! Optional second opinion on meaning drift (`eval --judge`): a bigger local
//! model reads what Whisper heard, what was wanted, and what cleanup made of
//! it, and says whether cleanup kept the meaning. Slower and less
//! predictable than the word-based check, so it's reported next to it, not
//! instead of it.

use crate::ollama::{Message, Ollama, Options};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The same model as the notes Q&A, so nothing extra to download.
pub const MODEL: &str = "qwen2.5:7b";

const SYSTEM_PROMPT: &str = "\
You check the output of a dictation cleanup tool. You get three texts:
RAW: what speech recognition heard. It may contain filler words, repeated words, \
self-corrections (\"Tuesday, no wait, Wednesday\") and recognition mistakes.
INTENDED: what the speaker actually wanted to write.
CLEANED: what the cleanup tool made from RAW.

Decide whether CLEANED keeps the meaning of INTENDED, judging only the changes the tool \
made from RAW to CLEANED.
Fine: removing filler words, repetitions and the corrected-away part of a self-correction; \
fixing punctuation, capitalization and number format; recognition mistakes that were \
already in RAW and simply kept.
Not fine: dropping a fact, request or sentence; adding information; changing a name, \
number, date or negation; keeping the wrong half of a self-correction; answering or \
summarizing the text instead of cleaning it.

Reply with only JSON: {\"preserved\": true or false, \"reason\": \"one short sentence\"}";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Verdict {
    pub preserved: bool,
    pub reason: String,
}

pub struct Judge {
    ollama: Ollama,
    pub model: String,
}

impl Judge {
    pub fn new(model: &str) -> Self {
        // Generous: the first call loads a 7B model.
        Self { ollama: Ollama::new(Some(Duration::from_secs(180))), model: model.to_string() }
    }

    pub fn judge(&self, raw: &str, intended: &str, cleaned: &str) -> Result<Verdict> {
        let messages = [
            Message::system(SYSTEM_PROMPT),
            Message::user(format!("RAW: {raw}\nINTENDED: {intended}\nCLEANED: {cleaned}")),
        ];
        let options = Options { temperature: 0.0, max_tokens: 120, keep_alive: "10m" };
        parse(&self.ollama.chat(&self.model, &messages, &options)?)
    }
}

/// The JSON object in the reply, even if the model wrapped it in a code
/// block or added a sentence.
fn parse(reply: &str) -> Result<Verdict> {
    let start = reply.find('{');
    let end = reply.rfind('}');
    match (start, end) {
        (Some(s), Some(e)) if s < e => serde_json::from_str(&reply[s..=e])
            .map_err(|err| anyhow!("Judge reply isn't the expected JSON ({err}): {reply:?}")),
        _ => Err(anyhow!("Judge reply has no JSON: {reply:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wrapped_json() {
        let v = parse("```json\n{\"preserved\": false, \"reason\": \"Dropped the date.\"}\n```").unwrap();
        assert_eq!(v, Verdict { preserved: false, reason: "Dropped the date.".into() });
        assert!(parse("Looks fine to me.").is_err());
        assert!(parse("{\"preserved\": \"maybe\"}").is_err());
    }
}
