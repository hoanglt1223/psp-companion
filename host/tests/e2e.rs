//! Drives the real server over localhost, playing the PSP side of the protocol.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use companion_host::config::Config;
use companion_host::net::crypto::{DatagramOpener, DatagramSealer, FrameCipher, SessionKeys, random_array};
use companion_host::net::discovery::{Announce, FLAG_PAIRING_OPEN, encode_request};
use companion_host::net::frame::{read_frame, write_frame};
use companion_host::net::handshake::{Handshake, Id16, ParsedHandshake, Status};
use companion_host::net::message::{ByeReason, Decoded, Message};
use companion_host::net::server::{self, Listeners};
use companion_host::pairing::ClientPairing;
use companion_host::state::AppState;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time::timeout;

const T: Duration = Duration::from_secs(5);

struct Host {
    state: Arc<AppState>,
    control: SocketAddr,
    audio: SocketAddr,
    discovery: SocketAddr,
    host_id: Id16,
    dir: std::path::PathBuf,
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start_host(name: &str) -> Host {
    let lo = Ipv4Addr::LOCALHOST;
    let tcp = TcpListener::bind((lo, 0)).await.unwrap();
    let audio = Arc::new(UdpSocket::bind((lo, 0)).await.unwrap());
    let discovery = UdpSocket::bind((lo, 0)).await.unwrap();
    let (control, audio_addr, disc) =
        (tcp.local_addr().unwrap(), audio.local_addr().unwrap(), discovery.local_addr().unwrap());

    let dir = std::env::temp_dir().join(format!("pspc-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = Config::new_default();
    cfg.network.control_port = control.port();
    cfg.network.audio_port = audio_addr.port();
    cfg.network.discovery_port = disc.port();
    let host_id = cfg.host_id.0;
    let state = AppState::new(cfg, dir.join("config.toml"));
    tokio::spawn(server::run(state.clone(), Listeners { tcp, audio, discovery }));
    Host { state, control, audio: audio_addr, discovery: disc, host_id, dir }
}

async fn send_hs(s: &mut TcpStream, h: &Handshake) -> Vec<u8> {
    let body = h.encode();
    write_frame(s, &body).await.unwrap();
    body
}

async fn recv_hs(s: &mut TcpStream) -> (Vec<u8>, Handshake) {
    let body = timeout(T, read_frame(s)).await.unwrap().unwrap();
    match Handshake::decode(&body).unwrap() {
        ParsedHandshake::Message(m) => (body, m),
        other => panic!("unexpected {other:?}"),
    }
}

async fn pair(host: &Host, pin: &str, device_id: Id16) -> Result<[u8; 32], Status> {
    let mut s = TcpStream::connect(host.control).await.unwrap();
    let client = ClientPairing::start(pin, device_id).unwrap();
    send_hs(&mut s, &Handshake::PairRequest { device_id, pa: client.pa, device_name: "Test PSP".into() }).await;
    let (_, m) = recv_hs(&mut s).await;
    let Handshake::PairChallenge { status, host_id, pb, cb } = m else { panic!("{m:?}") };
    if status != Status::Ok {
        return Err(status);
    }
    assert_eq!(host_id, host.host_id);
    // A wrong PIN is detected on the PSP side from cB; it then just closes.
    let paired = client.finish(&host_id, &pb, &cb).map_err(|_| Status::WrongPin)?;
    send_hs(&mut s, &Handshake::PairConfirm { ca: paired.ca }).await;
    let (_, m) = recv_hs(&mut s).await;
    assert_eq!(m, Handshake::PairResult { status: Status::Ok });
    Ok(paired.pairing_key)
}

struct Session {
    stream: TcpStream,
    tx: FrameCipher,
    rx: FrameCipher,
    keys: SessionKeys,
    session_tag: u32,
}

impl Session {
    async fn send(&mut self, m: &Message) {
        let body = self.tx.seal(&m.encode()).unwrap();
        write_frame(&mut self.stream, &body).await.unwrap();
    }

    async fn recv(&mut self) -> Message {
        let body = timeout(T, read_frame(&mut self.stream)).await.unwrap().unwrap();
        match Message::decode(&self.rx.open(&body).unwrap()).unwrap() {
            Decoded::Known(m) => m,
            Decoded::Unknown(t) => panic!("unknown {t}"),
        }
    }
}

async fn open_session(host: &Host, device_id: Id16, key: &[u8; 32]) -> Session {
    let mut stream = TcpStream::connect(host.control).await.unwrap();
    let hello = send_hs(&mut stream, &Handshake::SessionHello { device_id, client_nonce: random_array() }).await;
    let (reply, m) = recv_hs(&mut stream).await;
    assert!(matches!(m, Handshake::SessionReply { status: Status::Ok, .. }), "{m:?}");
    let keys = SessionKeys::derive(key, &hello, &reply);
    let mut s = Session {
        stream,
        tx: FrameCipher::new(&keys.tcp_c2s),
        rx: FrameCipher::new(&keys.tcp_s2c),
        keys,
        session_tag: 0,
    };
    s.send(&Message::Hello { caps: 0, sw_version: "test".into(), device_name: "Test PSP".into() }).await;
    match s.recv().await {
        Message::HelloAck { session_tag, audio_port, keepalive_s, budget_kbps, .. } => {
            assert_eq!(audio_port, host.audio.port());
            assert_eq!(keepalive_s, 15);
            assert_eq!(budget_kbps, 300);
            s.session_tag = session_tag;
        }
        other => panic!("{other:?}"),
    }
    s
}

#[tokio::test]
async fn discovery_announces_pairing_state() {
    let host = start_host("discovery").await;
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let mut buf = [0u8; 128];

    sock.send_to(&encode_request(&[1; 8]), host.discovery).await.unwrap();
    let n = timeout(T, sock.recv(&mut buf)).await.unwrap().unwrap();
    let a = Announce::decode(&buf[..n]).unwrap();
    assert_eq!((a.nonce, a.host_id, a.control_port, a.flags), ([1; 8], host.host_id, host.control.port(), 0));

    host.state.open_pairing();
    sock.send_to(&encode_request(&[2; 8]), host.discovery).await.unwrap();
    let n = timeout(T, sock.recv(&mut buf)).await.unwrap().unwrap();
    assert_eq!(Announce::decode(&buf[..n]).unwrap().flags, FLAG_PAIRING_OPEN);

    sock.send_to(b"garbage", host.discovery).await.unwrap();
    assert!(timeout(Duration::from_millis(300), sock.recv(&mut buf)).await.is_err(), "no reply to garbage");
}

#[tokio::test]
async fn pairing_session_keepalive_and_audio_channel() {
    let host = start_host("full").await;
    let device_id: Id16 = random_array();

    assert_eq!(pair(&host, "000000", device_id).await, Err(Status::Unknown), "pairing not open");
    let pin = host.state.open_pairing();
    let wrong = if pin == "999999" { "000000" } else { "999999" };
    assert_eq!(pair(&host, wrong, device_id).await, Err(Status::WrongPin));
    let key = pair(&host, &pin, device_id).await.expect("pairing with the right PIN");
    assert_eq!(host.state.device_key(&device_id), Some(key));
    assert!(host.state.snapshot().pin.is_none(), "PIN is void after success");
    let saved = std::fs::read_to_string(host.dir.join("config.toml")).unwrap();
    assert!(saved.contains(&hex::encode(device_id)));

    let mut s = open_session(&host, device_id, &key).await;
    s.send(&Message::Ping { id: 1, t_us: 42 }).await;
    assert_eq!(s.recv().await, Message::Pong { id: 1, t_us: 42 });

    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let mut sealer = DatagramSealer::new(&s.keys.udp_c2s, s.session_tag);
    udp.send_to(&sealer.seal(&Message::AudioRegister.encode()).unwrap(), host.audio).await.unwrap();
    assert_eq!(s.recv().await, Message::AudioRegisterAck);

    let packet = Message::AudioPacket { stream_id: 1, seq: 9, pts_ms: 20, data: vec![0xaa; 400] };
    host.state.send_datagram(&device_id, &packet).await.unwrap();
    let mut buf = [0u8; 2048];
    let n = timeout(T, udp.recv(&mut buf)).await.unwrap().unwrap();
    let plain = DatagramOpener::new(&s.keys.udp_s2c).open(&buf[..n]).unwrap();
    assert_eq!(Message::decode(&plain).unwrap(), Decoded::Known(packet));

    assert_eq!(host.state.broadcast(&Message::WidgetData { widget_id: 1, format: 0, data: b"{}".to_vec() }), 1);
    assert!(matches!(s.recv().await, Message::WidgetData { widget_id: 1, .. }));

    // A newer session from the same PSP replaces this one.
    let mut s2 = open_session(&host, device_id, &key).await;
    assert_eq!(s.recv().await, Message::Bye { reason: ByeReason::Replaced });
    s2.send(&Message::Ping { id: 2, t_us: 0 }).await;
    assert_eq!(s2.recv().await, Message::Pong { id: 2, t_us: 0 });
}

#[tokio::test]
async fn rejects_unknown_device_wrong_key_and_bad_direction() {
    let host = start_host("reject").await;
    let mut s = TcpStream::connect(host.control).await.unwrap();
    send_hs(&mut s, &Handshake::SessionHello { device_id: [9; 16], client_nonce: [0; 16] }).await;
    let (_, m) = recv_hs(&mut s).await;
    assert!(matches!(m, Handshake::SessionReply { status: Status::Unknown, .. }));

    let device_id: Id16 = random_array();
    let pin = host.state.open_pairing();
    let key = pair(&host, &pin, device_id).await.unwrap();

    // Wrong key: the host cannot decrypt Hello and closes without a reply.
    let mut stream = TcpStream::connect(host.control).await.unwrap();
    let hello = send_hs(&mut stream, &Handshake::SessionHello { device_id, client_nonce: [1; 16] }).await;
    let (reply, _) = recv_hs(&mut stream).await;
    let keys = SessionKeys::derive(&[0u8; 32], &hello, &reply);
    let body = FrameCipher::new(&keys.tcp_c2s)
        .seal(&Message::Hello { caps: 0, sw_version: String::new(), device_name: String::new() }.encode())
        .unwrap();
    write_frame(&mut stream, &body).await.unwrap();
    assert!(timeout(T, read_frame(&mut stream)).await.unwrap().is_err(), "closed");

    // A host-to-PSP message sent by the PSP is a protocol error.
    let mut s = open_session(&host, device_id, &key).await;
    s.send(&Message::Pong { id: 0, t_us: 0 }).await;
    assert!(matches!(s.recv().await, Message::Error { code: 1, .. }));
    assert_eq!(s.recv().await, Message::Bye { reason: ByeReason::Normal });
}

#[tokio::test]
async fn version_mismatch_is_reported() {
    let host = start_host("version").await;
    let mut s = TcpStream::connect(host.control).await.unwrap();
    let mut body = Handshake::SessionHello { device_id: [0; 16], client_nonce: [0; 16] }.encode();
    body[4] = 9;
    write_frame(&mut s, &body).await.unwrap();
    let (_, m) = recv_hs(&mut s).await;
    assert!(matches!(m, Handshake::SessionReply { status: Status::Version, .. }));
}
