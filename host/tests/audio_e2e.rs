//! PC speaker and voice streams end to end over localhost, with a generated signal instead of loopback.

mod common;

use common::*;
use companion_host::actions::{ActionError, Executor, Key, MediaKey};
use companion_host::app::{self, Backends, Features};
use companion_host::audio::capture::GeneratedSource;
use companion_host::audio::stream::{
    AudioHandle, KIND_SPEAKER, KIND_VOICE, STOP_ENDED, STOP_PREEMPTED, STOP_USER, VoicePcm,
};
use companion_host::widgets::layout::Layout;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

struct NoActions;

impl Executor for NoActions {
    fn key_chord(&self, _: &[Key]) -> Result<(), ActionError> {
        Err(ActionError::Unsupported)
    }
    fn media_key(&self, _: MediaKey) -> Result<(), ActionError> {
        Err(ActionError::Unsupported)
    }
    fn toggle_mic_mute(&self) -> Result<bool, ActionError> {
        Err(ActionError::Unsupported)
    }
    fn spawn(&self, _: &str, _: &[String]) -> Result<(), ActionError> {
        Err(ActionError::Unsupported)
    }
    fn open(&self, _: &str) -> Result<(), ActionError> {
        Err(ActionError::Unsupported)
    }
}

fn decode_mono(mp3: &[u8]) -> Vec<f32> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(mp3.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .unwrap();
    let track = format.default_track(TrackType::Audio).unwrap().clone();
    let params = track.codec_params.as_ref().unwrap().audio().unwrap().clone();
    let mut dec =
        symphonia::default::get_codecs().make_audio_decoder(&params, &AudioDecoderOptions::default()).unwrap();
    let mut out = Vec::new();
    while let Ok(Some(packet)) = format.next_packet() {
        let buf = dec.decode(&packet).unwrap();
        let ch = buf.spec().channels().count();
        let mut v = vec![0f32; buf.samples_interleaved()];
        buf.copy_to_slice_interleaved(&mut v);
        out.extend(v.chunks_exact(ch).map(|f| f[0]));
    }
    out
}

fn tone_power(x: &[f32], freq: f32) -> f32 {
    let w = std::f32::consts::TAU * freq / 44_100.0;
    let (mut s1, mut s2) = (0f32, 0f32);
    for &v in x {
        let s = v + 2.0 * w.cos() * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2).sqrt() / x.len() as f32
}

struct Psp {
    s: Session,
    udp: UdpSocket,
    opener: DatagramOpener,
}

impl Psp {
    /// Next audio packet: (stream_id, seq, pts_ms, data, datagram bytes).
    async fn packet(&mut self, wait: Duration) -> Option<(u8, u32, u32, Vec<u8>, usize)> {
        let mut buf = [0u8; 2048];
        let n = timeout(wait, self.udp.recv(&mut buf)).await.ok()?.unwrap();
        match Message::decode(&self.opener.open(&buf[..n]).unwrap()).unwrap() {
            Decoded::Known(Message::AudioPacket { stream_id, seq, pts_ms, data }) => {
                Some((stream_id, seq, pts_ms, data, n))
            }
            other => panic!("{other:?}"),
        }
    }

    async fn drain_until_quiet(&mut self) -> usize {
        let mut n = 0;
        while self.packet(Duration::from_millis(300)).await.is_some() {
            n += 1;
        }
        n
    }
}

async fn setup(script: Vec<(f32, f32)>, speedup: u32) -> (Host, Psp, AudioHandle, Id16) {
    let host = start_host(&format!("audio-{speedup}-{}", script.len())).await;
    let mut cfg = host.state.config();
    cfg.widgets.enabled = false;
    cfg.streamdeck.enabled = false;
    let events = host.state.subscribe_events();
    let backends = Backends {
        exec: Arc::new(NoActions),
        capture: Arc::new(GeneratedSource { sample_rate: 48_000, channels: 2, script, speedup }),
    };
    let features = Features::start(&host.state, &cfg, Layout::default(), backends).await;
    let audio = features.audio.clone();
    let device: Id16 = random_array();
    let pin = host.state.open_pairing();
    let key = pair(&host, &pin, device).await.unwrap();
    let mut s = open_session(&host, device, &key).await;
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let mut sealer = DatagramSealer::new(&s.keys.udp_c2s, s.session_tag);
    udp.send_to(&sealer.seal(&Message::AudioRegister.encode()).unwrap(), host.audio).await.unwrap();
    assert_eq!(s.recv().await, Message::AudioRegisterAck);
    let opener = DatagramOpener::new(&s.keys.udp_s2c);
    tokio::spawn(app::run(events, features));
    (host, Psp { s, udp, opener }, audio, device)
}

#[tokio::test]
async fn speaker_streams_mp3_and_skips_silence() {
    // 1 s tone, 1.5 s silence, 1 s tone, delivered at 2x speed.
    let (_host, mut psp, _audio, _device) = setup(vec![(1000.0, 1.0), (0.0, 1.5), (1000.0, 1.0), (0.0, 30.0)], 2).await;
    psp.s.send(&Message::AudioStreamRequest { kind: KIND_SPEAKER, enable: true }).await;
    let stream_id = match psp.s.recv().await {
        Message::AudioStreamStart { stream_id, kind, codec: 1, sample_rate: 44_100, channels: 2, bitrate_kbps: 96 } => {
            assert_eq!(kind, KIND_SPEAKER);
            stream_id
        }
        other => panic!("{other:?}"),
    };

    let mut packets = Vec::new();
    while let Some(p) = psp.packet(Duration::from_millis(1500)).await {
        packets.push(p);
        if packets.len() > 400 {
            break;
        }
    }
    assert!(packets.iter().all(|p| p.0 == stream_id));
    let seqs: Vec<u32> = packets.iter().map(|p| p.1).collect();
    assert_eq!(seqs, (0..seqs.len() as u32).collect::<Vec<_>>(), "consecutive, nothing lost on localhost");
    let pts: Vec<u32> = packets.iter().map(|p| p.2).collect();
    let gap = pts.windows(2).map(|w| w[1] - w[0]).max().unwrap();
    assert!(gap > 900, "no packets during the silence; largest pts gap {gap} ms");
    // Two seconds of tone plus the 400 ms hold, at 26 ms per packet.
    assert!((80..=110).contains(&packets.len()), "{} packets", packets.len());

    let mp3: Vec<u8> = packets.iter().flat_map(|p| p.3.clone()).collect();
    let audio = decode_mono(&mp3);
    let w = &audio[4410..4410 + 22_050];
    assert!(tone_power(w, 1000.0) > 0.2, "the tone survives");

    // Wire rate while sound plays: 96 kbps of MP3 plus per-packet headers, well under the 300 kbps budget.
    let tone_packets: Vec<_> = packets.iter().filter(|p| p.2 < 1000).collect();
    let bytes: usize = tone_packets.iter().map(|p| p.4 + 28).sum();
    let secs = f64::from(tone_packets.last().unwrap().2 - tone_packets[0].2) / 1000.0 + 0.026;
    let kbps = bytes as f64 * 8.0 / 1000.0 / secs;
    assert!((96.0..130.0).contains(&kbps), "{kbps:.1} kbps on the wire");

    psp.s.send(&Message::AudioStreamRequest { kind: KIND_SPEAKER, enable: false }).await;
    assert_eq!(psp.s.recv().await, Message::AudioStreamStop { stream_id, reason: STOP_USER });
}

#[tokio::test]
async fn voice_preempts_the_speaker_which_then_resumes() {
    let (_host, mut psp, audio, device) = setup(vec![(440.0, 30.0)], 1).await;
    psp.s.send(&Message::AudioStreamRequest { kind: KIND_SPEAKER, enable: true }).await;
    let Message::AudioStreamStart { stream_id: speaker, .. } = psp.s.recv().await else { panic!() };
    assert!(psp.packet(Duration::from_secs(2)).await.is_some());

    let voice = audio.start_voice(device).await.expect("voice stream");
    assert_eq!(psp.s.recv().await, Message::AudioStreamStop { stream_id: speaker, reason: STOP_PREEMPTED });
    match psp.s.recv().await {
        Message::AudioStreamStart { stream_id, kind, channels: 1, bitrate_kbps: 48, .. } => {
            assert_eq!((stream_id, kind), (voice.stream_id, KIND_VOICE));
        }
        other => panic!("{other:?}"),
    }
    // Half a second of 22.05 kHz speech-like audio, handed over faster than real time.
    let mono: Vec<f32> =
        (0..11_025).map(|i| (i as f32 * 300.0 * std::f32::consts::TAU / 22_050.0).sin() * 0.4).collect();
    let started = tokio::time::Instant::now();
    voice.tx.send(VoicePcm::Samples { rate: 22_050, mono }).await.unwrap();
    let voice_id = voice.stream_id;
    drop(voice.tx);

    let mut voice_packets = Vec::new();
    let mut first_at = None;
    while let Some(p) = psp.packet(Duration::from_secs(2)).await {
        if p.0 == voice_id {
            first_at.get_or_insert_with(|| started.elapsed());
            voice_packets.push(p);
        } else if !voice_packets.is_empty() {
            break; // the speaker is back
        }
    }
    assert!((17..=21).contains(&voice_packets.len()), "{} voice packets for 0.5 s", voice_packets.len());
    assert!(first_at.unwrap() < Duration::from_millis(300), "first voice audio after {:?}", first_at.unwrap());
    let voice_bytes: Vec<u8> = voice_packets.iter().flat_map(|p| p.3.clone()).collect();
    let decoded = decode_mono(&voice_bytes);
    let windows: Vec<f32> = decoded[2304..].as_chunks::<1152>().0.iter().map(|w| tone_power(w, 300.0)).collect();
    assert!(windows.iter().sum::<f32>() / windows.len() as f32 > 0.1, "the voice audio survives");

    // After the reply has played out, the voice stream ends and the speaker comes back.
    assert_eq!(psp.s.recv().await, Message::AudioStreamStop { stream_id: voice_id, reason: STOP_ENDED });
    let resumed = match psp.s.recv().await {
        Message::AudioStreamStart { stream_id, kind: KIND_SPEAKER, .. } => stream_id,
        other => panic!("{other:?}"),
    };

    // The PSP stopping the speaker itself turns it off without a reply.
    psp.s.send(&Message::AudioStreamStop { stream_id: resumed, reason: STOP_USER }).await;
    psp.drain_until_quiet().await;
    assert!(psp.packet(Duration::from_millis(500)).await.is_none());
    assert!(timeout(Duration::from_millis(300), psp.s.recv()).await.is_err(), "no reply to a PSP stop");
}
