//! Widgets, macro slots, and widget taps end to end over localhost, with a recording action executor.

mod common;

use std::sync::Mutex;

use common::*;
use companion_host::actions::{ActionError, Executor, Key, MediaKey};
use companion_host::app::{self, Backends, Features, INPUT_TAP};
use companion_host::audio::capture::GeneratedSource;
use companion_host::widgets::layout::Layout;
use companion_host::widgets::model::{NowPlaying, PlayState, WidgetValue};
use companion_host::widgets::{self, cover_asset};

#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl Recorder {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl Executor for Recorder {
    fn key_chord(&self, keys: &[Key]) -> Result<(), ActionError> {
        self.0.lock().unwrap().push(format!("keys {keys:?}"));
        Ok(())
    }
    fn media_key(&self, key: MediaKey) -> Result<(), ActionError> {
        self.0.lock().unwrap().push(format!("media {key:?}"));
        Ok(())
    }
    fn toggle_mic_mute(&self) -> Result<bool, ActionError> {
        self.0.lock().unwrap().push("mic".into());
        Ok(true)
    }
    fn spawn(&self, program: &str, _: &[String]) -> Result<(), ActionError> {
        self.0.lock().unwrap().push(format!("spawn {program}"));
        Ok(())
    }
    fn open(&self, target: &str) -> Result<(), ActionError> {
        self.0.lock().unwrap().push(format!("open {target}"));
        Ok(())
    }
}

async fn wait_for(rec: &Recorder, n: usize) -> Vec<String> {
    for _ in 0..100 {
        let got = rec.0.lock().unwrap().len();
        if got >= n {
            return rec.take();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    rec.take()
}

fn png_cover() -> Vec<u8> {
    let img = image::RgbImage::from_fn(32, 32, |x, y| image::Rgb([(x * 8) as u8, (y * 8) as u8, 99]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[tokio::test]
async fn widgets_macros_and_taps() {
    let host = start_host("widgets").await;
    let mut cfg = host.state.config();
    cfg.widgets.now_playing = false;
    cfg.streamdeck.enabled = false;
    cfg.widgets.ingest_port = 0;
    cfg.widgets.system_interval_secs = 60;
    let rec = Arc::new(Recorder::default());
    let events = host.state.subscribe_events();
    let backends = Backends {
        exec: rec.clone(),
        capture: Arc::new(GeneratedSource { sample_rate: 48_000, channels: 2, script: vec![], speedup: 1 }),
    };
    let features = Features::start(&host.state, &cfg, Layout::default(), backends).await;
    let hub = features.widgets.clone().unwrap();
    let ingest = widgets::ingest::bind(0).await.unwrap();
    let ingest_addr = ingest.local_addr().unwrap();
    widgets::ingest::serve(ingest, widgets::ingest::router(hub.clone(), "secret".into()));
    tokio::spawn(app::run(events, features));

    let device: Id16 = random_array();
    let pin = host.state.open_pairing();
    let key = pair(&host, &pin, device).await.unwrap();
    let mut s = open_session(&host, device, &key).await;
    match s.recv().await {
        Message::WidgetLayout { format: 1, data } => {
            assert_eq!(data, Layout::default().encode(&cfg.macros));
        }
        other => panic!("{other:?}"),
    }

    // Notifications through the local endpoint; the token is enforced.
    let http = reqwest::Client::new();
    let url = format!("http://{ingest_addr}/notify");
    let body = serde_json::json!({"title": "Build passed", "body": "Tất cả đều ổn", "app": "CI"});
    let r = http.post(&url).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = http.post(&url).header("x-companion-token", "secret").json(&body).send().await.unwrap();
    assert_eq!(r.status(), 204);
    let r = http.post(&url).header("x-companion-token", "secret").body("{}").send().await.unwrap();
    assert_eq!(r.status(), 415, "JSON content type is required");
    match s.recv().await {
        Message::WidgetData { widget_id: 5, format: 1, data } => {
            assert_eq!(data[0], 1);
            assert!(String::from_utf8_lossy(&data).contains("Tất cả đều ổn"));
        }
        other => panic!("{other:?}"),
    }

    let hook = serde_json::json!({"session_id": "x", "hook_event_name": "PreToolUse", "cwd": "/w/anyport", "tool_name": "Bash"});
    let url = format!("http://{ingest_addr}/claude-code");
    http.post(&url).header("x-companion-token", "secret").json(&hook).send().await.unwrap();
    match s.recv().await {
        Message::WidgetData { widget_id: 6, data, .. } => assert!(String::from_utf8_lossy(&data).contains("anyport")),
        other => panic!("{other:?}"),
    }

    // Now playing with a cover: the asset goes first, once; a quick second change is coalesced.
    let cover = cover_asset(&png_cover(), 64).unwrap();
    let np = |state| {
        WidgetValue::NowPlaying(NowPlaying {
            state,
            title: "Song".into(),
            artist: "Artist".into(),
            cover: Some(cover.clone()),
        })
    };
    hub.update(np(PlayState::Playing)).await;
    match s.recv().await {
        Message::WidgetAsset { asset_id, format: 1, data } => {
            assert_eq!(asset_id, cover.id);
            assert_eq!(data, cover.data);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(s.recv().await, Message::WidgetData { widget_id: 2, .. }));
    let t = tokio::time::Instant::now();
    hub.update(np(PlayState::Paused)).await;
    match s.recv().await {
        Message::WidgetData { widget_id: 2, data, .. } => assert_eq!(data[0], PlayState::Paused as u8),
        other => panic!("{other:?}"),
    }
    assert!(t.elapsed() >= Duration::from_millis(800), "held back for the 1 s interval: {:?}", t.elapsed());

    // Macro slot and widget tap run host actions; the PSP only sent IDs.
    s.send(&Message::MacroTrigger { slot: 7 }).await;
    s.send(&Message::InputEvent { kind: INPUT_TAP, page: 0, widget_id: 2, x: 300, y: 60, buttons: 0 }).await;
    s.send(&Message::InputEvent { kind: INPUT_TAP, page: 0, widget_id: 1, x: 10, y: 30, buttons: 0 }).await;
    let ran = wait_for(&rec, 2).await;
    assert_eq!(ran, ["keys [Win, Char('D')]", "media PlayPause"]);

    // A reconnecting PSP replaces its old session and gets the layout, the current values, and the cover again.
    let mut s2 = open_session(&host, device, &key).await;
    assert_eq!(s.recv().await, Message::Bye { reason: ByeReason::Replaced });
    drop(s);
    assert!(matches!(s2.recv().await, Message::WidgetLayout { .. }));
    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.push(s2.recv().await.type_id());
    }
    seen.sort();
    assert_eq!(seen, [0x11, 0x11, 0x11, 0x12], "three widgets with data and one asset");

    // The old session closing afterwards must not unsubscribe the new one.
    tokio::time::sleep(Duration::from_millis(200)).await;
    hub.update(np(PlayState::Stopped)).await;
    match s2.recv().await {
        Message::WidgetData { widget_id: 2, data, .. } => assert_eq!(data[0], PlayState::Stopped as u8),
        other => panic!("{other:?}"),
    }
}
