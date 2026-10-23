//! Stream Deck mode end to end: a fake Bitfocus Companion speaking the documented Satellite line protocol
//! (https://companion.free/for-developers/Satellite-API), the real host, and a test client playing the PSP.

mod common;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use common::*;
use companion_host::actions::{ActionError, Executor, Key, MediaKey};
use companion_host::app::{self, Backends, Features};
use companion_host::audio::capture::GeneratedSource;
use companion_host::imaging::Indexed;
use companion_host::streamdeck::bridge::KEY_PAGE_NEXT;
use companion_host::streamdeck::satellite::parse_line;
use companion_host::widgets::layout::Layout;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;

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

const SIZE: usize = 76;

fn key_color(key: u8, page: u8) -> [u8; 3] {
    [key * 12, page * 100, 200 - key * 5]
}

fn key_state(device: &str, key: u8, page: u8) -> String {
    let rgb = key_color(key, page).repeat(SIZE * SIZE);
    format!("KEY-STATE DEVICEID={device} KEY={key} TYPE=BUTTON BITMAP={} PRESSED=0\n", B64.encode(rgb))
}

/// One connection of the fake Companion. Lines it receives (other than PING) go to `seen`.
async fn companion_connection(stream: TcpStream, seen: mpsc::Sender<String>, mut cmds: mpsc::Receiver<String>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    write.write_all(b"BEGIN CompanionVersion=5.2.0+8000-stable ApiVersion=1.15.0\n").await.unwrap();
    write.write_all(b"CAPS SUBSCRIPTIONS=1 NONSQUARE=1 BITMAP_FORMATS=rgb,png,webp ROTARY_AMOUNT=1\n").await.unwrap();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { return };
                if let Some(payload) = line.strip_prefix("PING ") {
                    write.write_all(format!("PONG {payload}\n").as_bytes()).await.unwrap();
                    continue;
                }
                if line.starts_with("ADD-DEVICE ") {
                    let device = param(&line, "DEVICEID");
                    write.write_all(format!("ADD-DEVICE OK DEVICEID={device}\n").as_bytes()).await.unwrap();
                    send_page(&mut write, &device, 0).await;
                } else if line.starts_with("CHANGE-PAGE ") {
                    write.write_all(b"CHANGE-PAGE OK\n").await.unwrap();
                    send_page(&mut write, &param(&line, "DEVICEID"), 1).await;
                }
                let _ = seen.send(line).await;
            }
            cmd = cmds.recv() => match cmd {
                Some(raw) => write.write_all(raw.as_bytes()).await.unwrap(),
                None => return, // drop the connection
            }
        }
    }
}

async fn send_page(write: &mut OwnedWriteHalf, device: &str, page: u8) {
    for key in 0..18 {
        write.write_all(key_state(device, key, page).as_bytes()).await.unwrap();
    }
}

fn param(line: &str, key: &str) -> String {
    line.split(' ').find_map(|t| t.strip_prefix(&format!("{key}="))).unwrap_or_default().trim_matches('"').to_string()
}

/// Waits for the next deck message, skipping keepalive traffic.
async fn next_deck(s: &mut Session) -> Message {
    loop {
        match s.recv().await {
            m @ (Message::DeckLayout { .. } | Message::DeckKeyImage { .. }) => return m,
            Message::Pong { .. } => {}
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn psp_works_as_a_companion_surface() {
    let companion = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = companion.local_addr().unwrap().port();
    let (seen_tx, mut seen) = mpsc::channel(256);
    let (conn_tx, mut conns) = mpsc::channel::<mpsc::Sender<String>>(4);
    tokio::spawn(async move {
        loop {
            let (stream, _) = companion.accept().await.unwrap();
            let (tx, rx) = mpsc::channel(16);
            conn_tx.send(tx).await.unwrap();
            tokio::spawn(companion_connection(stream, seen_tx.clone(), rx));
        }
    });

    let host = start_host("deck").await;
    let mut cfg = host.state.config();
    cfg.widgets.enabled = false;
    cfg.streamdeck.enabled = true;
    cfg.streamdeck.port = port;
    let events = host.state.subscribe_events();
    let backends = Backends {
        exec: Arc::new(NoActions),
        capture: Arc::new(GeneratedSource { sample_rate: 48_000, channels: 2, script: vec![], speedup: 1 }),
    };
    let features = Features::start(&host.state, &cfg, Layout::default(), backends).await;
    tokio::spawn(app::run(events, features));
    let connection = timeout(T, conns.recv()).await.unwrap().unwrap();

    let psp_id: Id16 = random_array();
    let pin = host.state.open_pairing();
    let key = pair(&host, &pin, psp_id).await.unwrap();
    let mut s = open_session(&host, psp_id, &key).await;

    // Registration with the documented simple-mode parameters.
    let add = timeout(T, seen.recv()).await.unwrap().unwrap();
    assert!(add.starts_with("ADD-DEVICE "), "{add}");
    let parsed = parse_line(&add).unwrap();
    for (k, v) in [("KEYS_TOTAL", "18"), ("KEYS_PER_ROW", "6"), ("BITMAPS", "76"), ("PRODUCT_NAME", "PSP Companion")] {
        assert_eq!(parsed.get(k), Some(v), "{k} in {add}");
    }
    let device = param(&add, "DEVICEID");

    // The grid, then key images: one new image per 200 ms, so 18 keys take about 3.6 s.
    let mut layout = next_deck(&mut s).await;
    if layout == (Message::DeckLayout { columns: 0, rows: 0, key_w: 0, key_h: 0 }) {
        layout = next_deck(&mut s).await; // the PSP connected before the handshake finished
    }
    assert_eq!(layout, Message::DeckLayout { columns: 6, rows: 3, key_w: 76, key_h: 76 });
    let t0 = tokio::time::Instant::now();
    let mut got = std::collections::BTreeMap::new();
    let mut in_first_second = 0;
    while got.len() < 18 {
        match next_deck(&mut s).await {
            Message::DeckKeyImage { key, image_id, format: 1, width: 76, height: 76, data } => {
                assert!(!data.is_empty(), "each key is new");
                let img = Indexed::decode(&data).expect("IDX8-LZ4 image");
                assert_eq!(img.palette, vec![key_color(key, 0)], "key {key} keeps its colour");
                assert!(data.len() < 600, "a flat key compresses to a few hundred bytes: {}", data.len());
                got.insert(key, image_id);
                if t0.elapsed() < Duration::from_secs(1) {
                    in_first_second += 1;
                }
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(in_first_second <= 6, "{in_first_second} new images in the first second");

    // Presses and releases are separate (long press works); L/R switch Companion pages.
    s.send(&Message::DeckKeyPress { key: 3, pressed: true }).await;
    s.send(&Message::DeckKeyPress { key: 3, pressed: false }).await;
    s.send(&Message::DeckKeyPress { key: KEY_PAGE_NEXT, pressed: true }).await;
    s.send(&Message::DeckKeyPress { key: 40, pressed: true }).await; // outside the grid: ignored
    let mut lines = Vec::new();
    while lines.len() < 3 {
        lines.push(timeout(T, seen.recv()).await.unwrap().unwrap());
    }
    assert_eq!(lines[0], format!("KEY-PRESS DEVICEID={device} KEY=3 PRESSED=true"));
    assert_eq!(lines[1], format!("KEY-PRESS DEVICEID={device} KEY=3 PRESSED=false"));
    assert_eq!(lines[2], format!("CHANGE-PAGE DEVICEID={device} DIRECTION=1"));

    // Page 1 arrives: all new images, still paced.
    let mut page1 = 0;
    while page1 < 18 {
        if let Message::DeckKeyImage { key, data, .. } = next_deck(&mut s).await {
            let img = Indexed::decode(&data).unwrap();
            assert_eq!(img.palette, vec![key_color(key, 1)]);
            page1 += 1;
        }
    }

    // A single key change shows up within a second.
    let changed = format!("KEY-STATE DEVICEID={device} KEY=5 BITMAP={}\n", B64.encode([9u8, 9, 9].repeat(SIZE * SIZE)));
    let t = tokio::time::Instant::now();
    connection.send(changed).await.unwrap();
    match next_deck(&mut s).await {
        Message::DeckKeyImage { key: 5, data, .. } => {
            assert_eq!(Indexed::decode(&data).unwrap().palette, vec![[9, 9, 9]])
        }
        other => panic!("{other:?}"),
    }
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());

    // Going back to page 0 costs no image data: the PSP has those images cached.
    connection.send((0..18).map(|k| key_state(&device, k, 0)).collect::<String>()).await.unwrap();
    let mut cached = 0;
    while cached < 18 {
        if let Message::DeckKeyImage { key, image_id, data, .. } = next_deck(&mut s).await {
            assert!(data.is_empty(), "key {key} reuses its cached image");
            assert_eq!(Some(&image_id), got.get(&key));
            cached += 1;
        }
    }

    // Companion goes away: the PSP is told, then the host reconnects and registers it again.
    drop(connection);
    assert_eq!(next_deck(&mut s).await, Message::DeckLayout { columns: 0, rows: 0, key_w: 0, key_h: 0 });
    let _connection = timeout(Duration::from_secs(10), conns.recv()).await.unwrap().unwrap();
    let again = timeout(Duration::from_secs(10), seen.recv()).await.unwrap().unwrap();
    assert!(again.starts_with("ADD-DEVICE ") && again.contains(&device), "{again}");
    assert_eq!(next_deck(&mut s).await, Message::DeckLayout { columns: 6, rows: 3, key_w: 76, key_h: 76 });

    // The PSP leaving removes the surface.
    s.send(&Message::Bye { reason: ByeReason::Normal }).await;
    loop {
        let line = timeout(T, seen.recv()).await.unwrap().unwrap();
        if line.starts_with("REMOVE-DEVICE") {
            assert_eq!(line, format!("REMOVE-DEVICE DEVICEID={device}"));
            break;
        }
    }
}
