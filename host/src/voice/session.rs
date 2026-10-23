//! One push-to-talk exchange per PSP at a time: collect the recording, clean it, transcribe it, stream
//! Claude's reply as text, and speak it sentence by sentence while the rest is still being written.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use super::claude::{ClaudeClient, ClaudeError, Role, Turn};
use super::dsp::{clean_speech, voiced_seconds, write_wav};
use super::stt::{STT_RATE, Stt};
use super::text::{SentenceSplitter, speakable};
use super::tts::Tts;
use super::{VoiceConfig, adpcm};
use crate::audio::encode_mp3::Resampler;
use crate::audio::stream::{AudioHandle, VoicePcm};
use crate::net::codec::truncate_utf8;
use crate::net::handshake::Id16;
use crate::net::message::{Message, error_code};
use crate::state::AppState;

pub const CODEC_IMA_ADPCM: u8 = 1;
pub const SAMPLE_RATES: [u32; 4] = [8_000, 11_025, 16_000, 22_050];
pub const MAX_SECONDS: u32 = 60;
pub const END_RELEASED: u8 = 0;
pub const END_CANCELLED: u8 = 1;
pub const END_TOO_LONG: u8 = 2;
/// Reply text is pushed to the PSP in batches, not per token.
pub const TEXT_FLUSH: Duration = Duration::from_millis(250);
const MAX_TEXT: usize = 4000;
const MIN_VOICED_SECONDS: f32 = 0.25;

/// What the voice feature runs on; each part may be missing with a reason shown to the user.
pub struct Engines {
    pub claude: Result<ClaudeClient, String>,
    pub stt: Result<Stt, String>,
    /// Ok(None): text replies only.
    pub tts: Result<Option<Tts>, String>,
}

impl Engines {
    pub fn from_config(c: &VoiceConfig) -> Self {
        let claude = c
            .resolve_api_key()
            .ok_or(ClaudeError::NoKey)
            .and_then(|key| ClaudeClient::new(&c.api_base, key, c.model.clone(), c.max_tokens, c.system_prompt.clone()))
            .map_err(|e| e.user_message());
        let stt = Stt::from_config(&c.stt).map_err(|e| e.to_string());
        let tts = Tts::from_config(&c.tts).map_err(|e| e.to_string());
        Self { claude, stt, tts }
    }
}

struct Recording {
    utterance_id: u32,
    rate: u32,
    next_seq: u32,
    pcm: Vec<i16>,
}

struct Ctx {
    state: Arc<AppState>,
    audio: AudioHandle,
    engines: Engines,
    history_turns: usize,
    save_wav_dir: Option<PathBuf>,
    histories: Mutex<HashMap<Id16, Vec<Turn>>>,
}

pub struct VoiceService {
    ctx: Arc<Ctx>,
    enabled: bool,
    recordings: HashMap<Id16, Recording>,
    tasks: HashMap<Id16, JoinHandle<()>>,
    sessions: HashMap<Id16, u64>,
}

async fn notice(state: &AppState, device: &Id16, code: u16, text: &str) {
    let _ = state.send_to(device, Message::Error { code, text: text.to_string() }).await;
}

impl VoiceService {
    pub fn new(state: Arc<AppState>, audio: AudioHandle, config: &VoiceConfig, engines: Engines) -> Self {
        for (what, r) in [("Claude", engines.claude.as_ref().err()), ("speech-to-text", engines.stt.as_ref().err())] {
            if let Some(e) = r {
                info!("voice assistant: {what} unavailable: {e}");
            }
        }
        if let Err(e) = &engines.tts {
            info!("voice assistant: spoken replies unavailable: {e}");
        }
        let save = config.save_wav_dir.trim();
        Self {
            ctx: Arc::new(Ctx {
                state,
                audio,
                engines,
                history_turns: usize::from(config.history_turns),
                save_wav_dir: (!save.is_empty()).then(|| PathBuf::from(save)),
                histories: Mutex::new(HashMap::new()),
            }),
            enabled: config.enabled,
            recordings: HashMap::new(),
            tasks: HashMap::new(),
            sessions: HashMap::new(),
        }
    }

    pub fn session_up(&mut self, device: Id16, session_id: u64) {
        self.sessions.insert(device, session_id);
    }

    /// Conversation history stays; the recording and any reply in progress are dropped.
    pub fn session_down(&mut self, device: Id16, session_id: u64) {
        if self.sessions.get(&device) != Some(&session_id) {
            return;
        }
        self.sessions.remove(&device);
        self.recordings.remove(&device);
        if let Some(t) = self.tasks.remove(&device) {
            t.abort();
        }
    }

    pub async fn on_message(&mut self, device: Id16, msg: Message) {
        match msg {
            Message::VoiceStart { utterance_id, codec, sample_rate, channels } => {
                if let Some(t) = self.tasks.remove(&device) {
                    t.abort(); // the user started talking again
                }
                self.recordings.remove(&device);
                if !self.enabled {
                    notice(
                        &self.ctx.state,
                        &device,
                        error_code::UNSUPPORTED,
                        "The voice assistant is turned off on the PC.",
                    )
                    .await;
                    return;
                }
                if codec != CODEC_IMA_ADPCM || channels != 1 || !SAMPLE_RATES.contains(&sample_rate) {
                    notice(&self.ctx.state, &device, error_code::UNSUPPORTED, "Unsupported microphone format.").await;
                    return;
                }
                self.recordings
                    .insert(device, Recording { utterance_id, rate: sample_rate, next_seq: 0, pcm: Vec::new() });
            }
            Message::VoiceChunk { utterance_id, seq, data } => {
                let Some(r) = self.recordings.get_mut(&device).filter(|r| r.utterance_id == utterance_id) else {
                    return;
                };
                let decoded = if seq == r.next_seq { adpcm::decode_chunk(&data).ok() } else { None };
                let Some(pcm) = decoded else {
                    warn!(seq, expected = r.next_seq, "bad voice chunk; dropping the recording");
                    self.recordings.remove(&device);
                    return;
                };
                r.next_seq += 1;
                let cap = (r.rate * MAX_SECONDS) as usize;
                let room = cap.saturating_sub(r.pcm.len());
                r.pcm.extend_from_slice(&pcm[..pcm.len().min(room)]);
            }
            Message::VoiceEnd { utterance_id, reason } => {
                let Some(r) = self.recordings.remove(&device).filter(|r| r.utterance_id == utterance_id) else {
                    return;
                };
                if reason == END_CANCELLED {
                    return;
                }
                let ctx = self.ctx.clone();
                self.tasks.insert(device, tokio::spawn(async move { exchange(ctx, device, r).await }));
            }
            _ => {}
        }
    }
}

fn seconds_since_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

async fn exchange(ctx: Arc<Ctx>, device: Id16, rec: Recording) {
    let state = &ctx.state;
    let utterance_id = rec.utterance_id;
    let clean = clean_speech(&rec.pcm, rec.rate);
    if let Some(dir) = &ctx.save_wav_dir {
        let path = dir.join(format!("utterance-{}-{utterance_id}.wav", seconds_since_epoch()));
        match std::fs::create_dir_all(dir).and_then(|_| write_wav(&path, &clean, rec.rate)) {
            Ok(()) => info!(path = %path.display(), "saved utterance"),
            Err(e) => warn!("cannot save utterance WAV: {e}"),
        }
    }
    if voiced_seconds(&clean, rec.rate) < MIN_VOICED_SECONDS {
        notice(state, &device, error_code::INTERNAL, "I did not hear anything. Hold R while you speak.").await;
        return;
    }
    let stt = match &ctx.engines.stt {
        Ok(s) => s,
        Err(e) => return notice(state, &device, error_code::UNSUPPORTED, &format!("Speech recognition: {e}")).await,
    };
    let pcm16k = Resampler::new(rec.rate, STT_RATE, 1).process(&clean);
    let transcript = match stt.transcribe(&pcm16k).await {
        Ok(t) => t,
        Err(e) => {
            warn!("{e}");
            return notice(state, &device, error_code::INTERNAL, "Speech recognition failed on the PC.").await;
        }
    };
    if transcript.is_empty() {
        notice(state, &device, error_code::INTERNAL, "I could not make out any words. Try again.").await;
        return;
    }
    let transcript = truncate_utf8(&transcript, MAX_TEXT).to_string();
    info!(utterance_id, chars = transcript.chars().count(), "transcribed");
    let msg = Message::VoiceTranscript { utterance_id, is_final: true, text: transcript.clone() };
    if state.send_to(&device, msg).await.is_err() {
        return;
    }

    let claude = match &ctx.engines.claude {
        Ok(c) => c,
        Err(e) => return notice(state, &device, error_code::UNSUPPORTED, e).await,
    };
    let mut messages =
        ctx.histories.lock().unwrap_or_else(|e| e.into_inner()).get(&device).cloned().unwrap_or_default();
    messages.push(Turn { role: Role::User, content: transcript.clone() });

    let (delta_tx, mut delta_rx) = mpsc::channel::<String>(64);
    let (sentence_tx, sentence_rx) = mpsc::channel::<String>(16);
    let ask = claude.stream(&messages, delta_tx);
    let forward = async move {
        let mut splitter = SentenceSplitter::default();
        let mut pending = String::new();
        let mut tick = tokio::time::interval(TEXT_FLUSH);
        tick.tick().await;
        loop {
            tokio::select! {
                d = delta_rx.recv() => match d {
                    Some(d) => {
                        pending.push_str(&d);
                        for s in splitter.push(&d) {
                            let _ = sentence_tx.send(s).await;
                        }
                    }
                    None => break,
                },
                _ = tick.tick() => {
                    if !pending.is_empty() {
                        send_text(state, &device, utterance_id, false, std::mem::take(&mut pending)).await;
                    }
                }
            }
        }
        if let Some(rest) = splitter.finish() {
            let _ = sentence_tx.send(rest).await;
        }
        pending
    };
    let speak = speak(&ctx, device, sentence_rx);
    let (result, pending, ()) = tokio::join!(ask, forward, speak);

    match result {
        Ok(full) => {
            send_text(state, &device, utterance_id, true, pending).await;
            let mut h = ctx.histories.lock().unwrap_or_else(|e| e.into_inner());
            let history = h.entry(device).or_default();
            history.push(Turn { role: Role::User, content: transcript });
            history.push(Turn { role: Role::Assistant, content: full });
            let keep = ctx.history_turns * 2;
            if history.len() > keep {
                history.drain(..history.len() - keep);
            }
        }
        Err(e) => {
            warn!("Claude request failed: {e}");
            send_text(state, &device, utterance_id, true, pending).await;
            let code = if e.is_rate_limit() { error_code::RATE_LIMITED } else { error_code::INTERNAL };
            notice(state, &device, code, &e.user_message()).await;
        }
    }
}

async fn send_text(state: &AppState, device: &Id16, utterance_id: u32, is_final: bool, mut text: String) {
    // Split long text so every message stays far below the 16 KiB message limit.
    loop {
        let head = truncate_utf8(&text, MAX_TEXT).to_string();
        let rest = text.split_off(head.len());
        let last = rest.is_empty();
        let msg = Message::VoiceReplyText { utterance_id, is_final: is_final && last, text: head };
        if state.send_to(device, msg).await.is_err() || last {
            return;
        }
        text = rest;
    }
}

/// Speaks sentences as they arrive; the voice stream opens with the first one (preempting the PC speaker).
async fn speak(ctx: &Ctx, device: Id16, mut sentences: mpsc::Receiver<String>) {
    let tts = match &ctx.engines.tts {
        Ok(Some(t)) => t,
        Ok(None) => return while sentences.recv().await.is_some() {},
        Err(e) => {
            notice(&ctx.state, &device, error_code::UNSUPPORTED, &format!("Spoken replies are off: {e}")).await;
            return while sentences.recv().await.is_some() {};
        }
    };
    let mut stream = None;
    while let Some(sentence) = sentences.recv().await {
        let text = speakable(&sentence);
        if text.is_empty() {
            continue;
        }
        let speech = match tts.synthesize(&text).await {
            Ok(s) => s,
            Err(e) => {
                warn!("{e}");
                notice(&ctx.state, &device, error_code::INTERNAL, "Text-to-speech failed on the PC.").await;
                break;
            }
        };
        if stream.is_none() {
            stream = ctx.audio.start_voice(device).await;
        }
        let Some(s) = &stream else { break };
        if s.tx.send(VoicePcm::Samples { rate: speech.rate, mono: speech.samples }).await.is_err() {
            break; // the PSP stopped the reply (L+R) or started another stream
        }
    }
    // Drain so the text side never blocks on a full channel.
    drop(stream);
    while sentences.recv().await.is_some() {}
}
