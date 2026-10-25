//! Text-to-speech: Piper on the PC (default) or an OpenAI-compatible hosted endpoint. One voice per
//! language; the language is guessed from the sentence.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::dsp::parse_wav;
use super::text::is_vietnamese;
use super::{TtsConfig, TtsEngine};

const TIMEOUT: Duration = Duration::from_secs(30);
const PIPER_DEFAULT_RATE: u32 = 22_050;

#[derive(Debug, Error)]
pub enum TtsError {
    #[error("text-to-speech is not configured: {0}")]
    NotConfigured(String),
    #[error("text-to-speech failed: {0}")]
    Failed(String),
}

pub enum Tts {
    Piper {
        bin: PathBuf,
        voice_vi: Option<PathBuf>,
        voice_en: Option<PathBuf>,
    },
    Openai {
        http: reqwest::Client,
        url: String,
        model: String,
        voice_vi: String,
        voice_en: String,
        key: Option<String>,
    },
}

/// Mono audio at `rate`.
pub struct Speech {
    pub rate: u32,
    pub samples: Vec<f32>,
}

fn non_empty(s: &str) -> Option<PathBuf> {
    let s = s.trim();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

impl Tts {
    /// None when replies are text-only.
    pub fn from_config(c: &TtsConfig) -> Result<Option<Self>, TtsError> {
        match c.engine {
            TtsEngine::None => Ok(None),
            TtsEngine::Piper => {
                let (voice_vi, voice_en) = (non_empty(&c.piper_voice_vi), non_empty(&c.piper_voice_en));
                if c.piper_bin.trim().is_empty() || (voice_vi.is_none() && voice_en.is_none()) {
                    return Err(TtsError::NotConfigured(
                        "set voice.tts.piper_bin and at least one of piper_voice_vi / piper_voice_en".into(),
                    ));
                }
                Ok(Some(Self::Piper { bin: c.piper_bin.trim().into(), voice_vi, voice_en }))
            }
            TtsEngine::Openai => Ok(Some(Self::Openai {
                http: reqwest::Client::builder()
                    .timeout(TIMEOUT)
                    .build()
                    .map_err(|e| TtsError::Failed(e.to_string()))?,
                url: c.url.clone(),
                model: c.model.clone(),
                voice_vi: c.voice_vi.clone(),
                voice_en: c.voice_en.clone(),
                key: std::env::var(&c.api_key_env).ok().filter(|k| !k.trim().is_empty()),
            })),
        }
    }

    pub async fn synthesize(&self, text: &str) -> Result<Speech, TtsError> {
        let vi = is_vietnamese(text);
        match self {
            Self::Piper { bin, voice_vi, voice_en } => {
                // Fall back to the other voice rather than staying silent.
                let voice =
                    if vi { voice_vi.as_ref().or(voice_en.as_ref()) } else { voice_en.as_ref().or(voice_vi.as_ref()) };
                piper(bin, voice.expect("checked in from_config"), text).await
            }
            Self::Openai { http, url, model, voice_vi, voice_en, key } => {
                let body = serde_json::json!({
                    "model": model,
                    "input": text,
                    "voice": if vi { voice_vi } else { voice_en },
                    "response_format": "wav",
                });
                let mut req = http.post(url).json(&body);
                if let Some(k) = key {
                    req = req.bearer_auth(k);
                }
                let resp = req.send().await.map_err(|e| TtsError::Failed(e.without_url().to_string()))?;
                let status = resp.status();
                let bytes = resp.bytes().await.map_err(|e| TtsError::Failed(e.without_url().to_string()))?;
                if !status.is_success() {
                    return Err(TtsError::Failed(format!("HTTP {status}")));
                }
                let (rate, samples) =
                    parse_wav(&bytes).ok_or_else(|| TtsError::Failed("response is not 16-bit PCM WAV".into()))?;
                Ok(Speech { rate, samples })
            }
        }
    }
}

/// Piper voices ship with `<voice>.onnx.json`, which holds the output sample rate.
pub fn piper_rate(voice: &Path) -> u32 {
    let json = PathBuf::from(format!("{}.json", voice.display()));
    std::fs::read_to_string(json)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["audio"]["sample_rate"].as_u64())
        .and_then(|r| u32::try_from(r).ok())
        .filter(|r| (8_000..=48_000).contains(r))
        .unwrap_or(PIPER_DEFAULT_RATE)
}

async fn piper(bin: &Path, voice: &Path, text: &str) -> Result<Speech, TtsError> {
    let mut child = Command::new(bin)
        .arg("--model")
        .arg(voice)
        .arg("--output_raw")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| TtsError::NotConfigured(format!("cannot start {}: {e}", bin.display())))?;
    let mut stdin = child.stdin.take().expect("piped");
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    // Piper reads one utterance per line.
    let line = format!("{}\n", text.replace(['\r', '\n'], " "));
    let run = async {
        let write = async {
            stdin.write_all(line.as_bytes()).await?;
            drop(stdin);
            Ok::<_, std::io::Error>(())
        };
        let (mut raw, mut err) = (Vec::new(), String::new());
        let (w, r, e) = tokio::join!(write, stdout.read_to_end(&mut raw), stderr.read_to_string(&mut err));
        w.and(r.map(drop)).and(e.map(drop)).map_err(|e| TtsError::Failed(e.to_string()))?;
        let status = child.wait().await.map_err(|e| TtsError::Failed(e.to_string()))?;
        if !status.success() {
            let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
            return Err(TtsError::Failed(format!("piper exited with {status}: {tail}")));
        }
        Ok(raw)
    };
    let raw = tokio::time::timeout(TIMEOUT, run).await.map_err(|_| TtsError::Failed("piper timed out".into()))??;
    let samples =
        raw.as_chunks::<2>().0.iter().map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0).collect();
    Ok(Speech { rate: piper_rate(voice), samples })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_checks() {
        assert!(Tts::from_config(&TtsConfig::default()).is_err(), "piper needs paths");
        let none = TtsConfig { engine: TtsEngine::None, ..Default::default() };
        assert!(Tts::from_config(&none).unwrap().is_none());
    }

    #[test]
    fn reads_piper_sample_rate() {
        let dir = std::env::temp_dir().join(format!("pspc-piper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let voice = dir.join("vi_VN-test.onnx");
        std::fs::write(dir.join("vi_VN-test.onnx.json"), r#"{"audio": {"sample_rate": 16000}}"#).unwrap();
        assert_eq!(piper_rate(&voice), 16_000);
        assert_eq!(piper_rate(&dir.join("missing.onnx")), PIPER_DEFAULT_RATE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn missing_binary_is_reported_as_not_configured() {
        let t = Tts::Piper { bin: "Z:/no/piper.exe".into(), voice_vi: None, voice_en: Some("v.onnx".into()) };
        assert!(matches!(t.synthesize("hello").await, Err(TtsError::NotConfigured(_))));
    }

    /// Runs only with a real Piper install: PSPC_PIPER_BIN and PSPC_PIPER_VOICE (an .onnx voice).
    #[tokio::test]
    async fn piper_speaks() {
        let (Ok(bin), Ok(voice)) = (std::env::var("PSPC_PIPER_BIN"), std::env::var("PSPC_PIPER_VOICE")) else {
            eprintln!("skipped: set PSPC_PIPER_BIN and PSPC_PIPER_VOICE to run against Piper");
            return;
        };
        let t = Tts::Piper { bin: bin.into(), voice_vi: Some(voice.clone().into()), voice_en: Some(voice.into()) };
        let s = t.synthesize("Xin chào, đây là một bài kiểm tra.").await.unwrap();
        assert!(s.samples.len() as u32 > s.rate / 2, "at least half a second of speech");
    }
}
