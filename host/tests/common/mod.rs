//! Shared helpers: a real host on localhost and a test client that plays the PSP.
#![allow(dead_code, unused_imports)]

pub use std::net::{Ipv4Addr, SocketAddr};
pub use std::sync::Arc;
pub use std::time::Duration;

pub use companion_host::config::Config;
pub use companion_host::net::crypto::{DatagramOpener, DatagramSealer, FrameCipher, SessionKeys, random_array};
pub use companion_host::net::discovery::{Announce, FLAG_PAIRING_OPEN, encode_request};
pub use companion_host::net::frame::{read_frame, write_frame};
pub use companion_host::net::handshake::{Handshake, Id16, ParsedHandshake, Status};
pub use companion_host::net::message::{ByeReason, Decoded, Message};
pub use companion_host::net::server::{self, Listeners};
pub use companion_host::pairing::ClientPairing;
pub use companion_host::state::AppState;
pub use tokio::net::{TcpListener, TcpStream, UdpSocket};
pub use tokio::time::timeout;

pub const T: Duration = Duration::from_secs(5);

pub struct Host {
    pub state: Arc<AppState>,
    pub control: SocketAddr,
    pub audio: SocketAddr,
    pub discovery: SocketAddr,
    pub host_id: Id16,
    pub dir: std::path::PathBuf,
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub async fn start_host(name: &str) -> Host {
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

pub async fn send_hs(s: &mut TcpStream, h: &Handshake) -> Vec<u8> {
    let body = h.encode();
    write_frame(s, &body).await.unwrap();
    body
}

pub async fn recv_hs(s: &mut TcpStream) -> (Vec<u8>, Handshake) {
    let body = timeout(T, read_frame(s)).await.unwrap().unwrap();
    match Handshake::decode(&body).unwrap() {
        ParsedHandshake::Message(m) => (body, m),
        other => panic!("unexpected {other:?}"),
    }
}

pub async fn pair(host: &Host, pin: &str, device_id: Id16) -> Result<[u8; 32], Status> {
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

pub struct Session {
    pub stream: TcpStream,
    pub tx: FrameCipher,
    pub rx: FrameCipher,
    pub keys: SessionKeys,
    pub session_tag: u32,
}

impl Session {
    pub async fn send(&mut self, m: &Message) {
        let body = self.tx.seal(&m.encode()).unwrap();
        write_frame(&mut self.stream, &body).await.unwrap();
    }

    pub async fn recv(&mut self) -> Message {
        let body = timeout(T, read_frame(&mut self.stream)).await.unwrap().unwrap();
        match Message::decode(&self.rx.open(&body).unwrap()).unwrap() {
            Decoded::Known(m) => m,
            Decoded::Unknown(t) => panic!("unknown {t}"),
        }
    }
}

pub async fn open_session(host: &Host, device_id: Id16, key: &[u8; 32]) -> Session {
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
