//! Minimal client for the local Ollama server: chat (whole or streamed) and
//! embeddings. Used by cleanup, note filing, and the notes Q&A.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::time::Duration;

/// Ollama's default local address.
const URL: &str = "http://127.0.0.1:11434";

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
}

/// Generation settings for one request.
pub struct Options {
    pub temperature: f32,
    /// Maximum tokens to generate.
    pub max_tokens: i32,
    /// How long Ollama keeps the model loaded afterwards, e.g. "60m".
    pub keep_alive: &'static str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    stream: bool,
    keep_alive: &'a str,
    options: RequestOptions,
}

#[derive(Serialize)]
struct RequestOptions {
    temperature: f32,
    num_predict: i32,
}

#[derive(Deserialize)]
struct ChatChunk {
    message: Option<ChunkMessage>,
    #[serde(default)]
    done: bool,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ChunkMessage {
    content: String,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[derive(Clone)]
pub struct Ollama {
    agent: ureq::Agent,
}

impl Ollama {
    /// `timeout` bounds a whole request. Streaming answers can take minutes
    /// on a CPU, so pass `None` for those.
    pub fn new(timeout: Option<Duration>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(timeout)
            .build()
            .into();
        Self { agent }
    }

    /// Load `model` into memory without generating anything.
    pub fn load(&self, model: &str, keep_alive: &str) -> Result<()> {
        let body = serde_json::json!({ "model": model, "keep_alive": keep_alive });
        self.agent
            .post(format!("{URL}/api/generate"))
            .send_json(&body)
            .map_err(|e| explain(e, model))?;
        Ok(())
    }

    /// Run a chat and return the whole reply.
    pub fn chat(&self, model: &str, messages: &[Message], options: &Options) -> Result<String> {
        self.chat_stream(model, messages, options, |_| {})
    }

    /// Run a chat, calling `on_token` with each piece of the reply as it is
    /// generated. Returns the whole reply.
    pub fn chat_stream(
        &self,
        model: &str,
        messages: &[Message],
        options: &Options,
        mut on_token: impl FnMut(&str),
    ) -> Result<String> {
        let request = ChatRequest {
            model,
            messages,
            stream: true,
            keep_alive: options.keep_alive,
            options: RequestOptions { temperature: options.temperature, num_predict: options.max_tokens },
        };
        let response = self
            .agent
            .post(format!("{URL}/api/chat"))
            .send_json(&request)
            .map_err(|e| explain(e, model))?;

        // The reply is one JSON object per line.
        let mut reply = String::new();
        let reader = BufReader::new(response.into_body().into_reader());
        for line in reader.lines() {
            let line = line.map_err(|e| anyhow!("Ollama connection dropped: {e}"))?;
            if line.trim().is_empty() {
                continue;
            }
            let chunk: ChatChunk = serde_json::from_str(&line)?;
            if let Some(error) = chunk.error {
                return Err(anyhow!("Ollama: {error}"));
            }
            if let Some(m) = chunk.message {
                on_token(&m.content);
                reply.push_str(&m.content);
            }
            if chunk.done {
                break;
            }
        }
        Ok(reply)
    }

    /// One embedding vector per input text.
    pub fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let body = serde_json::json!({ "model": model, "input": inputs, "keep_alive": "10m" });
        let response: EmbedResponse = self
            .agent
            .post(format!("{URL}/api/embed"))
            .send_json(&body)
            .and_then(|mut r| r.body_mut().read_json())
            .map_err(|e| explain(e, model))?;
        if response.embeddings.len() != inputs.len() {
            return Err(anyhow!("Ollama returned {} embeddings for {} texts", response.embeddings.len(), inputs.len()));
        }
        Ok(response.embeddings)
    }
}

fn explain(e: ureq::Error, model: &str) -> anyhow::Error {
    match e {
        ureq::Error::ConnectionFailed | ureq::Error::Io(_) => anyhow!("Ollama isn't running. Start Ollama and try again."),
        ureq::Error::StatusCode(404) => anyhow!("Model {model} isn't downloaded. Run: ollama pull {model}"),
        ureq::Error::Timeout(_) => anyhow!("Ollama took too long to respond"),
        other => anyhow!("Ollama request failed: {other}"),
    }
}
