//! Secondary-screen widgets: data sources push values into the hub, which sends a widget to every PSP
//! only when its encoded value changed, at most once per second per widget.

pub mod calendar;
pub mod ingest;
pub mod layout;
#[cfg(windows)]
pub mod media;
pub mod model;
pub mod system;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::debug;

use crate::imaging::FORMAT_IDX8_LZ4;
use crate::net::handshake::Id16;
use crate::net::message::Message;
use crate::state::AppState;
use layout::Layout;
use model::{Asset, FORMAT_BINARY, WidgetKind, WidgetValue};

pub const MIN_INTERVAL: Duration = Duration::from_secs(1);
const HUB_QUEUE: usize = 64;

#[derive(Debug)]
pub enum HubCmd {
    Update(WidgetValue),
    SessionUp(Id16, u64),
    /// Ignored unless the session ID is the current one (a replaced session closes after its successor opens).
    SessionDown(Id16, u64),
}

#[derive(Clone)]
pub struct WidgetHandle {
    tx: mpsc::Sender<HubCmd>,
}

impl WidgetHandle {
    pub async fn update(&self, v: WidgetValue) {
        let _ = self.tx.send(HubCmd::Update(v)).await;
    }

    /// For sources that run on their own OS thread. Returns false once the hub has stopped.
    pub fn blocking_update(&self, v: WidgetValue) -> bool {
        self.tx.blocking_send(HubCmd::Update(v)).is_ok()
    }

    pub async fn send(&self, cmd: HubCmd) {
        let _ = self.tx.send(cmd).await;
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

#[derive(Default)]
struct Entry {
    sent: Option<Vec<u8>>,
    sent_at: Option<Instant>,
    pending: Option<Vec<u8>>,
}

/// Per-widget change detection and rate limiting.
#[derive(Default)]
pub struct Coalescer {
    entries: HashMap<u16, Entry>,
}

impl Coalescer {
    /// Returns the bytes to send now, or None when unchanged or held back until the interval passes.
    pub fn offer(&mut self, id: u16, bytes: Vec<u8>, now: Instant) -> Option<Vec<u8>> {
        let e = self.entries.entry(id).or_default();
        if e.sent.as_ref() == Some(&bytes) {
            e.pending = None;
            return None;
        }
        if e.sent_at.is_some_and(|t| now.duration_since(t) < MIN_INTERVAL) {
            e.pending = Some(bytes);
            return None;
        }
        e.sent = Some(bytes.clone());
        e.sent_at = Some(now);
        e.pending = None;
        Some(bytes)
    }

    pub fn due(&mut self, now: Instant) -> Vec<(u16, Vec<u8>)> {
        let mut out = Vec::new();
        for (&id, e) in &mut self.entries {
            if e.pending.is_some() && e.sent_at.is_none_or(|t| now.duration_since(t) >= MIN_INTERVAL) {
                let bytes = e.pending.take().expect("checked");
                e.sent = Some(bytes.clone());
                e.sent_at = Some(now);
                out.push((id, bytes));
            }
        }
        out.sort_by_key(|(id, _)| *id);
        out
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.entries.values().filter(|e| e.pending.is_some()).filter_map(|e| e.sent_at.map(|t| t + MIN_INTERVAL)).min()
    }

    pub fn current(&self, id: u16) -> Option<&Vec<u8>> {
        self.entries.get(&id).and_then(|e| e.sent.as_ref())
    }
}

struct Hub {
    state: Arc<AppState>,
    layout: Arc<Layout>,
    layout_msg: Vec<u8>,
    latest: HashMap<WidgetKind, WidgetValue>,
    coalescer: Coalescer,
    /// Asset IDs each connected PSP already has.
    sessions: HashMap<Id16, HashSet<u64>>,
    session_ids: HashMap<Id16, u64>,
}

impl Hub {
    fn asset_for(&self, id: u16) -> Option<Arc<Asset>> {
        let kind = self.layout.widget(id)?.kind;
        self.latest.get(&kind)?.asset().cloned()
    }

    async fn send_to(&mut self, device: Id16, msg: Message) {
        if self.state.send_to(&device, msg).await.is_err() {
            debug!("widget update for a closed session");
            self.sessions.remove(&device);
        }
    }

    async fn push(&mut self, device: Id16, widget_id: u16, data: Vec<u8>) {
        if let Some(asset) = self.asset_for(widget_id) {
            let has = self.sessions.get(&device).is_some_and(|s| s.contains(&asset.id));
            if !has {
                let msg = Message::WidgetAsset { asset_id: asset.id, format: asset.format, data: asset.data.clone() };
                self.send_to(device, msg).await;
                if let Some(s) = self.sessions.get_mut(&device) {
                    s.insert(asset.id);
                }
            }
        }
        self.send_to(device, Message::WidgetData { widget_id, format: FORMAT_BINARY, data }).await;
    }

    async fn broadcast(&mut self, widget_id: u16, data: Vec<u8>) {
        let devices: Vec<Id16> = self.sessions.keys().copied().collect();
        for d in devices {
            self.push(d, widget_id, data.clone()).await;
        }
    }

    async fn handle(&mut self, cmd: HubCmd) {
        match cmd {
            HubCmd::Update(v) => {
                let kind = v.kind();
                let bytes = v.encode();
                self.latest.insert(kind, v);
                let ids: Vec<u16> = self.layout.widgets.iter().filter(|d| d.kind == kind).map(|d| d.id).collect();
                for id in ids {
                    if let Some(b) = self.coalescer.offer(id, bytes.clone(), Instant::now()) {
                        self.broadcast(id, b).await;
                    }
                }
            }
            HubCmd::SessionUp(device, session_id) => {
                self.session_ids.insert(device, session_id);
                self.sessions.insert(device, HashSet::new());
                self.send_to(device, Message::WidgetLayout { format: FORMAT_BINARY, data: self.layout_msg.clone() })
                    .await;
                let current: Vec<(u16, Vec<u8>)> = self
                    .layout
                    .widgets
                    .iter()
                    .filter_map(|d| self.coalescer.current(d.id).map(|b| (d.id, b.clone())))
                    .collect();
                for (id, b) in current {
                    self.push(device, id, b).await;
                }
            }
            HubCmd::SessionDown(device, session_id) => {
                if self.session_ids.get(&device) != Some(&session_id) {
                    return;
                }
                self.session_ids.remove(&device);
                self.sessions.remove(&device);
            }
        }
    }
}

/// Starts the hub task. `layout_msg` is the encoded `WidgetLayout` (pages, widgets, macro labels).
pub fn spawn_hub(state: Arc<AppState>, layout: Arc<Layout>, layout_msg: Vec<u8>) -> WidgetHandle {
    let (tx, mut rx) = mpsc::channel(HUB_QUEUE);
    let mut hub = Hub {
        state,
        layout,
        layout_msg,
        latest: HashMap::new(),
        coalescer: Coalescer::default(),
        sessions: HashMap::new(),
        session_ids: HashMap::new(),
    };
    tokio::spawn(async move {
        loop {
            let deadline = hub.coalescer.next_deadline();
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(c) => hub.handle(c).await,
                    None => break,
                },
                _ = async { tokio::time::sleep_until(deadline.expect("guarded")).await }, if deadline.is_some() => {
                    for (id, b) in hub.coalescer.due(Instant::now()) {
                        hub.broadcast(id, b).await;
                    }
                }
            }
        }
    });
    WidgetHandle { tx }
}

/// IDX8-LZ4 asset from an encoded image file (cover art).
pub fn cover_asset(image_bytes: &[u8], size: u32) -> Option<Arc<Asset>> {
    let img = crate::imaging::fit_encoded(image_bytes, size, size).ok()?;
    let data = img.encode();
    Some(Arc::new(Asset { id: crate::imaging::image_id(&data), format: FORMAT_IDX8_LZ4, data }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::crypto::TAG_LEN;
    use crate::net::frame::LEN_PREFIX;
    use crate::net::ratelimit::TCP_OVERHEAD;
    use model::{ClaudeSession, ClaudeState, SystemStats};

    #[tokio::test(start_paused = true)]
    async fn coalesces_to_one_update_per_second() {
        let mut c = Coalescer::default();
        let t0 = Instant::now();
        assert_eq!(c.offer(1, vec![1], t0), Some(vec![1]));
        assert_eq!(c.offer(1, vec![1], t0), None, "unchanged");
        assert_eq!(c.offer(1, vec![2], t0 + Duration::from_millis(100)), None, "held back");
        assert_eq!(c.offer(1, vec![3], t0 + Duration::from_millis(200)), None, "replaces pending");
        assert_eq!(c.next_deadline(), Some(t0 + MIN_INTERVAL));
        assert!(c.due(t0 + Duration::from_millis(900)).is_empty());
        assert_eq!(c.due(t0 + MIN_INTERVAL), vec![(1, vec![3])]);
        assert_eq!(c.next_deadline(), None);
        assert_eq!(c.offer(1, vec![4], t0 + Duration::from_millis(1100)), None);
        assert_eq!(c.offer(1, vec![3], t0 + Duration::from_millis(1200)), None, "back to sent value");
        assert!(c.due(t0 + Duration::from_secs(5)).is_empty(), "pending was dropped");
        assert_eq!(c.current(1), Some(&vec![3]));
    }

    /// One simulated hour with every source changing far more often than real ones do.
    #[tokio::test(start_paused = true)]
    async fn widgets_stay_under_5_kbps() {
        let layout = Layout::default();
        let mut c = Coalescer::default();
        let t0 = Instant::now();
        let mut wire = 0usize;
        let count = |bytes: &Vec<u8>| LEN_PREFIX + 1 + 2 + 1 + bytes.len() + TAG_LEN + TCP_OVERHEAD;
        for step in 0..36_000u32 {
            let now = t0 + Duration::from_millis(u64::from(step) * 100);
            let sys = WidgetValue::System(SystemStats {
                cpu_pct: Some((step % 100) as u8),
                gpu_pct: Some((step % 37) as u8),
                ram_used_mb: 8000 + step % 50,
                ram_total_mb: 16000,
                cpu_temp_dc: Some(400 + (step % 20) as i16),
                gpu_temp_dc: None,
            });
            let claude = WidgetValue::ClaudeCode(vec![ClaudeSession {
                project: "psp-companion".into(),
                state: if step % 30 < 15 { ClaudeState::Working } else { ClaudeState::Waiting },
                detail: format!("tool step {}", step / 10),
            }]);
            for v in [sys, claude] {
                let b = v.encode();
                for d in layout.widgets.iter().filter(|d| d.kind == v.kind()) {
                    if let Some(sent) = c.offer(d.id, b.clone(), now) {
                        wire += count(&sent);
                    }
                }
            }
            for (_, sent) in c.due(now) {
                wire += count(&sent);
            }
        }
        let kbps = wire as f64 * 8.0 / 1000.0 / 3600.0;
        assert!(kbps < 5.0, "widget traffic {kbps:.2} kbps");
    }
}
