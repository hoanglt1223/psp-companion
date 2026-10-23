//! TCP control server (pairing and sessions) and the UDP audio socket.

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use super::crypto::{DatagramOpener, DatagramSealer, FrameCipher, SessionKeys, datagram_session_tag, random_array};
use super::frame::{LEN_PREFIX, encode_frame, read_frame};
use super::handshake::{Handshake, Id16, ParsedHandshake, Status};
use super::message::{ByeReason, Decoded, Direction, Message, error_code};
use super::ratelimit::{TCP_OVERHEAD, UDP_OVERHEAD};
use crate::config::NetworkConfig;
use crate::pairing::HostPairing;
use crate::state::{AppState, OUTBOUND_QUEUE, Outbound, SessionHandle, UdpEndpoint};

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
pub const SW_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Listeners {
    pub tcp: TcpListener,
    pub audio: Arc<UdpSocket>,
    pub discovery: UdpSocket,
}

impl Listeners {
    pub async fn bind(net: &NetworkConfig) -> io::Result<Self> {
        let tcp = TcpListener::bind(SocketAddr::new(net.bind, net.control_port)).await?;
        let audio = Arc::new(UdpSocket::bind(SocketAddr::new(net.bind, net.audio_port)).await?);
        let discovery = UdpSocket::bind(SocketAddr::new(net.bind, net.discovery_port)).await?;
        discovery.set_broadcast(true)?;
        Ok(Self { tcp, audio, discovery })
    }
}

/// Runs all network tasks until one of the listeners fails.
pub async fn run(state: Arc<AppState>, listeners: Listeners) -> io::Result<()> {
    state.set_audio_socket(listeners.audio.clone());
    let discovery = tokio::spawn(super::discovery::run(state.clone(), listeners.discovery));
    let audio = tokio::spawn(run_audio(state.clone(), listeners.audio));
    let accept = accept_loop(state, listeners.tcp);
    let result = accept.await;
    discovery.abort();
    audio.abort();
    result
}

async fn accept_loop(state: Arc<AppState>, listener: TcpListener) -> io::Result<()> {
    loop {
        let (stream, addr) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(state, stream, addr).await {
                debug!(%addr, "connection ended: {e}");
            }
        });
    }
}

fn protocol_err(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

async fn read_counted<R: AsyncRead + Unpin>(state: &AppState, r: &mut R, limit: Duration) -> io::Result<Vec<u8>> {
    let body = timeout(limit, read_frame(r)).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "idle"))??;
    state.record_rx(body.len() + LEN_PREFIX + TCP_OVERHEAD);
    Ok(body)
}

async fn write_counted<W: AsyncWrite + Unpin>(state: &AppState, w: &mut W, body: &[u8]) -> io::Result<()> {
    let frame = encode_frame(body)?;
    state.limiter.acquire(frame.len() + TCP_OVERHEAD).await;
    w.write_all(&frame).await?;
    w.flush().await?;
    state.record_tx(frame.len() + TCP_OVERHEAD);
    Ok(())
}

async fn handle_connection(state: Arc<AppState>, mut stream: TcpStream, addr: SocketAddr) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let first = read_counted(&state, &mut stream, HANDSHAKE_TIMEOUT).await?;
    let parsed = Handshake::decode(&first).map_err(|e| protocol_err(format!("bad handshake: {e}")))?;
    match parsed {
        ParsedHandshake::UnsupportedVersion(v) => {
            info!(%addr, "rejecting protocol version {v}");
            let reply = Handshake::SessionReply { status: Status::Version, host_id: [0; 16], server_nonce: [0; 16] };
            write_counted(&state, &mut stream, &reply.encode()).await
        }
        ParsedHandshake::Message(Handshake::PairRequest { device_id, pa, device_name }) => {
            handle_pairing(&state, &mut stream, addr, device_id, pa, sanitize_name(&device_name)).await
        }
        ParsedHandshake::Message(Handshake::SessionHello { device_id, .. }) => {
            handle_session(state, stream, addr, first, device_id).await
        }
        ParsedHandshake::Message(other) => Err(protocol_err(format!("unexpected first message {other:?}"))),
    }
}

/// Device names come from the network and end up in logs and the tray menu.
fn sanitize_name(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    let clean = super::codec::truncate_utf8(clean.trim(), 32).to_string();
    if clean.is_empty() { "PSP".to_string() } else { clean }
}

async fn handle_pairing(
    state: &AppState,
    stream: &mut TcpStream,
    addr: SocketAddr,
    device_id: Id16,
    pa: [u8; 32],
    device_name: String,
) -> io::Result<()> {
    let host_id = state.config().host_id.0;
    let reject = |status| Handshake::PairChallenge { status, host_id, pb: [0; 32], cb: [0; 32] };
    let pin = match state.take_pin_attempt() {
        Ok(pin) => pin,
        Err(status) => {
            info!(%addr, ?status, "pairing request while pairing is not open");
            return write_counted(state, stream, &reject(status).encode()).await;
        }
    };
    let host = match HostPairing::respond(&pin, &device_id, &host_id, &pa) {
        Ok(h) => h,
        Err(e) => {
            warn!(%addr, "pairing aborted: {e}");
            return write_counted(state, stream, &reject(Status::WrongPin).encode()).await;
        }
    };
    let challenge = Handshake::PairChallenge { status: Status::Ok, host_id, pb: host.pb, cb: host.cb };
    write_counted(state, stream, &challenge.encode()).await?;

    let body = read_counted(state, stream, HANDSHAKE_TIMEOUT).await?;
    let ca = match Handshake::decode(&body) {
        Ok(ParsedHandshake::Message(Handshake::PairConfirm { ca })) => ca,
        other => return Err(protocol_err(format!("expected PairConfirm, got {other:?}"))),
    };
    let status = match host.verify(&ca) {
        Ok(key) => match state.complete_pairing(device_id, device_name.clone(), key) {
            Ok(()) => {
                info!(%addr, device = %device_name, "paired");
                Status::Ok
            }
            Err(e) => {
                warn!("pairing succeeded but the key could not be saved: {e}");
                Status::Busy
            }
        },
        Err(e) => {
            warn!(%addr, "pairing failed: {e}");
            Status::WrongPin
        }
    };
    write_counted(state, stream, &Handshake::PairResult { status }.encode()).await
}

async fn handle_session(
    state: Arc<AppState>,
    mut stream: TcpStream,
    addr: SocketAddr,
    hello_body: Vec<u8>,
    device_id: Id16,
) -> io::Result<()> {
    let cfg = state.config();
    let Some(pairing_key) = state.device_key(&device_id) else {
        info!(%addr, device = %hex::encode(device_id), "session from unknown device");
        let reply = Handshake::SessionReply { status: Status::Unknown, host_id: cfg.host_id.0, server_nonce: [0; 16] };
        return write_counted(&state, &mut stream, &reply.encode()).await;
    };
    let reply =
        Handshake::SessionReply { status: Status::Ok, host_id: cfg.host_id.0, server_nonce: random_array() }.encode();
    write_counted(&state, &mut stream, &reply).await?;
    let keys = SessionKeys::derive(&pairing_key, &hello_body, &reply);
    let mut rx_cipher = FrameCipher::new(&keys.tcp_c2s);
    let mut tx_cipher = FrameCipher::new(&keys.tcp_s2c);

    let body = read_counted(&state, &mut stream, HANDSHAKE_TIMEOUT).await?;
    let plain = rx_cipher.open(&body).map_err(|_| protocol_err("Hello failed to decrypt (wrong key)"))?;
    let device_name = match Message::decode(&plain) {
        Ok(Decoded::Known(Message::Hello { device_name, sw_version, .. })) => {
            info!(%addr, device = %device_name, psp_version = %sw_version, "session established");
            sanitize_name(&device_name)
        }
        other => return Err(protocol_err(format!("expected Hello, got {other:?}"))),
    };

    let session_tag = loop {
        let tag = u32::from_le_bytes(random_array());
        if tag != 0 && !state.session_tag_in_use(tag) {
            break tag;
        }
    };
    let ack = Message::HelloAck {
        caps: 0,
        host_name: cfg.host_name.clone(),
        session_tag,
        audio_port: cfg.network.audio_port,
        keepalive_s: cfg.network.keepalive_secs,
        budget_kbps: cfg.network.budget_kbps.min(u32::from(u16::MAX)) as u16,
    };
    let sealed = tx_cipher.seal(&ack.encode()).map_err(|e| protocol_err(e.to_string()))?;
    write_counted(&state, &mut stream, &sealed).await?;

    let (tx, rx) = mpsc::channel(OUTBOUND_QUEUE);
    let session_id = state.new_session_id();
    let udp = Arc::new(Mutex::new(UdpEndpoint {
        peer: None,
        sealer: DatagramSealer::new(&keys.udp_s2c, session_tag),
        opener: DatagramOpener::new(&keys.udp_c2s),
    }));
    state.register_session(
        device_id,
        SessionHandle { session_id, device_name: device_name.clone(), addr, session_tag, outbound: tx.clone(), udp },
    );

    let (mut reader, writer) = stream.into_split();
    let mut writer_task = tokio::spawn(write_loop(state.clone(), writer, tx_cipher, rx));
    let idle = Duration::from_secs(3 * u64::from(cfg.network.keepalive_secs) + 5);

    let result = loop {
        tokio::select! {
            frame = read_counted(&state, &mut reader, idle) => {
                let body = match frame {
                    Ok(b) => b,
                    Err(e) => break Err(e),
                };
                let plain = match rx_cipher.open(&body) {
                    Ok(p) => p,
                    Err(e) => break Err(protocol_err(format!("decrypt: {e}"))),
                };
                match handle_message(&plain, &tx) {
                    Ok(true) => {}
                    Ok(false) => break Ok(()),
                    Err(e) => {
                        let _ = tx.try_send(Outbound::Message(Message::Error {
                            code: error_code::PROTOCOL,
                            text: e.to_string(),
                        }));
                        let _ = tx.try_send(Outbound::Close(ByeReason::Normal));
                        break Err(e);
                    }
                }
            }
            _ = &mut writer_task => break Ok(()),
        }
    };
    state.unregister_session(&device_id, session_id);
    drop(tx);
    // Give the writer a moment to flush a final Error or Bye before the socket drops.
    if !writer_task.is_finished() {
        let _ = timeout(Duration::from_secs(2), &mut writer_task).await;
        writer_task.abort();
    }
    info!(%addr, device = %device_name, "session closed: {result:?}");
    result
}

/// Returns Ok(false) when the peer asked to close.
fn handle_message(plain: &[u8], tx: &mpsc::Sender<Outbound>) -> io::Result<bool> {
    let msg = match Message::decode(plain) {
        Ok(Decoded::Known(m)) => m,
        Ok(Decoded::Unknown(t)) => {
            debug!("ignoring unknown message type 0x{t:02x}");
            return Ok(true);
        }
        Err(e) => return Err(protocol_err(format!("malformed message: {e}"))),
    };
    if msg.direction() == Direction::ServerToClient || msg.allowed_on_udp() {
        return Err(protocol_err(format!("message 0x{:02x} not allowed from the PSP on TCP", msg.type_id())));
    }
    match msg {
        Message::Ping { id, t_us } => {
            if tx.try_send(Outbound::Message(Message::Pong { id, t_us })).is_err() {
                warn!("outbound queue full; Pong dropped");
            }
        }
        Message::Bye { reason } => {
            debug!(?reason, "PSP said goodbye");
            return Ok(false);
        }
        Message::Error { code, text } => warn!(code, "PSP reported an error: {text}"),
        Message::Hello { .. } => return Err(protocol_err("duplicate Hello")),
        other => debug!(type_id = other.type_id(), "message not handled by this host version"),
    }
    Ok(true)
}

async fn write_loop(
    state: Arc<AppState>,
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    mut cipher: FrameCipher,
    mut rx: mpsc::Receiver<Outbound>,
) -> io::Result<()> {
    while let Some(out) = rx.recv().await {
        let (msg, last) = match out {
            Outbound::Message(m) => (m, false),
            Outbound::Close(reason) => (Message::Bye { reason }, true),
        };
        let body = cipher.seal(&msg.encode()).map_err(|e| protocol_err(e.to_string()))?;
        write_counted(&state, &mut writer, &body).await?;
        if last {
            break;
        }
    }
    writer.shutdown().await
}

async fn run_audio(state: Arc<AppState>, socket: Arc<UdpSocket>) {
    let mut buf = [0u8; 2048];
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                debug!("audio recv error: {e}");
                continue;
            }
        };
        state.record_rx(n + UDP_OVERHEAD);
        let datagram = &buf[..n];
        let Some((udp, outbound)) = datagram_session_tag(datagram).and_then(|tag| state.udp_route(tag)) else {
            debug!(%from, "datagram for no session");
            continue;
        };
        let opened = udp.lock().unwrap_or_else(|e| e.into_inner()).opener.open(datagram);
        let plain = match opened {
            Ok(p) => p,
            Err(e) => {
                debug!(%from, "dropping datagram: {e}");
                continue;
            }
        };
        match Message::decode(&plain) {
            Ok(Decoded::Known(Message::AudioRegister)) => {
                udp.lock().unwrap_or_else(|e| e.into_inner()).peer = Some(from);
                if outbound.try_send(Outbound::Message(Message::AudioRegisterAck)).is_err() {
                    warn!("outbound queue full; AudioRegisterAck dropped");
                }
            }
            other => debug!(%from, "unexpected datagram content: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_device_names() {
        assert_eq!(sanitize_name("  PSP\u{7}-3000\n "), "PSP-3000");
        assert_eq!(sanitize_name(""), "PSP");
        assert_eq!(sanitize_name(&"é".repeat(40)).len(), 32);
    }
}
