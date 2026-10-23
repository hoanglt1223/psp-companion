//! Audio streams to the PSP. One stream per PSP at a time: a voice reply preempts the PC speaker, which
//! resumes afterwards if the PSP still wants it. MP3 frames go one per UDP packet; silence sends nothing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use super::capture::CaptureSource;
use super::encode_mp3::{FRAME_US, Mp3Encoder, Resampler, SAMPLE_RATE, remix, to_i16};
use crate::net::handshake::Id16;
use crate::net::message::Message;
use crate::state::{AppState, SendError};

pub const KIND_VOICE: u8 = 0;
pub const KIND_SPEAKER: u8 = 1;
pub const CODEC_MP3: u8 = 1;
pub const STOP_ENDED: u8 = 0;
pub const STOP_USER: u8 = 1;
pub const STOP_PREEMPTED: u8 = 2;

pub const SPEAKER_KBPS: u16 = 96;
pub const VOICE_KBPS: u16 = 48;
/// Generated audio (speech) is sent at most this far ahead of real time, so the PSP buffer never overflows.
pub const VOICE_LEAD: Duration = Duration::from_millis(300);
/// Below this peak level (about -60 dBFS) for longer than the hold time, packets stop.
const SILENCE_PEAK: f32 = 0.001;
const SILENCE_HOLD_US: u64 = 400_000;

/// Opens and closes the packet flow on sustained silence.
#[derive(Default)]
pub struct SilenceGate {
    silent_us: u64,
}

impl SilenceGate {
    /// Returns true while audio should be sent.
    pub fn observe(&mut self, block: &[f32], duration_us: u64) -> bool {
        if block.iter().any(|s| s.abs() > SILENCE_PEAK) {
            self.silent_us = 0;
        } else {
            self.silent_us = self.silent_us.saturating_add(duration_us);
        }
        self.silent_us <= SILENCE_HOLD_US
    }
}

/// PCM in any rate and channel count to timestamped MP3 frames (44.1 kHz).
pub struct Pipeline {
    in_channels: usize,
    in_rate: u32,
    resampler: Resampler,
    enc: Mp3Encoder,
    gate: Option<SilenceGate>,
    open: bool,
    frame_index: u64,
}

impl Pipeline {
    pub fn new(in_rate: u32, in_channels: usize, out_channels: u8, kbps: u16, gate: bool) -> Result<Self, String> {
        if in_rate == 0 || in_channels == 0 {
            return Err("invalid capture format".into());
        }
        Ok(Self {
            in_channels,
            in_rate,
            resampler: Resampler::new(in_rate, SAMPLE_RATE, usize::from(out_channels)),
            enc: Mp3Encoder::new(out_channels, kbps).map_err(|e| e.to_string())?,
            gate: gate.then(SilenceGate::default),
            open: true,
            frame_index: 0,
        })
    }

    fn stamp(&mut self, frames: Vec<Vec<u8>>) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        for f in frames {
            let pts_ms = (self.frame_index * FRAME_US / 1000) as u32;
            self.frame_index += 1;
            // The encoder runs through silence too, so timestamps stay on the real timeline.
            if self.open {
                out.push((pts_ms, f));
            }
        }
        out
    }

    pub fn push(&mut self, block: &[f32]) -> Result<Vec<(u32, Vec<u8>)>, String> {
        let frames = block.len() / self.in_channels;
        let duration_us = frames as u64 * 1_000_000 / u64::from(self.in_rate);
        if let Some(g) = self.gate.as_mut() {
            self.open = g.observe(block, duration_us);
        }
        let mixed = remix(block, self.in_channels, usize::from(self.enc.channels()));
        let pcm = to_i16(&self.resampler.process(&mixed));
        let encoded = self.enc.encode(&pcm).map_err(|e| e.to_string())?;
        Ok(self.stamp(encoded))
    }

    pub fn finish(&mut self) -> Result<Vec<(u32, Vec<u8>)>, String> {
        let encoded = self.enc.flush().map_err(|e| e.to_string())?;
        Ok(self.stamp(encoded))
    }
}

/// Sends packets of one stream, numbering them; rate-limited packets still use up a sequence number,
/// so the PSP sees them as lost and conceals them.
struct Sender {
    state: Arc<AppState>,
    device: Id16,
    stream_id: u8,
    seq: u32,
    dropped: u64,
    last_pts: u32,
}

impl Sender {
    async fn send(&mut self, pts_ms: u32, data: Vec<u8>) -> Result<(), SendError> {
        let msg = Message::AudioPacket { stream_id: self.stream_id, seq: self.seq, pts_ms, data };
        self.last_pts = pts_ms;
        self.seq = self.seq.wrapping_add(1);
        match self.state.send_datagram(&self.device, &msg).await {
            Err(SendError::RateLimited) | Err(SendError::NoUdpPeer) => {
                self.dropped += 1;
                Ok(())
            }
            other => other,
        }
    }
}

/// PCM handed to a voice stream; `rate` may differ per chunk (TTS voices vary).
pub enum VoicePcm {
    Samples { rate: u32, mono: Vec<f32> },
}

pub struct VoiceStream {
    pub stream_id: u8,
    pub tx: mpsc::Sender<VoicePcm>,
}

pub enum AudioCmd {
    SessionUp { device: Id16, session_id: u64 },
    SessionDown { device: Id16, session_id: u64 },
    Request { device: Id16, kind: u8, enable: bool },
    PspStop { device: Id16, stream_id: u8 },
    StartVoice { device: Id16, reply: oneshot::Sender<Option<VoiceStream>> },
    Finished { device: Id16, stream_id: u8 },
}

#[derive(Clone)]
pub struct AudioHandle {
    tx: mpsc::Sender<AudioCmd>,
}

impl AudioHandle {
    pub async fn send(&self, cmd: AudioCmd) {
        let _ = self.tx.send(cmd).await;
    }

    /// Preempts whatever plays on this PSP and opens a voice reply stream.
    pub async fn start_voice(&self, device: Id16) -> Option<VoiceStream> {
        let (reply, rx) = oneshot::channel();
        self.send(AudioCmd::StartVoice { device, reply }).await;
        rx.await.ok().flatten()
    }
}

struct Active {
    id: u8,
    kind: u8,
    cancel: oneshot::Sender<()>,
}

#[derive(Default)]
struct Device {
    session_id: u64,
    speaker_wanted: bool,
    active: Option<Active>,
}

struct Manager {
    state: Arc<AppState>,
    capture: Arc<dyn CaptureSource>,
    handle: AudioHandle,
    devices: HashMap<Id16, Device>,
    next_id: u8,
}

impl Manager {
    fn next_stream_id(&mut self) -> u8 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }

    async fn stop_active(&mut self, device: Id16, reason: Option<u8>) {
        let Some(active) = self.devices.get_mut(&device).and_then(|d| d.active.take()) else { return };
        let _ = active.cancel.send(());
        if let Some(reason) = reason {
            let _ = self.state.send_to(&device, Message::AudioStreamStop { stream_id: active.id, reason }).await;
        }
    }

    fn start_speaker(&mut self, device: Id16) {
        let id = self.next_stream_id();
        let (cancel, cancel_rx) = oneshot::channel();
        if let Some(d) = self.devices.get_mut(&device) {
            d.active = Some(Active { id, kind: KIND_SPEAKER, cancel });
        }
        let (state, capture, handle) = (self.state.clone(), self.capture.clone(), self.handle.clone());
        tokio::spawn(async move {
            let result = run_speaker(state.clone(), device, id, capture, cancel_rx).await;
            if let Err(e) = &result {
                warn!("PC speaker stream ended: {e}");
                let _ = state.send_to(&device, Message::AudioStreamStop { stream_id: id, reason: STOP_ENDED }).await;
            }
            handle.send(AudioCmd::Finished { device, stream_id: id }).await;
        });
    }

    fn start_voice(&mut self, device: Id16) -> VoiceStream {
        let id = self.next_stream_id();
        let (cancel, cancel_rx) = oneshot::channel();
        let (tx, rx) = mpsc::channel(64);
        if let Some(d) = self.devices.get_mut(&device) {
            d.active = Some(Active { id, kind: KIND_VOICE, cancel });
        }
        let (state, handle) = (self.state.clone(), self.handle.clone());
        tokio::spawn(async move {
            if let Err(e) = run_voice(state.clone(), device, id, rx, cancel_rx).await {
                warn!("voice reply stream failed: {e}");
                let _ = state.send_to(&device, Message::AudioStreamStop { stream_id: id, reason: STOP_ENDED }).await;
            }
            handle.send(AudioCmd::Finished { device, stream_id: id }).await;
        });
        VoiceStream { stream_id: id, tx }
    }

    async fn handle(&mut self, cmd: AudioCmd) {
        match cmd {
            AudioCmd::SessionUp { device, session_id } => {
                self.stop_active(device, None).await;
                self.devices.insert(device, Device { session_id, ..Default::default() });
            }
            AudioCmd::SessionDown { device, session_id } => {
                if self.devices.get(&device).is_some_and(|d| d.session_id == session_id) {
                    self.stop_active(device, None).await;
                    self.devices.remove(&device);
                }
            }
            AudioCmd::Request { device, kind, enable } => {
                if kind != KIND_SPEAKER {
                    debug!(kind, "ignoring a stream request for a kind the PSP cannot request");
                    return;
                }
                let Some(d) = self.devices.get_mut(&device) else { return };
                d.speaker_wanted = enable;
                let playing = d.active.as_ref().map(|a| a.kind);
                match (enable, playing) {
                    (true, None) => self.start_speaker(device),
                    (false, Some(KIND_SPEAKER)) => self.stop_active(device, Some(STOP_USER)).await,
                    _ => {} // already playing, or a voice reply is playing and the speaker resumes after it
                }
                info!(enable, "PC speaker requested");
            }
            AudioCmd::PspStop { device, stream_id } => {
                let Some(d) = self.devices.get_mut(&device) else { return };
                if d.active.as_ref().is_some_and(|a| a.id == stream_id) {
                    if d.active.as_ref().is_some_and(|a| a.kind == KIND_SPEAKER) {
                        d.speaker_wanted = false;
                    }
                    self.stop_active(device, None).await;
                    self.resume_speaker(device);
                }
            }
            AudioCmd::StartVoice { device, reply } => {
                if !self.devices.contains_key(&device) {
                    let _ = reply.send(None);
                    return;
                }
                self.stop_active(device, Some(STOP_PREEMPTED)).await;
                let stream = self.start_voice(device);
                let _ = reply.send(Some(stream));
            }
            AudioCmd::Finished { device, stream_id } => {
                let Some(d) = self.devices.get_mut(&device) else { return };
                let Some(active) = d.active.as_ref().filter(|a| a.id == stream_id) else { return };
                if active.kind == KIND_SPEAKER {
                    d.speaker_wanted = false; // a capture failure must not restart in a loop
                }
                d.active = None;
                self.resume_speaker(device);
            }
        }
    }

    fn resume_speaker(&mut self, device: Id16) {
        if self.devices.get(&device).is_some_and(|d| d.speaker_wanted && d.active.is_none()) {
            self.start_speaker(device);
        }
    }
}

pub fn spawn_manager(state: Arc<AppState>, capture: Arc<dyn CaptureSource>) -> AudioHandle {
    let (tx, mut rx) = mpsc::channel(64);
    let handle = AudioHandle { tx };
    let mut m = Manager { state, capture, handle: handle.clone(), devices: HashMap::new(), next_id: 0 };
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            m.handle(cmd).await;
        }
    });
    handle
}

async fn run_speaker(
    state: Arc<AppState>,
    device: Id16,
    stream_id: u8,
    capture: Arc<dyn CaptureSource>,
    mut cancel: oneshot::Receiver<()>,
) -> Result<(), String> {
    let mut cap = tokio::task::spawn_blocking(move || capture.open()).await.map_err(|e| e.to_string())??;
    let mut pipe = Pipeline::new(cap.sample_rate, cap.channels, 2, SPEAKER_KBPS, true)?;
    let start = Message::AudioStreamStart {
        stream_id,
        kind: KIND_SPEAKER,
        codec: CODEC_MP3,
        sample_rate: SAMPLE_RATE,
        channels: 2,
        bitrate_kbps: SPEAKER_KBPS,
    };
    state.send_to(&device, start).await.map_err(|e| e.to_string())?;
    let mut tx = Sender { state, device, stream_id, seq: 0, dropped: 0, last_pts: 0 };
    loop {
        tokio::select! {
            _ = &mut cancel => break,
            block = cap.rx.recv() => {
                let Some(block) = block else { return Err("capture stopped".into()) };
                for (pts, frame) in pipe.push(&block)? {
                    tx.send(pts, frame).await.map_err(|e| e.to_string())?;
                }
            }
        }
    }
    if tx.dropped > 0 {
        info!(dropped = tx.dropped, "speaker packets dropped by the Wi-Fi budget or before UDP registration");
    }
    Ok(())
}

async fn run_voice(
    state: Arc<AppState>,
    device: Id16,
    stream_id: u8,
    mut rx: mpsc::Receiver<VoicePcm>,
    mut cancel: oneshot::Receiver<()>,
) -> Result<(), String> {
    let start = Message::AudioStreamStart {
        stream_id,
        kind: KIND_VOICE,
        codec: CODEC_MP3,
        sample_rate: SAMPLE_RATE,
        channels: 1,
        bitrate_kbps: VOICE_KBPS,
    };
    state.send_to(&device, start).await.map_err(|e| e.to_string())?;
    let mut tx = Sender { state: state.clone(), device, stream_id, seq: 0, dropped: 0, last_pts: 0 };
    let mut pipe: Option<(u32, Pipeline)> = None;
    let began = Instant::now();
    let mut pending: Vec<(u32, Vec<u8>)> = Vec::new();
    loop {
        tokio::select! {
            _ = &mut cancel => return Ok(()),
            chunk = rx.recv() => {
                let Some(VoicePcm::Samples { rate, mono }) = chunk else { break };
                if pipe.as_ref().is_none_or(|(r, _)| *r != rate) {
                    let mut fresh = Pipeline::new(rate, 1, 1, VOICE_KBPS, false)?;
                    if let Some((_, mut old)) = pipe.take() {
                        pending.extend(old.finish()?);
                        fresh.frame_index = old.frame_index;
                    }
                    pipe = Some((rate, fresh));
                }
                let (_, p) = pipe.as_mut().expect("set above");
                pending.extend(p.push(&mono)?);
            }
        }
        if pace(&mut tx, &mut pending, began, &mut cancel).await? {
            return Ok(());
        }
    }
    if let Some((_, mut p)) = pipe {
        pending.extend(p.finish()?);
    }
    if pace(&mut tx, &mut pending, began, &mut cancel).await? {
        return Ok(());
    }
    // Let the last frames play out before saying the stream ended.
    let end = pending_end(&tx, began);
    tokio::select! {
        _ = &mut cancel => return Ok(()),
        _ = tokio::time::sleep_until(end) => {}
    }
    state.send_to(&device, Message::AudioStreamStop { stream_id, reason: STOP_ENDED }).await.map_err(|e| e.to_string())
}

fn pending_end(tx: &Sender, began: Instant) -> Instant {
    began + Duration::from_millis(u64::from(tx.last_pts)) + Duration::from_micros(FRAME_US)
}

/// Sends queued frames no more than VOICE_LEAD ahead of their play time. Returns true if cancelled.
async fn pace(
    tx: &mut Sender,
    pending: &mut Vec<(u32, Vec<u8>)>,
    began: Instant,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<bool, String> {
    for (pts, frame) in pending.drain(..) {
        let due = (began + Duration::from_millis(u64::from(pts))).checked_sub(VOICE_LEAD).unwrap_or(began);
        tokio::select! {
            _ = &mut *cancel => return Ok(true),
            _ = tokio::time::sleep_until(due) => {}
        }
        tx.send(pts, frame).await.map_err(|e| e.to_string())?;
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::encode_mp3::tests::sine;

    #[test]
    fn gate_closes_after_hold_and_reopens_at_once() {
        let mut g = SilenceGate::default();
        let quiet = vec![0.0005f32; 480];
        let loud = vec![0.1f32; 480];
        assert!(g.observe(&loud, 10_000));
        for _ in 0..40 {
            assert!(g.observe(&quiet, 10_000));
        }
        assert!(!g.observe(&quiet, 10_000), "closed after 400 ms");
        assert!(g.observe(&loud, 10_000), "opens on the first sound");
    }

    #[test]
    fn pipeline_timestamps_follow_real_time_across_silence() {
        let mut p = Pipeline::new(48_000, 2, 2, SPEAKER_KBPS, true).unwrap();
        let tone = sine(440.0, 48_000, 2, 1.0);
        let silence = vec![0.0f32; 48_000 * 2 * 2];
        let mut out = Vec::new();
        for chunk in tone.chunks(960).chain(silence.chunks(960)).chain(tone.chunks(960)) {
            out.extend(p.push(chunk).unwrap());
        }
        let total_frames = 4.0 * 44_100.0 / 1152.0;
        assert!((out.len() as f64) < total_frames * 0.75, "silence was not sent: {} frames", out.len());
        let pts: Vec<u32> = out.iter().map(|(t, _)| *t).collect();
        assert!(pts.windows(2).all(|w| w[1] > w[0]));
        let gap = pts.windows(2).map(|w| w[1] - w[0]).max().unwrap();
        assert!((1400..1700).contains(&gap), "a ~1.6 s gap where silence was dropped: {gap} ms");
        assert!(*pts.last().unwrap() > 3800);
        assert!(out.iter().all(|(_, f)| f.len() + 32 <= crate::net::crypto::MAX_DATAGRAM));
    }

    #[test]
    fn rejects_bad_formats() {
        assert!(Pipeline::new(0, 2, 2, 96, true).is_err());
        assert!(Pipeline::new(48_000, 2, 2, 100, true).is_err());
    }
}
