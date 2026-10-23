//! Session key schedule and ChaCha20-Poly1305 framing for the TCP and UDP channels.

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const TAG_LEN: usize = 16;
pub const UDP_HEADER_LEN: usize = 13;
pub const UDP_KIND_DATA: u8 = 0x01;
pub const MAX_DATAGRAM: usize = 1200;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("authentication failed")]
    Auth,
    #[error("message too large")]
    TooLarge,
    #[error("malformed datagram")]
    Malformed,
    #[error("replayed or stale datagram")]
    Replay,
    #[error("nonce counter exhausted")]
    CounterExhausted,
}

/// Panics if the OS RNG fails; without it no key or nonce can be made safely.
pub fn random_array<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("OS random number generator unavailable");
    out
}

#[derive(Clone)]
pub struct SessionKeys {
    pub tcp_c2s: [u8; 32],
    pub tcp_s2c: [u8; 32],
    pub udp_c2s: [u8; 32],
    pub udp_s2c: [u8; 32],
}

impl SessionKeys {
    /// `hello` and `reply` are the full cleartext frame bodies of SessionHello and SessionReply.
    pub fn derive(pairing_key: &[u8; 32], hello: &[u8], reply: &[u8]) -> Self {
        let th = Sha256::new().chain_update(hello).chain_update(reply).finalize();
        let hk = Hkdf::<Sha256>::new(Some(&th), pairing_key);
        let mut okm = [0u8; 128];
        hk.expand(b"PSPC v1 session keys", &mut okm).expect("128 bytes is a valid HKDF-SHA256 length");
        let take = |i: usize| -> [u8; 32] { okm[i * 32..(i + 1) * 32].try_into().expect("32-byte slice") };
        Self { tcp_c2s: take(0), tcp_s2c: take(1), udp_c2s: take(2), udp_s2c: take(3) }
    }
}

fn nonce(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_le_bytes());
    n
}

fn cipher(key: &[u8; 32]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(&(*key).into())
}

/// One direction of the TCP channel. The counter is implicit, so frames must be
/// processed strictly in order and any failure ends the connection.
pub struct FrameCipher {
    aead: ChaCha20Poly1305,
    counter: u64,
}

impl FrameCipher {
    pub fn new(key: &[u8; 32]) -> Self {
        Self { aead: cipher(key), counter: 0 }
    }

    /// Returns the frame body (ciphertext || tag). The caller writes the u16 length first.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let body_len = plaintext.len() + TAG_LEN;
        let len = u16::try_from(body_len).map_err(|_| CryptoError::TooLarge)?;
        let n = self.next_nonce()?;
        self.aead
            .encrypt(&n.into(), Payload { msg: plaintext, aad: &len.to_le_bytes() })
            .map_err(|_| CryptoError::TooLarge)
    }

    pub fn open(&mut self, body: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let len = u16::try_from(body.len()).map_err(|_| CryptoError::TooLarge)?;
        if body.len() < TAG_LEN {
            return Err(CryptoError::Auth);
        }
        let n = self.next_nonce()?;
        self.aead.decrypt(&n.into(), Payload { msg: body, aad: &len.to_le_bytes() }).map_err(|_| CryptoError::Auth)
    }

    fn next_nonce(&mut self) -> Result<[u8; 12], CryptoError> {
        let n = nonce(self.counter);
        self.counter = self.counter.checked_add(1).ok_or(CryptoError::CounterExhausted)?;
        Ok(n)
    }
}

pub struct DatagramSealer {
    aead: ChaCha20Poly1305,
    session_tag: u32,
    seq: u64,
}

impl DatagramSealer {
    pub fn new(key: &[u8; 32], session_tag: u32) -> Self {
        Self { aead: cipher(key), session_tag, seq: 0 }
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if UDP_HEADER_LEN + plaintext.len() + TAG_LEN > MAX_DATAGRAM {
            return Err(CryptoError::TooLarge);
        }
        let seq = self.seq;
        self.seq = self.seq.checked_add(1).ok_or(CryptoError::CounterExhausted)?;
        let header = datagram_header(self.session_tag, seq);
        let ct = self
            .aead
            .encrypt(&nonce(seq).into(), Payload { msg: plaintext, aad: &header })
            .map_err(|_| CryptoError::TooLarge)?;
        let mut out = Vec::with_capacity(UDP_HEADER_LEN + ct.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(&ct);
        Ok(out)
    }
}

fn datagram_header(session_tag: u32, seq: u64) -> [u8; UDP_HEADER_LEN] {
    let mut h = [0u8; UDP_HEADER_LEN];
    h[0] = UDP_KIND_DATA;
    h[1..5].copy_from_slice(&session_tag.to_le_bytes());
    h[5..].copy_from_slice(&seq.to_le_bytes());
    h
}

/// Reads the session tag so the receiver can pick the right session before decrypting.
pub fn datagram_session_tag(datagram: &[u8]) -> Option<u32> {
    if datagram.len() < UDP_HEADER_LEN + TAG_LEN || datagram[0] != UDP_KIND_DATA {
        return None;
    }
    Some(u32::from_le_bytes(datagram[1..5].try_into().ok()?))
}

pub struct DatagramOpener {
    aead: ChaCha20Poly1305,
    window: ReplayWindow,
}

impl DatagramOpener {
    pub fn new(key: &[u8; 32]) -> Self {
        Self { aead: cipher(key), window: ReplayWindow::default() }
    }

    pub fn open(&mut self, datagram: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if datagram_session_tag(datagram).is_none() || datagram.len() > MAX_DATAGRAM {
            return Err(CryptoError::Malformed);
        }
        let (header, body) = datagram.split_at(UDP_HEADER_LEN);
        let seq = u64::from_le_bytes(header[5..].try_into().map_err(|_| CryptoError::Malformed)?);
        if !self.window.check(seq) {
            return Err(CryptoError::Replay);
        }
        let pt =
            self.aead.decrypt(&nonce(seq).into(), Payload { msg: body, aad: header }).map_err(|_| CryptoError::Auth)?;
        // Only authenticated packets may move the window, or a forged seq could stall it.
        self.window.accept(seq);
        Ok(pt)
    }
}

/// 64-entry sliding window over datagram sequence numbers.
#[derive(Default)]
pub struct ReplayWindow {
    highest: Option<u64>,
    bitmap: u64,
}

impl ReplayWindow {
    pub fn check(&self, seq: u64) -> bool {
        match self.highest {
            None => true,
            Some(h) if seq > h => true,
            Some(h) => {
                let age = h - seq;
                age < 64 && self.bitmap & (1u64 << age) == 0
            }
        }
    }

    pub fn accept(&mut self, seq: u64) {
        match self.highest {
            None => {
                self.highest = Some(seq);
                self.bitmap = 1;
            }
            Some(h) if seq > h => {
                let shift = seq - h;
                self.bitmap = if shift >= 64 { 0 } else { self.bitmap << shift };
                self.bitmap |= 1;
                self.highest = Some(seq);
            }
            Some(h) => {
                let age = h - seq;
                if age < 64 {
                    self.bitmap |= 1u64 << age;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> SessionKeys {
        SessionKeys::derive(&[7u8; 32], b"hello", b"reply")
    }

    #[test]
    fn rfc8439_aead_vector() {
        // RFC 8439 section 2.8.2.
        let key: [u8; 32] = hex::decode("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f")
            .unwrap()
            .try_into()
            .unwrap();
        let nonce: [u8; 12] = hex::decode("070000004041424344454647").unwrap().try_into().unwrap();
        let aad = hex::decode("50515253c0c1c2c3c4c5c6c7").unwrap();
        let pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let out = cipher(&key).encrypt(&nonce.into(), Payload { msg: pt, aad: &aad }).unwrap();
        assert_eq!(hex::encode(&out[pt.len()..]), "1ae10b594f09e26a7e902ecbd0600691");
        assert_eq!(hex::encode(&out[..16]), "d31a8d34648e60db7b86afbc53ef7ec2");
    }

    #[test]
    fn session_keys_are_distinct_and_bound_to_transcript() {
        let k = keys();
        let all = [k.tcp_c2s, k.tcp_s2c, k.udp_c2s, k.udp_s2c];
        for i in 0..4 {
            for j in i + 1..4 {
                assert_ne!(all[i], all[j]);
            }
        }
        assert_ne!(SessionKeys::derive(&[7u8; 32], b"hello", b"replz").tcp_c2s, k.tcp_c2s);
        assert_ne!(SessionKeys::derive(&[8u8; 32], b"hello", b"reply").tcp_c2s, k.tcp_c2s);
    }

    #[test]
    fn frame_round_trip_in_order() {
        let k = keys();
        let mut tx = FrameCipher::new(&k.tcp_c2s);
        let mut rx = FrameCipher::new(&k.tcp_c2s);
        for msg in [&b"one"[..], b"", b"three"] {
            let body = tx.seal(msg).unwrap();
            assert_eq!(body.len(), msg.len() + TAG_LEN);
            assert_eq!(rx.open(&body).unwrap(), msg);
        }
    }

    #[test]
    fn frame_rejects_tamper_reorder_and_wrong_key() {
        let k = keys();
        let mut tx = FrameCipher::new(&k.tcp_c2s);
        let a = tx.seal(b"a").unwrap();
        let b = tx.seal(b"b").unwrap();
        assert_eq!(FrameCipher::new(&k.tcp_c2s).open(&b), Err(CryptoError::Auth), "out of order");
        assert_eq!(FrameCipher::new(&k.tcp_s2c).open(&a), Err(CryptoError::Auth), "wrong key");
        let mut bad = a.clone();
        bad[0] ^= 1;
        assert_eq!(FrameCipher::new(&k.tcp_c2s).open(&bad), Err(CryptoError::Auth), "tampered");
        assert_eq!(FrameCipher::new(&k.tcp_c2s).open(&a[..4]), Err(CryptoError::Auth), "short");
    }

    #[test]
    fn datagram_round_trip_and_replay() {
        let k = keys();
        let mut tx = DatagramSealer::new(&k.udp_s2c, 0xabcd);
        let mut rx = DatagramOpener::new(&k.udp_s2c);
        let d0 = tx.seal(b"frame0").unwrap();
        let d1 = tx.seal(b"frame1").unwrap();
        assert_eq!(datagram_session_tag(&d0), Some(0xabcd));
        assert_eq!(rx.open(&d1).unwrap(), b"frame1");
        assert_eq!(rx.open(&d0).unwrap(), b"frame0", "reordered within window is fine");
        assert_eq!(rx.open(&d0), Err(CryptoError::Replay));
        let mut forged = d1.clone();
        forged[5] = 0xff; // claims a far-future seq
        assert_eq!(rx.open(&forged), Err(CryptoError::Auth));
        assert!(tx.seal(&[0u8; MAX_DATAGRAM]).is_err());
    }

    #[test]
    fn replay_window_edges() {
        let mut w = ReplayWindow::default();
        w.accept(100);
        assert!(w.check(37));
        assert!(!w.check(36), "older than 64 entries");
        assert!(!w.check(100));
        w.accept(300);
        assert!(!w.check(236));
        assert!(w.check(237));
        assert!(w.check(299));
        w.accept(299);
        assert!(!w.check(299));
    }
}
