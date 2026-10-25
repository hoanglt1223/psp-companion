//! Shared host state: config, pending PIN, live sessions, traffic counters.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::{Config, ConfigError, Hex, PairedDevice};
use crate::net::crypto::{CryptoError, DatagramOpener, DatagramSealer};
use crate::net::discovery::{Announce, FLAG_PAIRING_OPEN};
use crate::net::handshake::Id16;
use crate::net::message::{ByeReason, Message};
use crate::net::ratelimit::{RateLimiter, UDP_OVERHEAD};
use crate::pairing::{PendingPin, generate_pin};

pub const OUTBOUND_QUEUE: usize = 64;
pub const EVENT_QUEUE: usize = 256;

/// What the network layer reports to the feature modules (widgets, macros, audio, voice, deck).
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    SessionUp { device: Id16, session_id: u64, name: String },
    SessionDown { device: Id16, session_id: u64 },
    Message { device: Id16, msg: Message },
}

#[derive(Debug)]
pub enum Outbound {
    Message(Message),
    Close(ByeReason),
}

pub struct SessionHandle {
    pub session_id: u64,
    pub device_name: String,
    pub addr: SocketAddr,
    pub session_tag: u32,
    pub outbound: mpsc::Sender<Outbound>,
    pub udp: Arc<Mutex<UdpEndpoint>>,
}

pub struct UdpEndpoint {
    pub peer: Option<SocketAddr>,
    pub sealer: DatagramSealer,
    pub opener: DatagramOpener,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub device_name: String,
    pub addr: SocketAddr,
    pub udp_registered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub pin: Option<(String, Duration)>,
    pub sessions: Vec<SessionSummary>,
    pub paired_devices: usize,
    pub bytes_tx: u64,
    pub bytes_rx: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("no session for this device")]
    NoSession,
    #[error("the PSP has not registered its audio endpoint yet")]
    NoUdpPeer,
    #[error("dropped by the Wi-Fi rate limiter")]
    RateLimited,
    #[error("session closed")]
    Closed,
    #[error("audio socket not bound")]
    NoSocket,
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct AppState {
    config_path: PathBuf,
    config: Mutex<Config>,
    pending_pin: Mutex<Option<PendingPin>>,
    sessions: Mutex<HashMap<Id16, SessionHandle>>,
    next_session_id: AtomicU64,
    bytes_tx: AtomicU64,
    bytes_rx: AtomicU64,
    audio_socket: OnceLock<Arc<UdpSocket>>,
    events: Mutex<Option<mpsc::Sender<HostEvent>>>,
    pub limiter: RateLimiter,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // All guarded data stays consistent between statements, so a poisoned lock is still usable.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppState {
    pub fn new(config: Config, config_path: PathBuf) -> Arc<Self> {
        let limiter = RateLimiter::new(config.network.budget_kbps);
        Arc::new(Self {
            config_path,
            config: Mutex::new(config),
            pending_pin: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            next_session_id: AtomicU64::new(1),
            bytes_tx: AtomicU64::new(0),
            bytes_rx: AtomicU64::new(0),
            audio_socket: OnceLock::new(),
            events: Mutex::new(None),
            limiter,
        })
    }

    pub fn config(&self) -> Config {
        lock(&self.config).clone()
    }

    pub fn record_tx(&self, n: usize) {
        self.bytes_tx.fetch_add(n as u64, Ordering::Relaxed);
    }

    /// Counts the bytes and charges them to the shared Wi-Fi budget.
    pub fn record_rx(&self, n: usize) {
        self.bytes_rx.fetch_add(n as u64, Ordering::Relaxed);
        self.limiter.charge_incoming(n);
    }

    pub fn set_audio_socket(&self, socket: Arc<UdpSocket>) {
        if self.audio_socket.set(socket).is_err() {
            warn!("audio socket was already set");
        }
    }

    pub fn audio_socket(&self) -> Option<Arc<UdpSocket>> {
        self.audio_socket.get().cloned()
    }

    pub fn announce(&self, nonce: [u8; 8]) -> Announce {
        let cfg = lock(&self.config);
        let open = lock(&self.pending_pin).as_ref().is_some_and(|p| p.is_open(Instant::now()));
        Announce {
            nonce,
            host_id: cfg.host_id.0,
            control_port: cfg.network.control_port,
            audio_port: cfg.network.audio_port,
            flags: if open { FLAG_PAIRING_OPEN } else { 0 },
            host_name: cfg.host_name.clone(),
        }
    }

    /// Shows a fresh PIN, replacing any previous one.
    pub fn open_pairing(&self) -> String {
        let pin = generate_pin();
        *lock(&self.pending_pin) = Some(PendingPin::new(pin.clone(), Instant::now()));
        info!("pairing opened; PIN valid for 2 minutes");
        pin
    }

    pub fn cancel_pairing(&self) {
        *lock(&self.pending_pin) = None;
    }

    pub fn take_pin_attempt(&self) -> Result<String, crate::net::handshake::Status> {
        match lock(&self.pending_pin).as_mut() {
            Some(p) => p.take_attempt(Instant::now()),
            None => Err(crate::net::handshake::Status::Unknown),
        }
    }

    pub fn complete_pairing(&self, device_id: Id16, name: String, key: [u8; 32]) -> Result<(), ConfigError> {
        let paired_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut cfg = lock(&self.config);
        let mut next = cfg.clone();
        next.upsert_device(PairedDevice { id: Hex(device_id), name, key: Hex(key), paired_at });
        next.save(&self.config_path)?;
        *cfg = next;
        drop(cfg);
        *lock(&self.pending_pin) = None;
        Ok(())
    }

    /// Applies an edit to the config and saves it; the running features keep their settings until a restart.
    pub fn update_config(&self, edit: impl FnOnce(&mut Config)) -> Result<(), ConfigError> {
        let mut cfg = lock(&self.config);
        let mut next = cfg.clone();
        edit(&mut next);
        next.save(&self.config_path)?;
        *cfg = next;
        Ok(())
    }

    pub fn device_key(&self, device_id: &Id16) -> Option<[u8; 32]> {
        lock(&self.config).device(device_id).map(|d| d.key.0)
    }

    pub fn forget_all_devices(&self) -> Result<(), ConfigError> {
        let mut cfg = lock(&self.config);
        let mut next = cfg.clone();
        next.devices.clear();
        next.save(&self.config_path)?;
        *cfg = next;
        drop(cfg);
        self.close_all(ByeReason::Unpaired);
        Ok(())
    }

    pub fn new_session_id(&self) -> u64 {
        self.next_session_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Registers a session; an older session of the same device is told to close.
    pub fn register_session(&self, device_id: Id16, handle: SessionHandle) {
        let old = lock(&self.sessions).insert(device_id, handle);
        if let Some(old) = old {
            let _ = old.outbound.try_send(Outbound::Close(ByeReason::Replaced));
        }
    }

    /// Removes the session only if it is still the registered one for the device.
    pub fn unregister_session(&self, device_id: &Id16, session_id: u64) {
        let mut sessions = lock(&self.sessions);
        if sessions.get(device_id).is_some_and(|s| s.session_id == session_id) {
            sessions.remove(device_id);
        }
    }

    pub fn session_tag_in_use(&self, tag: u32) -> bool {
        lock(&self.sessions).values().any(|s| s.session_tag == tag)
    }

    /// Finds the UDP endpoint and control queue for a datagram's session tag.
    pub fn udp_route(&self, tag: u32) -> Option<(Arc<Mutex<UdpEndpoint>>, mpsc::Sender<Outbound>)> {
        lock(&self.sessions).values().find(|s| s.session_tag == tag).map(|s| (s.udp.clone(), s.outbound.clone()))
    }

    pub fn close_all(&self, reason: ByeReason) {
        for s in lock(&self.sessions).values() {
            let _ = s.outbound.try_send(Outbound::Close(reason));
        }
    }

    /// Queues a control message to every connected PSP. Push-on-change callers use this.
    pub fn broadcast(&self, msg: &Message) -> usize {
        let sessions = lock(&self.sessions);
        let mut queued = 0;
        for s in sessions.values() {
            match s.outbound.try_send(Outbound::Message(msg.clone())) {
                Ok(()) => queued += 1,
                Err(e) => warn!(device = %s.device_name, "outbound queue full or closed: {e}"),
            }
        }
        queued
    }

    /// Sends one audio datagram; real-time data is dropped rather than queued when over budget.
    pub async fn send_datagram(&self, device_id: &Id16, msg: &Message) -> Result<(), SendError> {
        let udp = lock(&self.sessions).get(device_id).map(|s| s.udp.clone()).ok_or(SendError::NoSession)?;
        let socket = self.audio_socket().ok_or(SendError::NoSocket)?;
        let (peer, datagram) = {
            let mut ep = lock(&udp);
            let peer = ep.peer.ok_or(SendError::NoUdpPeer)?;
            (peer, ep.sealer.seal(&msg.encode())?)
        };
        if !self.limiter.try_acquire(datagram.len() + UDP_OVERHEAD) {
            return Err(SendError::RateLimited);
        }
        let sent = socket.send_to(&datagram, peer).await?;
        self.record_tx(sent + UDP_OVERHEAD);
        Ok(())
    }

    /// Routes session and feature events to the returned receiver. Without a subscriber they are dropped.
    pub fn subscribe_events(&self) -> mpsc::Receiver<HostEvent> {
        let (tx, rx) = mpsc::channel(EVENT_QUEUE);
        *lock(&self.events) = Some(tx);
        rx
    }

    /// Waits for queue space, so a slow consumer slows the PSP's reader instead of losing voice data.
    pub async fn emit(&self, event: HostEvent) {
        let tx = lock(&self.events).clone();
        if let Some(tx) = tx
            && tx.send(event).await.is_err()
        {
            debug!("event consumer has stopped");
        }
    }

    /// Queues a control message to one PSP, waiting for queue space.
    pub async fn send_to(&self, device: &Id16, msg: Message) -> Result<(), SendError> {
        let tx = lock(&self.sessions).get(device).map(|s| s.outbound.clone()).ok_or(SendError::NoSession)?;
        tx.send(Outbound::Message(msg)).await.map_err(|_| SendError::Closed)
    }

    pub fn is_connected(&self, device: &Id16) -> bool {
        lock(&self.sessions).contains_key(device)
    }

    pub fn snapshot(&self) -> Snapshot {
        let now = Instant::now();
        let pin = lock(&self.pending_pin)
            .as_ref()
            .filter(|p| p.is_open(now))
            .map(|p| (p.pin().to_string(), p.remaining(now)));
        let mut sessions: Vec<SessionSummary> = lock(&self.sessions)
            .values()
            .map(|s| SessionSummary {
                device_name: s.device_name.clone(),
                addr: s.addr,
                udp_registered: lock(&s.udp).peer.is_some(),
            })
            .collect();
        sessions.sort_by(|a, b| a.device_name.cmp(&b.device_name));
        Snapshot {
            pin,
            sessions,
            paired_devices: lock(&self.config).devices.len(),
            bytes_tx: self.bytes_tx.load(Ordering::Relaxed),
            bytes_rx: self.bytes_rx.load(Ordering::Relaxed),
        }
    }
}
