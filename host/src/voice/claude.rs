//! Claude Messages API with streaming (server-sent events). The key stays on the host and is never logged.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;

pub const API_VERSION: &str = "2023-06-01";

pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a voice assistant. The user talks to you through the microphone of a \
PSP handheld, and your answer is read aloud by a text-to-speech voice through its small speaker and shown on its \
480x272 screen. Reply in the language the user spoke (Vietnamese or English). Keep answers short: one to three \
sentences unless the user asks for more detail. Write plain spoken text only: no markdown, lists, tables, code, \
emoji, or URLs. Write numbers, units, and abbreviations the way they should be spoken. If the transcript looks \
garbled, ask the user to repeat.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Turn {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Error)]
pub enum ClaudeError {
    #[error("no Claude API key")]
    NoKey,
    #[error("cannot reach the Claude API: {0}")]
    Network(String),
    #[error("Claude API returned HTTP {status} ({kind}): {message}")]
    Http { status: u16, kind: String, message: String },
    #[error("Claude API stream error ({kind}): {message}")]
    Stream { kind: String, message: String },
    #[error("malformed Claude API stream: {0}")]
    Protocol(String),
}

impl ClaudeError {
    /// Plain words for the PSP screen.
    pub fn user_message(&self) -> String {
        match self {
            Self::NoKey => "No Claude API key. Set ANTHROPIC_API_KEY or voice.api_key on the PC.".into(),
            Self::Network(_) => "Cannot reach Claude. Check the PC's internet connection.".into(),
            Self::Http { status: 401 | 403, .. } => "The Claude API key was rejected. Check it on the PC.".into(),
            Self::Http { status: 429, .. } => "Claude is rate limiting requests. Try again in a moment.".into(),
            Self::Http { status: 529, .. } => "Claude is overloaded right now. Try again shortly.".into(),
            Self::Stream { kind, .. } if kind == "overloaded_error" => {
                "Claude is overloaded right now. Try again shortly.".into()
            }
            Self::Stream { kind, .. } if kind == "rate_limit_error" => {
                "Claude is rate limiting requests. Try again in a moment.".into()
            }
            Self::Http { status, .. } if *status >= 500 => "Claude had a server error. Try again.".into(),
            Self::Http { message, .. } | Self::Stream { message, .. } => {
                format!("Claude refused the request: {}", crate::net::codec::truncate_utf8(message, 120))
            }
            Self::Protocol(_) => "Claude's reply was cut off. Try again.".into(),
        }
    }

    pub fn is_rate_limit(&self) -> bool {
        matches!(self, Self::Http { status: 429 | 529, .. })
            || matches!(self, Self::Stream { kind, .. } if kind == "overloaded_error" || kind == "rate_limit_error")
    }
}

#[derive(Clone)]
pub struct ClaudeClient {
    http: reqwest::Client,
    url: String,
    key: String,
    model: String,
    max_tokens: u32,
    system: String,
}

impl std::fmt::Debug for ClaudeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeClient").field("url", &self.url).field("model", &self.model).finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: &'a [Turn],
    stream: bool,
}

/// Incremental SSE parser: feed bytes, get complete events as (event name, data).
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<(String, String)> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            let end = find_blank_line(&self.buf);
            let Some((at, sep)) = end else { break };
            let block = String::from_utf8_lossy(&self.buf[..at]).into_owned();
            self.buf.drain(..at + sep);
            let (mut event, mut data) = (String::new(), Vec::new());
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    event = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
                }
            }
            if !event.is_empty() || !data.is_empty() {
                out.push((event, data.join("\n")));
            }
        }
        out
    }
}

fn find_blank_line(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len() {
        if buf[i..].starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
        if buf[i..].starts_with(b"\n\n") {
            return Some((i, 2));
        }
    }
    None
}

impl ClaudeClient {
    pub fn new(
        api_base: &str,
        key: String,
        model: String,
        max_tokens: u32,
        system: String,
    ) -> Result<Self, ClaudeError> {
        if key.trim().is_empty() {
            return Err(ClaudeError::NoKey);
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| ClaudeError::Network(e.to_string()))?;
        let system = if system.trim().is_empty() { DEFAULT_SYSTEM_PROMPT.to_string() } else { system };
        Ok(Self {
            http,
            url: format!("{}/v1/messages", api_base.trim_end_matches('/')),
            key,
            model,
            max_tokens,
            system,
        })
    }

    /// Streams one reply. Text deltas go to `deltas` as they arrive; the full reply is returned.
    /// If the receiver is dropped, the request is abandoned and the text so far is returned.
    pub async fn stream(&self, messages: &[Turn], deltas: mpsc::Sender<String>) -> Result<String, ClaudeError> {
        let body =
            Request { model: &self.model, max_tokens: self.max_tokens, system: &self.system, messages, stream: true };
        let mut resp = self
            .http
            .post(&self.url)
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ClaudeError::Network(e.without_url().to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let (kind, message) = error_fields(serde_json::from_str(&text).ok().as_ref());
            return Err(ClaudeError::Http { status: status.as_u16(), kind, message });
        }

        let mut parser = SseParser::default();
        let mut full = String::new();
        let mut stopped = false;
        while let Some(chunk) = resp.chunk().await.map_err(|e| ClaudeError::Network(e.without_url().to_string()))? {
            for (event, data) in parser.push(&chunk) {
                let v: Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(_) if event == "ping" => continue,
                    Err(e) => return Err(ClaudeError::Protocol(format!("bad JSON in {event}: {e}"))),
                };
                match v.get("type").and_then(Value::as_str).unwrap_or(&event) {
                    "content_block_delta" => {
                        let delta = &v["delta"];
                        if delta["type"] == "text_delta"
                            && let Some(t) = delta["text"].as_str()
                        {
                            full.push_str(t);
                            if deltas.send(t.to_string()).await.is_err() {
                                return Ok(full);
                            }
                        }
                    }
                    "error" => {
                        let (kind, message) = error_fields(Some(&v));
                        return Err(ClaudeError::Stream { kind, message });
                    }
                    "message_stop" => stopped = true,
                    // message_start, content_block_start/stop, message_delta, ping, and future event types.
                    _ => {}
                }
            }
            if stopped {
                break;
            }
        }
        if !stopped {
            return Err(ClaudeError::Protocol("the stream ended before message_stop".into()));
        }
        Ok(full)
    }
}

fn error_fields(v: Option<&Value>) -> (String, String) {
    let e = v.and_then(|v| v.get("error"));
    let kind = e.and_then(|e| e.get("type")).and_then(Value::as_str).unwrap_or("unknown").to_string();
    let message = e.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("").to_string();
    (kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sse_across_chunk_boundaries() {
        let stream = "event: ping\ndata: {\"type\": \"ping\"}\n\nevent: content_block_delta\r\ndata: {\"a\":\r\ndata: 1}\r\n\r\n: comment\n\n";
        let mut p = SseParser::default();
        let mut events = Vec::new();
        for b in stream.as_bytes().chunks(3) {
            events.extend(p.push(b));
        }
        assert_eq!(events[0], ("ping".to_string(), "{\"type\": \"ping\"}".to_string()));
        assert_eq!(events[1], ("content_block_delta".to_string(), "{\"a\":\n1}".to_string()));
        assert_eq!(events.len(), 2, "comment-only blocks are skipped");
    }

    #[test]
    fn errors_read_as_plain_words() {
        let e = ClaudeError::Http { status: 429, kind: "rate_limit_error".into(), message: "x".into() };
        assert!(e.is_rate_limit());
        assert!(e.user_message().contains("rate limiting"));
        let e = ClaudeError::Stream { kind: "overloaded_error".into(), message: "Overloaded".into() };
        assert!(e.is_rate_limit() && e.user_message().contains("overloaded"));
        assert!(ClaudeError::NoKey.user_message().contains("ANTHROPIC_API_KEY"));
        assert!(ClaudeClient::new("http://x", " ".into(), "m".into(), 10, String::new()).is_err());
    }
}
