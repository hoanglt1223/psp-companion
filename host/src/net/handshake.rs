//! Cleartext handshake messages carried in the first frames of a TCP connection.

use super::codec::{DecodeError, Reader, Writer};

pub const MAGIC: &[u8; 4] = b"PSPC";
pub const PROTOCOL_VERSION: u8 = 1;

pub type Id16 = [u8; 16];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Ok = 0,
    Unknown = 1,
    Version = 2,
    TooManyAttempts = 3,
    WrongPin = 4,
    Busy = 5,
}

impl Status {
    fn from_u8(v: u8) -> Result<Self, DecodeError> {
        Ok(match v {
            0 => Self::Ok,
            1 => Self::Unknown,
            2 => Self::Version,
            3 => Self::TooManyAttempts,
            4 => Self::WrongPin,
            5 => Self::Busy,
            _ => return Err(DecodeError::OutOfRange("status")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handshake {
    SessionHello { device_id: Id16, client_nonce: [u8; 16] },
    SessionReply { status: Status, host_id: Id16, server_nonce: [u8; 16] },
    PairRequest { device_id: Id16, pa: [u8; 32], device_name: String },
    PairChallenge { status: Status, host_id: Id16, pb: [u8; 32], cb: [u8; 32] },
    PairConfirm { ca: [u8; 32] },
    PairResult { status: Status },
}

mod kind {
    pub const SESSION_HELLO: u8 = 0x01;
    pub const SESSION_REPLY: u8 = 0x02;
    pub const PAIR_REQUEST: u8 = 0x03;
    pub const PAIR_CHALLENGE: u8 = 0x04;
    pub const PAIR_CONFIRM: u8 = 0x05;
    pub const PAIR_RESULT: u8 = 0x06;
}

/// Version is reported separately so the receiver can answer a mismatch with status 2.
#[derive(Debug, PartialEq, Eq)]
pub enum ParsedHandshake {
    Message(Handshake),
    UnsupportedVersion(u8),
}

impl Handshake {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(MAGIC).u8(PROTOCOL_VERSION);
        match self {
            Self::SessionHello { device_id, client_nonce } => {
                w.u8(kind::SESSION_HELLO).bytes(device_id).bytes(client_nonce);
            }
            Self::SessionReply { status, host_id, server_nonce } => {
                w.u8(kind::SESSION_REPLY).u8(*status as u8).bytes(host_id).bytes(server_nonce);
            }
            Self::PairRequest { device_id, pa, device_name } => {
                w.u8(kind::PAIR_REQUEST).bytes(device_id).bytes(pa).str8(device_name);
            }
            Self::PairChallenge { status, host_id, pb, cb } => {
                w.u8(kind::PAIR_CHALLENGE).u8(*status as u8).bytes(host_id).bytes(pb).bytes(cb);
            }
            Self::PairConfirm { ca } => {
                w.u8(kind::PAIR_CONFIRM).bytes(ca);
            }
            Self::PairResult { status } => {
                w.u8(kind::PAIR_RESULT).u8(*status as u8);
            }
        }
        w.into_vec()
    }

    pub fn decode(buf: &[u8]) -> Result<ParsedHandshake, DecodeError> {
        let mut r = Reader::new(buf);
        if r.bytes(4)? != MAGIC {
            return Err(DecodeError::BadHeader);
        }
        let version = r.u8()?;
        if version != PROTOCOL_VERSION {
            return Ok(ParsedHandshake::UnsupportedVersion(version));
        }
        let msg = match r.u8()? {
            kind::SESSION_HELLO => Self::SessionHello { device_id: r.array()?, client_nonce: r.array()? },
            kind::SESSION_REPLY => {
                Self::SessionReply { status: Status::from_u8(r.u8()?)?, host_id: r.array()?, server_nonce: r.array()? }
            }
            kind::PAIR_REQUEST => Self::PairRequest { device_id: r.array()?, pa: r.array()?, device_name: r.str8()? },
            kind::PAIR_CHALLENGE => Self::PairChallenge {
                status: Status::from_u8(r.u8()?)?,
                host_id: r.array()?,
                pb: r.array()?,
                cb: r.array()?,
            },
            kind::PAIR_CONFIRM => Self::PairConfirm { ca: r.array()? },
            kind::PAIR_RESULT => Self::PairResult { status: Status::from_u8(r.u8()?)? },
            other => return Err(DecodeError::BadKind(other)),
        };
        r.finish()?;
        Ok(ParsedHandshake::Message(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let msgs = [
            Handshake::SessionHello { device_id: [1; 16], client_nonce: [2; 16] },
            Handshake::SessionReply { status: Status::Ok, host_id: [3; 16], server_nonce: [4; 16] },
            Handshake::PairRequest { device_id: [5; 16], pa: [6; 32], device_name: "PSP".into() },
            Handshake::PairChallenge { status: Status::Ok, host_id: [7; 16], pb: [8; 32], cb: [9; 32] },
            Handshake::PairConfirm { ca: [10; 32] },
            Handshake::PairResult { status: Status::WrongPin },
        ];
        for m in msgs {
            let b = m.encode();
            assert_eq!(&b[..5], b"PSPC\x01");
            assert_eq!(Handshake::decode(&b).unwrap(), ParsedHandshake::Message(m));
        }
    }

    #[test]
    fn session_hello_is_38_bytes() {
        let b = Handshake::SessionHello { device_id: [0; 16], client_nonce: [0; 16] }.encode();
        assert_eq!(b.len(), 38);
        assert_eq!(b[5], 0x01);
    }

    #[test]
    fn version_and_magic_checks() {
        let mut b = Handshake::PairConfirm { ca: [0; 32] }.encode();
        b[4] = 2;
        assert_eq!(Handshake::decode(&b).unwrap(), ParsedHandshake::UnsupportedVersion(2));
        b[0] = b'X';
        assert_eq!(Handshake::decode(&b), Err(DecodeError::BadHeader));
        let mut b = Handshake::PairResult { status: Status::Ok }.encode();
        b.push(0);
        assert_eq!(Handshake::decode(&b), Err(DecodeError::Trailing(1)));
        assert_eq!(Handshake::decode(b"PSPC\x01\x09"), Err(DecodeError::BadKind(9)));
    }
}
