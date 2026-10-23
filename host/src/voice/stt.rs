//! Speech-to-text: whisper.cpp on the PC (default) or an OpenAI-compatible hosted endpoint.
//! Both detect the language (Vietnamese or English) themselves.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::dsp::wav_bytes;
use super::{SttConfig, SttEngine};

pub const STT_RATE: u32 = 16_000;
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Error)]
pub enum SttError {
    #[error("speech-to-text is not configured: {0}")]
    NotConfigured(String),
    #[error("speech-to-text failed: {0}")]
    Failed(String),
}

pub enum Stt {
    Whisper { bin: PathBuf, model: PathBuf },
    Openai { http: reqwest::Client, url: String, model: String, key: Option<String> },
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Stt {
    pub fn from_config(c: &SttConfig) -> Result<Self, SttError> {
        match c.engine {
            SttEngine::WhisperCpp => {
                if c.whisper_bin.trim().is_empty() || c.whisper_model.trim().is_empty() {
                    return Err(SttError::NotConfigured(
                        "set voice.stt.whisper_bin and voice.stt.whisper_model (whisper.cpp)".into(),
                    ));
                }
                Ok(Self::Whisper { bin: c.whisper_bin.trim().into(), model: c.whisper_model.trim().into() })
            }
            SttEngine::Openai => Ok(Self::Openai {
                http: reqwest::Client::builder()
                    .timeout(TIMEOUT)
                    .build()
                    .map_err(|e| SttError::Failed(e.to_string()))?,
                url: c.url.clone(),
                model: c.model.clone(),
                key: std::env::var(&c.api_key_env).ok().filter(|k| !k.trim().is_empty()),
            }),
        }
    }

    /// Transcribes 16 kHz mono audio.
    pub async fn transcribe(&self, pcm16k: &[f32]) -> Result<String, SttError> {
        let wav = wav_bytes(pcm16k, STT_RATE);
        match self {
            Self::Whisper { bin, model } => whisper(bin, model, &wav).await,
            Self::Openai { http, url, model, key } => {
                let part = reqwest::multipart::Part::bytes(wav)
                    .file_name("speech.wav")
                    .mime_str("audio/wav")
                    .map_err(|e| SttError::Failed(e.to_string()))?;
                let form = reqwest::multipart::Form::new().part("file", part).text("model", model.clone());
                let mut req = http.post(url).multipart(form);
                if let Some(k) = key {
                    req = req.bearer_auth(k);
                }
                let resp = req.send().await.map_err(|e| SttError::Failed(e.without_url().to_string()))?;
                let status = resp.status();
                let body = resp.text().await.map_err(|e| SttError::Failed(e.without_url().to_string()))?;
                if !status.is_success() {
                    return Err(SttError::Failed(format!("HTTP {status}")));
                }
                let v: serde_json::Value =
                    serde_json::from_str(&body).map_err(|_| SttError::Failed("response is not JSON".into()))?;
                v["text"].as_str().map(|t| t.trim().to_string()).ok_or_else(|| SttError::Failed("no text".into()))
            }
        }
    }
}

async fn whisper(bin: &PathBuf, model: &PathBuf, wav: &[u8]) -> Result<String, SttError> {
    let path = std::env::temp_dir().join(format!(
        "pspc-stt-{}-{}.wav",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    tokio::fs::write(&path, wav).await.map_err(|e| SttError::Failed(format!("cannot write temp WAV: {e}")))?;
    let result = run_whisper(bin, model, &path).await;
    let _ = tokio::fs::remove_file(&path).await;
    result
}

async fn run_whisper(bin: &PathBuf, model: &PathBuf, wav: &std::path::Path) -> Result<String, SttError> {
    let mut child = Command::new(bin)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(wav)
        .args(["-l", "auto", "-nt", "-np"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| SttError::NotConfigured(format!("cannot start {}: {e}", bin.display())))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let run = async {
        let (mut out, mut err) = (String::new(), String::new());
        let (a, b) = tokio::join!(stdout.read_to_string(&mut out), stderr.read_to_string(&mut err));
        a.and(b).map_err(|e| SttError::Failed(e.to_string()))?;
        let status = child.wait().await.map_err(|e| SttError::Failed(e.to_string()))?;
        if !status.success() {
            let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
            return Err(SttError::Failed(format!("whisper exited with {status}: {tail}")));
        }
        Ok(out)
    };
    let out = tokio::time::timeout(TIMEOUT, run).await.map_err(|_| SttError::Failed("whisper timed out".into()))??;
    Ok(clean_transcript(&out))
}

/// Joins whisper's lines and drops its non-speech markers such as "[BLANK_AUDIO]" or "(music)".
pub fn clean_transcript(raw: &str) -> String {
    let mut text = String::new();
    for line in raw.lines() {
        let mut l = line.trim().to_string();
        for (open, close) in [('[', ']'), ('(', ')')] {
            while let (Some(a), Some(b)) = (l.find(open), l.find(close)) {
                if b < a {
                    break;
                }
                l.replace_range(a..=b, "");
            }
        }
        let l = l.trim();
        if !l.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(l);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_whisper_output() {
        assert_eq!(
            clean_transcript(" Xin chào.\n [BLANK_AUDIO]\n Bạn khỏe không? (music)\n"),
            "Xin chào. Bạn khỏe không?"
        );
        assert_eq!(clean_transcript("[BLANK_AUDIO]"), "");
    }

    #[test]
    fn whisper_requires_paths() {
        assert!(matches!(Stt::from_config(&SttConfig::default()), Err(SttError::NotConfigured(_))));
    }

    #[tokio::test]
    async fn missing_binary_is_reported_as_not_configured() {
        let stt = Stt::Whisper { bin: "Z:/definitely/not/whisper-cli.exe".into(), model: "m.bin".into() };
        let e = stt.transcribe(&[0.0; 1600]).await.unwrap_err();
        assert!(matches!(e, SttError::NotConfigured(_)), "{e}");
    }

    /// Runs only with a real whisper.cpp install: PSPC_WHISPER_BIN and PSPC_WHISPER_MODEL.
    #[tokio::test]
    async fn whisper_cpp_transcribes_silence_to_nothing() {
        let (Ok(bin), Ok(model)) = (std::env::var("PSPC_WHISPER_BIN"), std::env::var("PSPC_WHISPER_MODEL")) else {
            eprintln!("skipped: set PSPC_WHISPER_BIN and PSPC_WHISPER_MODEL to run against whisper.cpp");
            return;
        };
        let stt = Stt::Whisper { bin: bin.into(), model: model.into() };
        let text = stt.transcribe(&vec![0.0; STT_RATE as usize]).await.unwrap();
        assert!(text.len() < 40, "silence: {text:?}");
    }
}
