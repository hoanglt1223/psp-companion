//! Push-to-talk Claude assistant: PSP microphone audio in, transcript and spoken reply out.

pub mod adpcm;
pub mod claude;
pub mod dsp;
pub mod session;
pub mod stt;
pub mod text;
pub mod tts;

use std::fmt;

use serde::{Deserialize, Serialize};

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Lower first-token latency for voice; set `voice.model` to this.
pub const FAST_MODEL: &str = "claude-haiku-5-5";
pub const DEFAULT_API_BASE: &str = "https://api.anthropic.com";

/// A secret from the config file: never printed by `Debug`.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_empty() { "Secret(empty)" } else { "Secret(***)" })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttEngine {
    /// whisper.cpp's command-line tool with a local model (default).
    WhisperCpp,
    /// Any service with an OpenAI-compatible `/v1/audio/transcriptions` endpoint.
    Openai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TtsEngine {
    /// Piper's command-line tool with local voices (default).
    Piper,
    /// Any service with an OpenAI-compatible `/v1/audio/speech` endpoint.
    Openai,
    /// Text replies only.
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SttConfig {
    pub engine: SttEngine,
    pub whisper_bin: String,
    pub whisper_model: String,
    pub url: String,
    pub model: String,
    /// Name of the environment variable holding the hosted service's key.
    pub api_key_env: String,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: SttEngine::WhisperCpp,
            whisper_bin: String::new(),
            whisper_model: String::new(),
            url: "https://api.openai.com/v1/audio/transcriptions".into(),
            model: "whisper-1".into(),
            api_key_env: "OPENAI_API_KEY".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TtsConfig {
    pub engine: TtsEngine,
    pub piper_bin: String,
    /// Piper `.onnx` voices; the reply's language picks one.
    pub piper_voice_vi: String,
    pub piper_voice_en: String,
    pub url: String,
    pub model: String,
    pub voice_vi: String,
    pub voice_en: String,
    pub api_key_env: String,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            engine: TtsEngine::Piper,
            piper_bin: String::new(),
            piper_voice_vi: String::new(),
            piper_voice_en: String::new(),
            url: "https://api.openai.com/v1/audio/speech".into(),
            model: "gpt-4o-mini-tts".into(),
            voice_vi: "alloy".into(),
            voice_en: "alloy".into(),
            api_key_env: "OPENAI_API_KEY".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VoiceConfig {
    pub enabled: bool,
    pub model: String,
    pub api_base: String,
    /// Empty: read ANTHROPIC_API_KEY from the environment.
    pub api_key: Secret,
    pub max_tokens: u32,
    /// Question/answer pairs kept per PSP.
    pub history_turns: u16,
    /// Empty: the built-in prompt for short spoken answers.
    pub system_prompt: String,
    /// When set, every utterance is saved here as a WAV file (for checking the microphone).
    pub save_wav_dir: String,
    pub stt: SttConfig,
    pub tts: TtsConfig,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: DEFAULT_MODEL.into(),
            api_base: DEFAULT_API_BASE.into(),
            api_key: Secret::default(),
            max_tokens: 400,
            history_turns: 10,
            system_prompt: String::new(),
            save_wav_dir: String::new(),
            stt: SttConfig::default(),
            tts: TtsConfig::default(),
        }
    }
}

impl VoiceConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.model.trim().is_empty() {
            return Err("voice.model is empty".into());
        }
        if !(16..=4096).contains(&self.max_tokens) {
            return Err("voice.max_tokens must be 16-4096".into());
        }
        if self.history_turns > 50 {
            return Err("voice.history_turns must be 0-50".into());
        }
        for (name, url) in
            [("voice.api_base", &self.api_base), ("voice.stt.url", &self.stt.url), ("voice.tts.url", &self.tts.url)]
        {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(format!("{name} must be an http(s) URL"));
            }
        }
        Ok(())
    }

    /// The API key from the config, else from ANTHROPIC_API_KEY.
    pub fn resolve_api_key(&self) -> Option<String> {
        let k = self.api_key.0.trim();
        if !k.is_empty() {
            return Some(k.to_string());
        }
        std::env::var("ANTHROPIC_API_KEY").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print() {
        let c = VoiceConfig { api_key: Secret("sk-ant-very-secret".into()), ..Default::default() };
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("very-secret"), "{dbg}");
        assert!(dbg.contains("Secret(***)"));
        assert_eq!(c.resolve_api_key().as_deref(), Some("sk-ant-very-secret"));
    }

    #[test]
    fn validates() {
        let mut c = VoiceConfig::default();
        c.validate().unwrap();
        assert_eq!(c.model, "claude-opus-5-5");
        c.max_tokens = 5;
        assert!(c.validate().is_err());
        let c = VoiceConfig { api_base: "ftp://x".into(), ..Default::default() };
        assert!(c.validate().is_err());
    }
}
