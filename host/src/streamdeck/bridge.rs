//! Bridges Bitfocus Companion (Satellite) to the PSPs: each PSP is one 6x3 surface. Key images become
//! IDX8-LZ4 images cached on the PSP by ID; only changed keys are sent, at most 5 new images per second
//! per PSP, and keys that keep changing (animations) are held until they settle.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{Instant, interval};
use tracing::{info, warn};

use super::satellite::{self, COLUMNS, KEYS, ROWS, SatCmd, SatEvent, StreamDeckConfig};
use crate::imaging::{FORMAT_IDX8_LZ4, image_id, quantize_rgb};
use crate::net::handshake::Id16;
use crate::net::message::Message;
use crate::state::AppState;

/// `DeckKeyPress` keys beyond the grid: Bitfocus page switching from the PSP's L and R buttons.
pub const KEY_PAGE_PREV: u8 = 254;
pub const KEY_PAGE_NEXT: u8 = 255;
pub const TICK: Duration = Duration::from_millis(200);
/// More changes than this within ANIMATION_WINDOW marks a key as animated.
const ANIMATION_CHANGES: usize = 3;
const ANIMATION_WINDOW: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
pub struct KeyImage {
    pub id: u64,
    pub data: Vec<u8>,
}

#[derive(Default)]
struct Key {
    sent: Option<u64>,
    pending: Option<Arc<KeyImage>>,
    changes: VecDeque<Instant>,
}

impl Key {
    fn animated(&self, now: Instant) -> bool {
        self.changes.iter().filter(|t| now.duration_since(**t) < ANIMATION_WINDOW).count() > ANIMATION_CHANGES
    }

    fn settled(&self, now: Instant) -> bool {
        self.changes.back().is_none_or(|t| now.duration_since(*t) >= ANIMATION_WINDOW)
    }
}

/// The per-PSP deck state, without I/O, so its pacing can be tested with a fake clock.
pub struct Deck {
    pub size: u8,
    keys: Vec<Key>,
    /// Image IDs this PSP session has received.
    has: HashSet<u64>,
}

impl Deck {
    pub fn new(size: u8) -> Self {
        Self { size, keys: (0..KEYS).map(|_| Key::default()).collect(), has: HashSet::new() }
    }

    /// A new key image from Companion (None: blank).
    pub fn set(&mut self, key: u8, rgb: Option<&[u8]>, now: Instant) {
        let Some(k) = self.keys.get_mut(usize::from(key)) else { return };
        let size = u32::from(self.size);
        let black = vec![0u8; (size * size * 3) as usize];
        let Ok(img) = quantize_rgb(rgb.unwrap_or(&black), size, size) else { return };
        let data = img.encode();
        let id = image_id(&data);
        if k.sent == Some(id) && k.pending.is_none() {
            return;
        }
        k.changes.push_back(now);
        while k.changes.len() > 8 {
            k.changes.pop_front();
        }
        k.pending = if k.sent == Some(id) { None } else { Some(Arc::new(KeyImage { id, data })) };
    }

    pub fn clear(&mut self, now: Instant) {
        for key in 0..KEYS {
            self.set(key, None, now);
        }
    }

    /// Messages to send this tick: every pending key whose image the PSP already has (a few bytes each),
    /// plus at most one new image. Animated keys wait until they stop changing.
    pub fn tick(&mut self, now: Instant) -> Vec<Message> {
        let mut out = Vec::new();
        let mut new_image_sent = false;
        for (i, k) in self.keys.iter_mut().enumerate() {
            let Some(img) = k.pending.clone() else { continue };
            if k.animated(now) && !k.settled(now) {
                continue;
            }
            let cached = self.has.contains(&img.id);
            if !cached && new_image_sent {
                continue;
            }
            out.push(Message::DeckKeyImage {
                key: i as u8,
                image_id: img.id,
                format: FORMAT_IDX8_LZ4,
                width: self.size,
                height: self.size,
                data: if cached { Vec::new() } else { img.data.clone() },
            });
            if !cached {
                new_image_sent = true;
                self.has.insert(img.id);
            }
            k.sent = Some(img.id);
            k.pending = None;
        }
        out
    }
}

pub enum BridgeCmd {
    SessionUp(Id16, u64),
    SessionDown(Id16, u64),
    KeyPress(Id16, u8, bool),
}

#[derive(Clone)]
pub struct BridgeHandle {
    tx: mpsc::Sender<BridgeCmd>,
}

impl BridgeHandle {
    pub async fn send(&self, cmd: BridgeCmd) {
        let _ = self.tx.send(cmd).await;
    }
}

struct Psp {
    session_id: u64,
    device: String,
    deck: Deck,
}

fn layout(connected: bool, size: u8) -> Message {
    if connected {
        Message::DeckLayout { columns: COLUMNS, rows: ROWS, key_w: size, key_h: size }
    } else {
        // An empty grid tells the PSP that Companion is not reachable (it offers the macro pad instead).
        Message::DeckLayout { columns: 0, rows: 0, key_w: 0, key_h: 0 }
    }
}

pub fn spawn_bridge(state: Arc<AppState>, config: StreamDeckConfig) -> BridgeHandle {
    let (tx, mut rx) = mpsc::channel(64);
    let size = config.bitmap_size;
    let mut sat = satellite::spawn(config);
    tokio::spawn(async move {
        let mut psps: HashMap<Id16, Psp> = HashMap::new();
        let mut connected = false;
        let mut tick = interval(TICK);
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else { return };
                    match cmd {
                        BridgeCmd::SessionUp(psp, session_id) => {
                            let device = satellite::device_name(&psp);
                            psps.insert(psp, Psp { session_id, device: device.clone(), deck: Deck::new(size) });
                            let _ = state.send_to(&psp, layout(connected, size)).await;
                            let _ = sat.cmds.send(SatCmd::AddDevice { device }).await;
                        }
                        BridgeCmd::SessionDown(psp, session_id) => {
                            if psps.get(&psp).is_some_and(|p| p.session_id == session_id) {
                                let p = psps.remove(&psp).expect("checked");
                                let _ = sat.cmds.send(SatCmd::RemoveDevice { device: p.device }).await;
                            }
                        }
                        BridgeCmd::KeyPress(psp, key, pressed) => {
                            let Some(p) = psps.get(&psp) else { continue };
                            let device = p.device.clone();
                            let cmd = match key {
                                KEY_PAGE_PREV | KEY_PAGE_NEXT if pressed => {
                                    SatCmd::ChangePage { device, forward: key == KEY_PAGE_NEXT }
                                }
                                k if k < KEYS => SatCmd::KeyPress { device, key: k, pressed },
                                _ => continue,
                            };
                            let _ = sat.cmds.send(cmd).await;
                        }
                    }
                }
                ev = sat.events.recv() => {
                    let Some(ev) = ev else { return };
                    let now = Instant::now();
                    match ev {
                        SatEvent::Connected { .. } | SatEvent::Disconnected => {
                            let up = matches!(ev, SatEvent::Connected { .. });
                            if up != connected {
                                connected = up;
                                for (psp, p) in psps.iter_mut() {
                                    p.deck = Deck::new(size);
                                    let _ = state.send_to(psp, layout(connected, size)).await;
                                }
                            }
                        }
                        SatEvent::DeviceAdded { device } => info!(%device, "PSP registered as a Companion surface"),
                        SatEvent::DeviceError { device, message } => warn!(%device, "Companion refused the PSP surface: {message}"),
                        SatEvent::KeyImage { device, key, rgb } => {
                            if let Some(p) = psps.values_mut().find(|p| p.device == device) {
                                p.deck.set(key, rgb.as_deref(), now);
                            }
                        }
                        SatEvent::KeysClear { device } => {
                            if let Some(p) = psps.values_mut().find(|p| p.device == device) {
                                p.deck.clear(now);
                            }
                        }
                    }
                }
                _ = tick.tick() => {
                    let now = Instant::now();
                    for (psp, p) in psps.iter_mut() {
                        for msg in p.deck.tick(now) {
                            let _ = state.send_to(psp, msg).await;
                        }
                    }
                }
            }
        }
    });
    BridgeHandle { tx }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(size: u8, rgb: [u8; 3]) -> Vec<u8> {
        rgb.repeat(usize::from(size) * usize::from(size))
    }

    fn keys_of(msgs: &[Message]) -> Vec<(u8, bool)> {
        msgs.iter()
            .map(|m| match m {
                Message::DeckKeyImage { key, data, .. } => (*key, !data.is_empty()),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn sends_at_most_five_new_images_per_second_and_reuses_cached_ones() {
        let mut d = Deck::new(76);
        let t0 = Instant::now();
        for k in 0..KEYS {
            d.set(k, Some(&solid(76, [k * 10, 0, 0])), t0);
        }
        let mut sent = Vec::new();
        for i in 0..5u32 {
            sent.extend(keys_of(&d.tick(t0 + TICK * i)));
        }
        assert_eq!(sent.len(), 5, "one new image per 200 ms tick");
        assert!(sent.iter().all(|(_, data)| *data));

        // Keys 0 and 1 swap images with keys that were already sent: no image data needed, no waiting.
        let mut d = Deck::new(76);
        d.set(0, Some(&solid(76, [1, 1, 1])), t0);
        d.tick(t0);
        d.set(1, Some(&solid(76, [2, 2, 2])), t0);
        d.tick(t0 + TICK);
        d.set(0, Some(&solid(76, [2, 2, 2])), t0 + TICK * 3);
        d.set(1, Some(&solid(76, [1, 1, 1])), t0 + TICK * 3);
        assert_eq!(keys_of(&d.tick(t0 + TICK * 4)), vec![(0, false), (1, false)]);
    }

    #[tokio::test(start_paused = true)]
    async fn unchanged_keys_are_not_resent() {
        let mut d = Deck::new(72);
        let t0 = Instant::now();
        d.set(3, Some(&solid(72, [9, 9, 9])), t0);
        assert_eq!(d.tick(t0).len(), 1);
        d.set(3, Some(&solid(72, [9, 9, 9])), t0 + TICK);
        assert!(d.tick(t0 + TICK * 2).is_empty());
        // A change and a change back before the tick cancel out.
        d.set(3, Some(&solid(72, [1, 2, 3])), t0 + TICK * 3);
        d.set(3, Some(&solid(72, [9, 9, 9])), t0 + TICK * 3);
        assert!(d.tick(t0 + TICK * 4).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn animated_keys_show_a_static_frame() {
        let mut d = Deck::new(40);
        let t0 = Instant::now();
        let mut sent = 0;
        // Ten frames per second for two seconds.
        for f in 0..20u32 {
            let now = t0 + Duration::from_millis(u64::from(f) * 100);
            d.set(0, Some(&solid(40, [f as u8 * 10, 0, 0])), now);
            if f % 2 == 1 {
                sent += d.tick(now).len();
            }
        }
        assert!(sent <= 2, "an animation does not stream: {sent} images");
        // Once it stops, its final frame goes out.
        let after = t0 + Duration::from_millis(1900) + ANIMATION_WINDOW;
        let last = d.tick(after);
        assert_eq!(keys_of(&last), vec![(0, true)]);
        assert!(d.tick(after + TICK).is_empty());
    }

    #[test]
    fn blank_and_clear() {
        let mut d = Deck::new(32);
        let t0 = Instant::now();
        d.clear(t0);
        let msgs = keys_of(&d.tick(t0));
        assert_eq!(msgs.len(), 18, "18 identical black keys: one image, then 17 cache hits");
        assert_eq!(msgs.iter().filter(|(_, data)| *data).count(), 1);
        assert!(d.tick(t0 + TICK).is_empty());
    }
}
