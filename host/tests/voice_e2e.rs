//! The voice assistant end to end over localhost. Claude, speech-to-text and text-to-speech are local HTTP
//! servers speaking the real wire formats: the Claude Messages streaming API (replayed from fixtures
//! written from its documented event format) and OpenAI-compatible audio endpoints.

mod common;

use std::sync::Mutex;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::post;
use common::*;
use companion_host::actions::{ActionError, Executor, Key, MediaKey};
use companion_host::app::{self, Backends, Features};
use companion_host::audio::capture::GeneratedSource;
use companion_host::audio::stream::{KIND_VOICE, STOP_ENDED};
use companion_host::config::Config;
use companion_host::net::message::error_code;
use companion_host::voice::claude::{ClaudeClient, ClaudeError, Role, Turn};
use companion_host::voice::dsp::wav_bytes;
use companion_host::voice::{SttEngine, TtsEngine, adpcm};
use companion_host::widgets::layout::Layout;
use serde_json::Value;
use tokio::sync::mpsc;

const REPLY: &str = include_str!("fixtures/claude_reply.sse");
const OVERLOADED: &str = include_str!("fixtures/claude_overloaded.sse");
const RATE_LIMITED: &str = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Number of requests has exceeded your rate limit"}}"#;
const KEY: &str = "sk-ant-test-key-not-real";

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

/// What the local API answers next, and what it was asked.
#[derive(Default)]
struct Api {
    claude_replies: Mutex<Vec<(u16, &'static str)>>,
    claude_requests: Mutex<Vec<(HeaderMap, Value)>>,
    transcripts: Mutex<Vec<&'static str>>,
    stt_uploads: Mutex<Vec<usize>>,
    tts_inputs: Mutex<Vec<String>>,
}

async fn messages(State(api): State<Arc<Api>>, headers: HeaderMap, body: String) -> Response {
    api.claude_requests.lock().unwrap().push((headers, serde_json::from_str(&body).unwrap()));
    let (status, body) = api.claude_replies.lock().unwrap().remove(0);
    let ctype = if status == 200 { "text/event-stream" } else { "application/json" };
    // Small chunks, so the client sees events split across reads.
    let chunks: Vec<Result<Bytes, std::io::Error>> =
        body.as_bytes().chunks(37).map(|c| Ok(Bytes::copy_from_slice(c))).collect();
    Response::builder()
        .status(status)
        .header("content-type", ctype)
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .unwrap()
}

async fn transcriptions(State(api): State<Arc<Api>>, headers: HeaderMap, body: Bytes) -> Response {
    let ctype = headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(ctype.starts_with("multipart/form-data"), "{ctype}");
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("name=\"file\"") && text.contains("RIFF") && text.contains("name=\"model\""));
    api.stt_uploads.lock().unwrap().push(body.len());
    let t = api.transcripts.lock().unwrap().remove(0);
    Response::new(Body::from(serde_json::json!({ "text": t }).to_string()))
}

async fn speech(State(api): State<Arc<Api>>, Json(v): axum::Json<Value>) -> Response {
    assert_eq!(v["response_format"], "wav");
    api.tts_inputs.lock().unwrap().push(v["input"].as_str().unwrap().to_string());
    // About 40 ms of a 300 Hz tone per character, at 24 kHz like hosted voices.
    let n = v["input"].as_str().unwrap().chars().count() * 960;
    let samples: Vec<f32> = (0..n).map(|i| (i as f32 * 300.0 * std::f32::consts::TAU / 24_000.0).sin() * 0.4).collect();
    Response::new(Body::from(wav_bytes(&samples, 24_000)))
}

use axum::Json;

async fn start_api(api: Arc<Api>) -> String {
    let app = Router::new()
        .route("/v1/messages", post(messages))
        .route("/v1/audio/transcriptions", post(transcriptions))
        .route("/v1/audio/speech", post(speech))
        .with_state(api);
    let l = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

fn voice_config(base: &str, cfg: &mut Config) {
    cfg.widgets.enabled = false;
    let v = &mut cfg.voice;
    v.api_base = base.to_string();
    v.api_key = companion_host::voice::Secret(KEY.into());
    v.stt.engine = SttEngine::Openai;
    v.stt.url = format!("{base}/v1/audio/transcriptions");
    v.stt.api_key_env = "PSPC_TEST_UNSET_KEY_VAR".into();
    v.tts.engine = TtsEngine::Openai;
    v.tts.url = format!("{base}/v1/audio/speech");
    v.tts.api_key_env = "PSPC_TEST_UNSET_KEY_VAR".into();
}

#[tokio::test]
async fn claude_client_streams_and_reports_errors() {
    let api = Arc::new(Api::default());
    api.claude_replies.lock().unwrap().extend([(200, REPLY), (429, RATE_LIMITED), (200, OVERLOADED)]);
    let base = start_api(api.clone()).await;
    let client = ClaudeClient::new(&base, KEY.into(), "claude-opus-5-5".into(), 300, String::new()).unwrap();
    let turns = [Turn { role: Role::User, content: "Xin chào".into() }];

    let (tx, mut rx) = mpsc::channel(64);
    let full = client.stream(&turns, tx).await.unwrap();
    assert_eq!(full, "Xin chào! Tôi là trợ lý giọng nói của bạn. Bạn cần giúp gì?");
    let mut deltas = Vec::new();
    while let Some(d) = rx.recv().await {
        deltas.push(d);
    }
    assert_eq!(deltas.len(), 3);

    let (headers, body) = api.claude_requests.lock().unwrap()[0].clone();
    assert_eq!(headers["x-api-key"], KEY);
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 300);
    assert_eq!(body["messages"][0]["role"], "user");
    assert!(body["system"].as_str().unwrap().contains("Vietnamese or English"));

    let (tx, _rx) = mpsc::channel(64);
    let e = client.stream(&turns, tx).await.unwrap_err();
    assert!(matches!(&e, ClaudeError::Http { status: 429, kind, .. } if kind == "rate_limit_error"), "{e:?}");
    assert!(e.is_rate_limit());

    let (tx, _rx) = mpsc::channel(64);
    let e = client.stream(&turns, tx).await.unwrap_err();
    assert!(matches!(&e, ClaudeError::Stream { kind, .. } if kind == "overloaded_error"), "{e:?}");
    assert!(!format!("{e} {e:?} {client:?}").contains(KEY), "the key never appears in errors or debug output");
}

/// A spoken question: noise, then 1.2 s of a 220 Hz "voice", then noise, at 11.025 kHz.
fn question() -> Vec<i16> {
    let mut seed = 9u32;
    (0..11_025 * 2)
        .map(|i| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let t = i as f32 / 11_025.0;
            let v = if (0.4..1.6).contains(&t) { (t * 220.0 * std::f32::consts::TAU).sin() * 6000.0 } else { 0.0 };
            (v + ((seed >> 16) as f32 / 32768.0 - 1.0) * 200.0) as i16
        })
        .collect()
}

async fn ask(s: &mut Session, utterance_id: u32, pcm: &[i16]) {
    s.send(&Message::VoiceStart { utterance_id, codec: 1, sample_rate: 11_025, channels: 1 }).await;
    // 1024 samples per chunk, about 93 ms, as the PSP sends them while R is held.
    for (seq, chunk) in pcm.chunks(1024).enumerate() {
        let data = adpcm::encode_chunk(chunk, chunk[0], 20);
        s.send(&Message::VoiceChunk { utterance_id, seq: seq as u32, data }).await;
    }
    s.send(&Message::VoiceEnd { utterance_id, reason: 0 }).await;
}

#[tokio::test]
async fn push_to_talk_round_trip_with_history_and_errors() {
    let api = Arc::new(Api::default());
    let base = start_api(api.clone()).await;
    let host = start_host("voice").await;
    let mut cfg = host.state.config();
    voice_config(&base, &mut cfg);
    let events = host.state.subscribe_events();
    let backends = Backends {
        exec: Arc::new(NoActions),
        capture: Arc::new(GeneratedSource { sample_rate: 48_000, channels: 2, script: vec![], speedup: 1 }),
    };
    let features = Features::start(&host.state, &cfg, Layout::default(), backends).await;
    tokio::spawn(app::run(events, features));

    let device: Id16 = random_array();
    let pin = host.state.open_pairing();
    let key = pair(&host, &pin, device).await.unwrap();
    let mut s = open_session(&host, device, &key).await;
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let mut sealer = DatagramSealer::new(&s.keys.udp_c2s, s.session_tag);
    udp.send_to(&sealer.seal(&Message::AudioRegister.encode()).unwrap(), host.audio).await.unwrap();
    assert_eq!(s.recv().await, Message::AudioRegisterAck);
    let mut opener = DatagramOpener::new(&s.keys.udp_s2c);

    // First question.
    api.transcripts.lock().unwrap().push("Xin chào");
    api.claude_replies.lock().unwrap().push((200, REPLY));
    let t0 = tokio::time::Instant::now();
    ask(&mut s, 1, &question()).await;
    assert_eq!(s.recv().await, Message::VoiceTranscript { utterance_id: 1, is_final: true, text: "Xin chào".into() });

    let mut reply = String::new();
    let mut voice_stream = None;
    let mut stopped = false;
    while !(stopped && reply.ends_with("Bạn cần giúp gì?")) {
        match s.recv().await {
            Message::VoiceReplyText { utterance_id: 1, is_final, text } => {
                reply.push_str(&text);
                if is_final {
                    assert_eq!(reply, "Xin chào! Tôi là trợ lý giọng nói của bạn. Bạn cần giúp gì?");
                }
            }
            Message::AudioStreamStart { stream_id, kind: KIND_VOICE, sample_rate: 44_100, channels: 1, .. } => {
                voice_stream = Some((stream_id, t0.elapsed()));
            }
            Message::AudioStreamStop { stream_id, reason: STOP_ENDED } => {
                assert_eq!(Some(stream_id), voice_stream.map(|v| v.0));
                stopped = true;
            }
            other => panic!("{other:?}"),
        }
    }
    let (stream_id, started_after) = voice_stream.expect("spoken reply");
    assert!(started_after < Duration::from_secs(3), "first audio after {started_after:?}");
    let mut buf = [0u8; 2048];
    let mut packets = 0;
    while let Ok(Ok(n)) = timeout(Duration::from_millis(300), udp.recv(&mut buf)).await {
        let plain = opener.open(&buf[..n]).expect("authentic datagram");
        if let Ok(Decoded::Known(Message::AudioPacket { stream_id: id, .. })) = Message::decode(&plain) {
            assert_eq!(id, stream_id);
            packets += 1;
        }
    }
    // The local TTS returns about 40 ms per character: 59 characters is about 2.4 s, 90 frames of 26 ms.
    assert!(packets >= 80, "{packets} voice packets");
    let tts = api.tts_inputs.lock().unwrap().clone();
    assert_eq!(tts, ["Xin chào! Tôi là trợ lý giọng nói của bạn.", "Bạn cần giúp gì?"], "spoken sentence by sentence");
    assert_eq!(api.stt_uploads.lock().unwrap().len(), 1);

    // Second question: the history goes with it.
    api.transcripts.lock().unwrap().push("Thời tiết hôm nay thế nào?");
    api.claude_replies.lock().unwrap().push((429, RATE_LIMITED));
    ask(&mut s, 2, &question()).await;
    assert!(matches!(s.recv().await, Message::VoiceTranscript { utterance_id: 2, .. }));
    loop {
        match s.recv().await {
            Message::VoiceReplyText { utterance_id: 2, is_final: true, .. } => {}
            Message::Error { code, text } => {
                assert_eq!(code, error_code::RATE_LIMITED);
                assert!(text.contains("rate limiting"), "{text}");
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    let (_, body) = api.claude_requests.lock().unwrap()[1].clone();
    let roles: Vec<&str> = body["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "user"]);
    assert_eq!(body["messages"][1]["content"], "Xin chào! Tôi là trợ lý giọng nói của bạn. Bạn cần giúp gì?");

    // Silence is caught before any API call.
    ask(&mut s, 3, &vec![0i16; 11_025]).await;
    match s.recv().await {
        Message::Error { text, .. } => assert!(text.contains("did not hear"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(api.stt_uploads.lock().unwrap().len(), 2, "no upload for silence");

    // A cancelled recording does nothing.
    s.send(&Message::VoiceStart { utterance_id: 4, codec: 1, sample_rate: 11_025, channels: 1 }).await;
    s.send(&Message::VoiceEnd { utterance_id: 4, reason: 1 }).await;
    s.send(&Message::Ping { id: 5, t_us: 0 }).await;
    assert_eq!(s.recv().await, Message::Pong { id: 5, t_us: 0 });
}
