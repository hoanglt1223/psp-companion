//! Bitfocus Companion Satellite API client (https://companion.free/for-developers/Satellite-API).
//! A line protocol over TCP (default port 16622): `COMMAND [OK|ERROR] KEY=VALUE KEY="value with spaces"`.
//! Simple mode only: each PSP registers as an 18-key (6x3) surface with square bitmaps.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Instant, interval, timeout};
use tracing::{debug, info, warn};

pub const DEFAULT_PORT: u16 = 16622;
pub const COLUMNS: u8 = 6;
pub const ROWS: u8 = 3;
pub const KEYS: u8 = COLUMNS * ROWS;
/// Supported Satellite API range: bitmap sizes arrived in 1.5; a 2.x API may change the protocol.
pub const MIN_API: (u32, u32, u32) = (1, 5, 0);
pub const MAX_API_MAJOR: u32 = 1;
/// CAPS after BEGIN, CHANGE-PAGE, and CAN_CHANGE_PAGE arrived in 1.10.
pub const CAPS_API: (u32, u32, u32) = (1, 10, 0);
const PING_EVERY: Duration = Duration::from_secs(2);
const SILENCE_LIMIT: Duration = Duration::from_secs(10);
/// Lines carry base64 bitmaps; Companion caps lines at 2 MB.
const MAX_LINE: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StreamDeckConfig {
    pub enabled: bool,
    /// The PC running Bitfocus Companion.
    pub host: String,
    pub port: u16,
    /// Square key bitmap size requested from Companion (32-96 px).
    pub bitmap_size: u8,
}

impl Default for StreamDeckConfig {
    fn default() -> Self {
        Self { enabled: false, host: "127.0.0.1".into(), port: DEFAULT_PORT, bitmap_size: 76 }
    }
}

impl StreamDeckConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(32..=96).contains(&self.bitmap_size) {
            return Err("streamdeck.bitmap_size must be 32-96".into());
        }
        if self.host.trim().is_empty() || self.port == 0 {
            return Err("streamdeck.host and streamdeck.port must be set".into());
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LineError {
    #[error("empty line")]
    Empty,
    #[error("unterminated quote")]
    Quote,
    #[error("value for {0} contains a quote or newline")]
    BadValue(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub command: String,
    /// "OK" or "ERROR" for responses.
    pub status: Option<String>,
    /// Keys are upper-cased.
    pub params: BTreeMap<String, String>,
    /// Everything after the command (PING/PONG payloads).
    pub body: String,
}

impl Line {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(String::as_str)
    }

    pub fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some("1" | "true"))
    }
}

pub fn parse_line(raw: &str) -> Result<Line, LineError> {
    let raw = raw.trim_end_matches(['\r', '\n']);
    let (command, body) = raw.split_once(' ').unwrap_or((raw, ""));
    if command.is_empty() {
        return Err(LineError::Empty);
    }
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let (mut in_quote, mut escaped) = (false, false);
    for c in body.chars() {
        match c {
            _ if escaped => {
                cur.push(c);
                escaped = false;
            }
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            ' ' if !in_quote => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if in_quote {
        return Err(LineError::Quote);
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let mut status = None;
    let mut params = BTreeMap::new();
    for (i, t) in tokens.into_iter().enumerate() {
        match t.split_once('=') {
            Some((k, v)) => {
                params.insert(k.to_ascii_uppercase(), v.to_string());
            }
            None if i == 0 && (t == "OK" || t == "ERROR") => status = Some(t),
            None => {}
        }
    }
    Ok(Line { command: command.to_ascii_uppercase(), status, params, body: body.to_string() })
}

pub fn format_line(command: &str, params: &[(&str, String)]) -> Result<String, LineError> {
    let mut out = command.to_string();
    for (k, v) in params {
        if v.contains(['"', '\n', '\r']) {
            return Err(LineError::BadValue((*k).to_string()));
        }
        out.push(' ');
        out.push_str(k);
        out.push('=');
        if v.is_empty() || v.contains(' ') {
            out.push('"');
            out.push_str(v);
            out.push('"');
        } else {
            out.push_str(v);
        }
    }
    out.push('\n');
    Ok(out)
}

pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.split(['.', '-', '+']).map(|p| p.parse::<u32>());
    Some((it.next()?.ok()?, it.next()?.ok()?, it.next().and_then(Result::ok).unwrap_or(0)))
}

pub fn supported(api: (u32, u32, u32)) -> bool {
    api >= MIN_API && api.0 <= MAX_API_MAJOR
}

/// Satellite device ID for a PSP: stable per pairing, unique per PSP.
pub fn device_name(psp: &[u8; 16]) -> String {
    format!("psp-companion-{}", hex::encode(&psp[..6]))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SatEvent {
    Connected {
        api: (u32, u32, u32),
    },
    Disconnected,
    DeviceAdded {
        device: String,
    },
    DeviceError {
        device: String,
        message: String,
    },
    /// Raw RGB, `bitmap_size` squared; None means the key is blank.
    KeyImage {
        device: String,
        key: u8,
        rgb: Option<Vec<u8>>,
    },
    KeysClear {
        device: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SatCmd {
    AddDevice { device: String },
    RemoveDevice { device: String },
    KeyPress { device: String, key: u8, pressed: bool },
    ChangePage { device: String, forward: bool },
}

pub struct SatelliteHandle {
    pub cmds: mpsc::Sender<SatCmd>,
    pub events: mpsc::Receiver<SatEvent>,
}

/// Connects and reconnects forever (back-off 1-30 s), re-registering every PSP after a reconnect.
pub fn spawn(config: StreamDeckConfig) -> SatelliteHandle {
    let (cmd_tx, mut cmd_rx) = mpsc::channel(64);
    let (ev_tx, ev_rx) = mpsc::channel(256);
    tokio::spawn(async move {
        let mut devices: HashSet<String> = HashSet::new();
        let mut backoff = Duration::from_secs(1);
        loop {
            let started = Instant::now();
            match session(&config, &mut cmd_rx, &ev_tx, &mut devices).await {
                Ok(()) => return, // the bridge has gone away
                Err(e) => debug!("Companion satellite connection: {e}"),
            }
            if ev_tx.send(SatEvent::Disconnected).await.is_err() {
                return;
            }
            if started.elapsed() > Duration::from_secs(30) {
                backoff = Duration::from_secs(1);
            }
            // Keep tracking registrations while waiting to reconnect.
            let wait = tokio::time::sleep(backoff);
            tokio::pin!(wait);
            loop {
                tokio::select! {
                    _ = &mut wait => break,
                    cmd = cmd_rx.recv() => match cmd {
                        Some(SatCmd::AddDevice { device }) => { devices.insert(device); }
                        Some(SatCmd::RemoveDevice { device }) => { devices.remove(&device); }
                        Some(_) => {}
                        None => return,
                    }
                }
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });
    SatelliteHandle { cmds: cmd_tx, events: ev_rx }
}

fn add_device_line(device: &str, size: u8, api: (u32, u32, u32)) -> String {
    let mut p = vec![
        ("DEVICEID", device.to_string()),
        ("PRODUCT_NAME", "PSP Companion".to_string()),
        ("KEYS_TOTAL", KEYS.to_string()),
        ("KEYS_PER_ROW", COLUMNS.to_string()),
        ("BITMAPS", size.to_string()),
        ("COLORS", "false".to_string()),
        ("TEXT", "false".to_string()),
        ("BRIGHTNESS", "false".to_string()),
    ];
    if api >= CAPS_API {
        p.push(("CAN_CHANGE_PAGE", "Switch pages with the PSP L and R buttons".to_string()));
    }
    format_line("ADD-DEVICE", &p).expect("fixed values have no quotes")
}

fn command_line(cmd: &SatCmd, api: (u32, u32, u32)) -> Option<String> {
    let line = match cmd {
        SatCmd::AddDevice { .. } => return None,
        SatCmd::RemoveDevice { device } => format_line("REMOVE-DEVICE", &[("DEVICEID", device.clone())]),
        SatCmd::KeyPress { device, key, pressed } => format_line(
            "KEY-PRESS",
            &[("DEVICEID", device.clone()), ("KEY", key.to_string()), ("PRESSED", pressed.to_string())],
        ),
        SatCmd::ChangePage { device, forward } => {
            if api < CAPS_API {
                return None;
            }
            format_line("CHANGE-PAGE", &[("DEVICEID", device.clone()), ("DIRECTION", u8::from(*forward).to_string())])
        }
    };
    line.ok()
}

fn decode_bitmap(v: &str, size: u8) -> Option<Vec<u8>> {
    if v.starts_with("data:") {
        return None; // compressed formats come only when requested, and this client never requests them
    }
    let rgb = B64.decode(v).ok()?;
    (rgb.len() == usize::from(size) * usize::from(size) * 3).then_some(rgb)
}

enum Handshake {
    WaitBegin,
    /// API 1.10+: CAPS follows BEGIN and must arrive before devices are added.
    WaitCaps((u32, u32, u32)),
    Ready((u32, u32, u32)),
}

async fn write_line(w: &mut tokio::net::tcp::OwnedWriteHalf, line: &str) -> Result<(), String> {
    w.write_all(line.as_bytes()).await.map_err(|e| e.to_string())
}

/// Reads one line of at most MAX_LINE bytes.
async fn read_line<R: AsyncBufReadExt + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> Result<(), String> {
    buf.clear();
    loop {
        let available = r.fill_buf().await.map_err(|e| e.to_string())?;
        if available.is_empty() {
            return Err("Companion closed the connection".into());
        }
        let (take, done) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        if buf.len() + take > MAX_LINE {
            return Err("Companion sent a line longer than 2 MB".into());
        }
        buf.extend_from_slice(&available[..take]);
        r.consume(take);
        if done {
            return Ok(());
        }
    }
}

/// One connection. Returns Ok(()) only when the bridge dropped its command channel.
async fn session(
    config: &StreamDeckConfig,
    cmds: &mut mpsc::Receiver<SatCmd>,
    events: &mpsc::Sender<SatEvent>,
    devices: &mut HashSet<String>,
) -> Result<(), String> {
    let stream = timeout(Duration::from_secs(5), TcpStream::connect((config.host.as_str(), config.port)))
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|e| format!("cannot connect to {}:{}: {e}", config.host, config.port))?;
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    let mut hs = Handshake::WaitBegin;
    let mut last_rx = Instant::now();
    let mut ping = interval(PING_EVERY);

    loop {
        tokio::select! {
            r = read_line(&mut reader, &mut buf) => {
                r?;
                last_rx = Instant::now();
                let text = String::from_utf8_lossy(&buf).into_owned();
                let Ok(line) = parse_line(&text) else { continue };
                let mut became_ready = None;
                match line.command.as_str() {
                    "BEGIN" => match line.get("APIVERSION").and_then(parse_version) {
                        Some(v) if supported(v) => {
                            info!(api = ?v, companion = line.get("COMPANIONVERSION").unwrap_or("?"), "connected to Bitfocus Companion");
                            if v >= CAPS_API {
                                hs = Handshake::WaitCaps(v);
                            } else {
                                became_ready = Some(v);
                            }
                        }
                        other => {
                            warn!("unsupported Companion Satellite API {other:?}; this host supports 1.5 to 1.x");
                            return Err("unsupported API version".into());
                        }
                    },
                    "CAPS" => {
                        if let Handshake::WaitCaps(v) = hs {
                            became_ready = Some(v);
                        }
                    }
                    "PING" => write_line(&mut write, &format!("PONG {}\n", line.body)).await?,
                    "ADD-DEVICE" => {
                        let device = line.get("DEVICEID").unwrap_or_default().to_string();
                        let ev = if line.status.as_deref() == Some("OK") {
                            SatEvent::DeviceAdded { device }
                        } else {
                            let message = line.get("MESSAGE").unwrap_or("unknown error").to_string();
                            SatEvent::DeviceError { device, message }
                        };
                        let _ = events.send(ev).await;
                    }
                    "KEY-STATE" => {
                        let key = line.get("KEY").and_then(|k| k.parse::<u8>().ok()).filter(|k| *k < KEYS);
                        let (Some(device), Some(key)) = (line.get("DEVICEID"), key) else { continue };
                        let rgb = line.get("BITMAP").and_then(|b| decode_bitmap(b, config.bitmap_size));
                        let _ = events.send(SatEvent::KeyImage { device: device.to_string(), key, rgb }).await;
                    }
                    "KEYS-CLEAR" => {
                        if let Some(device) = line.get("DEVICEID") {
                            let _ = events.send(SatEvent::KeysClear { device: device.to_string() }).await;
                        }
                    }
                    "ERROR" => warn!("Companion: {}", line.get("MESSAGE").unwrap_or(&line.body)),
                    // PONG, BRIGHTNESS, LOCKED-STATE, VARIABLE-VALUE, DEVICE-CONFIG, replies to our commands.
                    _ => {}
                }
                if let Some(v) = became_ready {
                    hs = Handshake::Ready(v);
                    let _ = events.send(SatEvent::Connected { api: v }).await;
                    for d in devices.iter() {
                        write_line(&mut write, &add_device_line(d, config.bitmap_size, v)).await?;
                    }
                }
            }
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                let api = match hs { Handshake::Ready(v) => Some(v), _ => None };
                match &cmd {
                    SatCmd::AddDevice { device } => {
                        devices.insert(device.clone());
                        if let Some(v) = api {
                            write_line(&mut write, &add_device_line(device, config.bitmap_size, v)).await?;
                        }
                    }
                    SatCmd::RemoveDevice { device } => {
                        devices.remove(device);
                    }
                    _ => {}
                }
                if let Some(line) = api.and_then(|v| command_line(&cmd, v)) {
                    write_line(&mut write, &line).await?;
                }
            }
            _ = ping.tick() => {
                if last_rx.elapsed() > SILENCE_LIMIT {
                    return Err("Companion stopped answering".into());
                }
                write_line(&mut write, "PING psp-companion\n").await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines() {
        let l = parse_line("BEGIN CompanionVersion=4.2.1+7000-stable ApiVersion=1.10.0\r\n").unwrap();
        assert_eq!(l.command, "BEGIN");
        assert_eq!(l.get("APIVERSION"), Some("1.10.0"));
        let l = parse_line(r#"ADD-DEVICE ERROR DEVICEID=psp-1 MESSAGE="Device already \"added\"""#).unwrap();
        assert_eq!(l.status.as_deref(), Some("ERROR"));
        assert_eq!(l.get("MESSAGE"), Some("Device already \"added\""));
        let l = parse_line("KEY-STATE DEVICEID=00000 KEY=0 BITMAP=abc= PRESSED=1").unwrap();
        assert!(l.flag("PRESSED"));
        assert_eq!(l.get("BITMAP"), Some("abc="), "base64 padding keeps its '='");
        assert_eq!(parse_line("PING 12 34").unwrap().body, "12 34");
        assert_eq!(parse_line("X A=\"open"), Err(LineError::Quote));
        assert_eq!(parse_line(""), Err(LineError::Empty));
    }

    #[test]
    fn formats_lines() {
        let l = format_line("KEY-PRESS", &[("DEVICEID", "d".into()), ("KEY", "3".into()), ("PRESSED", "true".into())]);
        assert_eq!(l.unwrap(), "KEY-PRESS DEVICEID=d KEY=3 PRESSED=true\n");
        assert_eq!(format_line("X", &[("NAME", "PSP Companion".into())]).unwrap(), "X NAME=\"PSP Companion\"\n");
        assert!(format_line("X", &[("A", "q\"".into())]).is_err());
        let add = add_device_line("psp-companion-0102", 76, (1, 10, 0));
        let parsed = parse_line(&add).unwrap();
        assert_eq!(parsed.get("KEYS_TOTAL"), Some("18"));
        assert_eq!(parsed.get("KEYS_PER_ROW"), Some("6"));
        assert_eq!(parsed.get("BITMAPS"), Some("76"));
        assert!(parsed.get("CAN_CHANGE_PAGE").is_some());
        assert!(parse_line(&add_device_line("d", 76, (1, 7, 0))).unwrap().get("CAN_CHANGE_PAGE").is_none());
    }

    #[test]
    fn version_range() {
        assert_eq!(parse_version("1.15.0"), Some((1, 15, 0)));
        assert_eq!(parse_version("1.5"), Some((1, 5, 0)));
        assert!(supported((1, 5, 0)) && supported((1, 15, 2)));
        assert!(!supported((1, 4, 9)) && !supported((2, 0, 0)));
        assert_eq!(parse_version("x"), None);
        assert!(command_line(&SatCmd::ChangePage { device: "d".into(), forward: true }, (1, 9, 0)).is_none());
    }

    #[test]
    fn bitmaps() {
        let rgb = vec![7u8; 4 * 4 * 3];
        assert_eq!(decode_bitmap(&B64.encode(&rgb), 4), Some(rgb));
        assert_eq!(decode_bitmap(&B64.encode([1, 2, 3]), 4), None, "wrong size");
        assert_eq!(decode_bitmap("data:image/png;base64,AAAA", 4), None);
    }
}
