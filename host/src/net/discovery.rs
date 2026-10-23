//! UDP broadcast discovery: the PSP asks, the host announces its ports.

use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::{debug, warn};

use super::codec::{DecodeError, Reader, Writer};
use super::handshake::{Id16, MAGIC, PROTOCOL_VERSION};
use super::ratelimit::UDP_OVERHEAD;
use crate::state::AppState;

const KIND_DISCOVER: u8 = 0x01;
const KIND_ANNOUNCE: u8 = 0x02;
pub const REQUEST_LEN: usize = 14;
pub const FLAG_PAIRING_OPEN: u8 = 0x01;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announce {
    pub nonce: [u8; 8],
    pub host_id: Id16,
    pub control_port: u16,
    pub audio_port: u16,
    pub flags: u8,
    pub host_name: String,
}

pub fn encode_request(nonce: &[u8; 8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.bytes(MAGIC).u8(PROTOCOL_VERSION).u8(KIND_DISCOVER).bytes(nonce);
    w.into_vec()
}

pub fn decode_request(buf: &[u8]) -> Result<[u8; 8], DecodeError> {
    let mut r = Reader::new(buf);
    if r.bytes(4)? != MAGIC || r.u8()? != PROTOCOL_VERSION {
        return Err(DecodeError::BadHeader);
    }
    let kind = r.u8()?;
    if kind != KIND_DISCOVER {
        return Err(DecodeError::BadKind(kind));
    }
    let nonce = r.array()?;
    r.finish()?;
    Ok(nonce)
}

impl Announce {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(MAGIC)
            .u8(PROTOCOL_VERSION)
            .u8(KIND_ANNOUNCE)
            .bytes(&self.nonce)
            .bytes(&self.host_id)
            .u16(self.control_port)
            .u16(self.audio_port)
            .u8(self.flags)
            .str8(super::codec::truncate_utf8(&self.host_name, 32));
        w.into_vec()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        if r.bytes(4)? != MAGIC || r.u8()? != PROTOCOL_VERSION {
            return Err(DecodeError::BadHeader);
        }
        let kind = r.u8()?;
        if kind != KIND_ANNOUNCE {
            return Err(DecodeError::BadKind(kind));
        }
        let a = Self {
            nonce: r.array()?,
            host_id: r.array()?,
            control_port: r.u16()?,
            audio_port: r.u16()?,
            flags: r.u8()?,
            host_name: r.str8()?,
        };
        r.finish()?;
        Ok(a)
    }
}

pub async fn run(state: Arc<AppState>, socket: UdpSocket) {
    let mut buf = [0u8; 64];
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                // Windows reports ICMP port-unreachable from earlier sends as a recv error; keep serving.
                debug!("discovery recv error: {e}");
                continue;
            }
        };
        let nonce = match decode_request(&buf[..n]) {
            Ok(nonce) => nonce,
            Err(e) => {
                debug!(%from, "ignoring discovery datagram: {e}");
                continue;
            }
        };
        let reply = state.announce(nonce).encode();
        state.record_rx(n + UDP_OVERHEAD);
        if !state.limiter.try_acquire(reply.len() + UDP_OVERHEAD) {
            debug!(%from, "discovery reply dropped by rate limiter");
            continue;
        }
        match socket.send_to(&reply, from).await {
            Ok(sent) => {
                state.record_tx(sent + UDP_OVERHEAD);
                debug!(%from, "announced");
            }
            Err(e) => warn!(%from, "discovery reply failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip() {
        let req = encode_request(&[9; 8]);
        assert_eq!(req.len(), REQUEST_LEN);
        assert_eq!(decode_request(&req).unwrap(), [9; 8]);
        assert!(decode_request(&req[..13]).is_err());
        let mut bad = req.clone();
        bad[5] = KIND_ANNOUNCE;
        assert!(decode_request(&bad).is_err());
    }

    #[test]
    fn announce_round_trip() {
        let a = Announce {
            nonce: [1; 8],
            host_id: [2; 16],
            control_port: 47861,
            audio_port: 47862,
            flags: FLAG_PAIRING_OPEN,
            host_name: "DESKTOP-1".into(),
        };
        let bytes = a.encode();
        assert_eq!(bytes.len(), 6 + 8 + 16 + 2 + 2 + 1 + 1 + 9);
        assert_eq!(Announce::decode(&bytes).unwrap(), a);
    }
}
