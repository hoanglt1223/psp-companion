//! PIN pairing: SPAKE2 over edwards25519 (RFC 9382 profile in protocol/PROTOCOL.md).
//!
//! Both roles live here: the host role is used by the server, the PSP role by
//! tests and the C cross-check harness.

use std::time::{Duration, Instant};

use curve25519_dalek::edwards::{CompressedEdwardsY, EdwardsPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::IsIdentity;
use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256, Sha512};
use thiserror::Error;

use crate::net::crypto::random_array;
use crate::net::handshake::{Id16, Status};

pub const PIN_LIFETIME: Duration = Duration::from_secs(120);
pub const PIN_MAX_ATTEMPTS: u8 = 3;

const M_COMPRESSED: &str = "d048032c6ea0b6d697ddc2e86bda85a33adac920f1bf18e1b0c6d166a5cecdaf";
const N_COMPRESSED: &str = "d3bfb518f44f3430f29d0c92af503865a1ed3281dc69b35dd868ba85f886c4ab";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PairingError {
    #[error("PIN must be exactly six digits")]
    BadPin,
    #[error("peer sent an invalid curve point")]
    BadPoint,
    #[error("key confirmation failed (wrong PIN)")]
    Confirmation,
}

fn const_point(hex_str: &str) -> EdwardsPoint {
    let bytes: [u8; 32] = hex::decode(hex_str).expect("constant is hex").try_into().expect("32 bytes");
    CompressedEdwardsY(bytes).decompress().expect("RFC 9382 constant is a valid point")
}

pub fn validate_pin(pin: &str) -> Result<(), PairingError> {
    if pin.len() == 6 && pin.bytes().all(|b| b.is_ascii_digit()) { Ok(()) } else { Err(PairingError::BadPin) }
}

pub fn password_scalar(pin: &str) -> Result<Scalar, PairingError> {
    validate_pin(pin)?;
    let digest: [u8; 64] = Sha512::new().chain_update(b"PSPC-PIN-v1").chain_update(pin.as_bytes()).finalize().into();
    Ok(Scalar::from_bytes_mod_order_wide(&digest))
}

/// Uniform six-digit PIN (rejection sampling avoids modulo bias).
pub fn generate_pin() -> String {
    const LIMIT: u32 = u32::MAX - (u32::MAX % 1_000_000);
    loop {
        let v = u32::from_le_bytes(random_array());
        if v < LIMIT {
            return format!("{:06}", v % 1_000_000);
        }
    }
}

fn decompress(bytes: &[u8; 32]) -> Result<EdwardsPoint, PairingError> {
    CompressedEdwardsY(*bytes).decompress().ok_or(PairingError::BadPoint)
}

fn shared_point(secret: &Scalar, peer: &[u8; 32], w: &Scalar, blind: &EdwardsPoint) -> Result<[u8; 32], PairingError> {
    let k = (secret * (decompress(peer)? - w * blind)).mul_by_cofactor();
    if k.is_identity() {
        return Err(PairingError::BadPoint);
    }
    Ok(k.compress().to_bytes())
}

fn transcript(a: &[u8], b: &[u8], pa: &[u8; 32], pb: &[u8; 32], k: &[u8; 32], w: &Scalar) -> Vec<u8> {
    let mut tt = Vec::with_capacity(6 * 8 + 2 * 16 + 4 * 32);
    for part in [a, b, &pa[..], &pb[..], &k[..], &w.to_bytes()[..]] {
        tt.extend_from_slice(&(part.len() as u64).to_le_bytes());
        tt.extend_from_slice(part);
    }
    tt
}

struct Derived {
    ca: [u8; 32],
    cb: [u8; 32],
    pairing_key: [u8; 32],
}

fn hmac256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

fn derive(tt: &[u8]) -> Derived {
    let h: [u8; 64] = Sha512::digest(tt).into();
    let (ke, ka) = h.split_at(32);
    let mut kc = [0u8; 64];
    Hkdf::<Sha256>::new(None, ka).expand(b"ConfirmationKeys", &mut kc).expect("64 bytes is a valid HKDF length");
    let mut pairing_key = [0u8; 32];
    Hkdf::<Sha256>::new(None, ke)
        .expand(b"PSPC v1 pairing key", &mut pairing_key)
        .expect("32 bytes is a valid HKDF length");
    Derived { ca: hmac256(&kc[..32], tt), cb: hmac256(&kc[32..], tt), pairing_key }
}

fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Host (party B) state between PairChallenge and PairConfirm.
pub struct HostPairing {
    pub pb: [u8; 32],
    pub cb: [u8; 32],
    expected_ca: [u8; 32],
    pairing_key: [u8; 32],
}

impl HostPairing {
    pub fn respond(pin: &str, device_id: &Id16, host_id: &Id16, pa: &[u8; 32]) -> Result<Self, PairingError> {
        Self::respond_with_randomness(pin, device_id, host_id, pa, &random_array())
    }

    /// `y_wide` is 64 random bytes; exposed so cross-implementation tests can be deterministic.
    pub fn respond_with_randomness(
        pin: &str,
        device_id: &Id16,
        host_id: &Id16,
        pa: &[u8; 32],
        y_wide: &[u8; 64],
    ) -> Result<Self, PairingError> {
        let w = password_scalar(pin)?;
        let y = Scalar::from_bytes_mod_order_wide(y_wide);
        let pb = (EdwardsPoint::mul_base(&y) + w * const_point(N_COMPRESSED)).compress().to_bytes();
        let k = shared_point(&y, pa, &w, &const_point(M_COMPRESSED))?;
        let d = derive(&transcript(device_id, host_id, pa, &pb, &k, &w));
        Ok(Self { pb, cb: d.cb, expected_ca: d.ca, pairing_key: d.pairing_key })
    }

    pub fn verify(self, ca: &[u8; 32]) -> Result<[u8; 32], PairingError> {
        if ct_eq(ca, &self.expected_ca) { Ok(self.pairing_key) } else { Err(PairingError::Confirmation) }
    }
}

/// PSP (party A) role.
pub struct ClientPairing {
    x: Scalar,
    w: Scalar,
    device_id: Id16,
    pub pa: [u8; 32],
}

pub struct ClientPaired {
    pub ca: [u8; 32],
    pub pairing_key: [u8; 32],
}

impl ClientPairing {
    pub fn start(pin: &str, device_id: Id16) -> Result<Self, PairingError> {
        Self::start_with_randomness(pin, device_id, &random_array())
    }

    pub fn start_with_randomness(pin: &str, device_id: Id16, x_wide: &[u8; 64]) -> Result<Self, PairingError> {
        let w = password_scalar(pin)?;
        let x = Scalar::from_bytes_mod_order_wide(x_wide);
        let pa = (EdwardsPoint::mul_base(&x) + w * const_point(M_COMPRESSED)).compress().to_bytes();
        Ok(Self { x, w, device_id, pa })
    }

    pub fn finish(self, host_id: &Id16, pb: &[u8; 32], cb: &[u8; 32]) -> Result<ClientPaired, PairingError> {
        let k = shared_point(&self.x, pb, &self.w, &const_point(N_COMPRESSED))?;
        let d = derive(&transcript(&self.device_id, host_id, &self.pa, pb, &k, &self.w));
        if !ct_eq(cb, &d.cb) {
            return Err(PairingError::Confirmation);
        }
        Ok(ClientPaired { ca: d.ca, pairing_key: d.pairing_key })
    }
}

/// The PIN currently shown to the user, with its lifetime and attempt budget.
#[derive(Debug, Clone)]
pub struct PendingPin {
    pin: String,
    expires: Instant,
    attempts_left: u8,
}

impl PendingPin {
    pub fn new(pin: String, now: Instant) -> Self {
        Self { pin, expires: now + PIN_LIFETIME, attempts_left: PIN_MAX_ATTEMPTS }
    }

    pub fn pin(&self) -> &str {
        &self.pin
    }

    pub fn is_open(&self, now: Instant) -> bool {
        now < self.expires && self.attempts_left > 0
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        self.expires.saturating_duration_since(now)
    }

    /// Consumes one attempt. Returns the PIN to use, or the status to send back.
    pub fn take_attempt(&mut self, now: Instant) -> Result<String, Status> {
        if now >= self.expires {
            return Err(Status::Unknown);
        }
        if self.attempts_left == 0 {
            return Err(Status::TooManyAttempts);
        }
        self.attempts_left -= 1;
        Ok(self.pin.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV: Id16 = [0x11; 16];
    const HOST: Id16 = [0x22; 16];

    #[test]
    fn rfc_points_are_valid_and_distinct() {
        let m = const_point(M_COMPRESSED);
        let n = const_point(N_COMPRESSED);
        assert_ne!(m, n);
        assert!(m.is_torsion_free() && n.is_torsion_free());
    }

    #[test]
    fn same_pin_derives_same_key() {
        let client = ClientPairing::start("123456", DEV).unwrap();
        let host = HostPairing::respond("123456", &DEV, &HOST, &client.pa).unwrap();
        let (pb, cb) = (host.pb, host.cb);
        let paired = client.finish(&HOST, &pb, &cb).unwrap();
        let host_key = host.verify(&paired.ca).unwrap();
        assert_eq!(host_key, paired.pairing_key);
    }

    #[test]
    fn wrong_pin_fails_on_both_sides() {
        let client = ClientPairing::start("123456", DEV).unwrap();
        let host = HostPairing::respond("123457", &DEV, &HOST, &client.pa).unwrap();
        let (pb, cb) = (host.pb, host.cb);
        assert_eq!(client.finish(&HOST, &pb, &cb).err(), Some(PairingError::Confirmation));
        assert_eq!(host.verify(&[0u8; 32]), Err(PairingError::Confirmation));
    }

    #[test]
    fn identities_are_bound() {
        let client = ClientPairing::start("000000", DEV).unwrap();
        let host = HostPairing::respond("000000", &DEV, &[0x33; 16], &client.pa).unwrap();
        let (pb, cb) = (host.pb, host.cb);
        assert!(client.finish(&HOST, &pb, &cb).is_err());
    }

    #[test]
    fn deterministic_with_fixed_randomness() {
        let a = ClientPairing::start_with_randomness("654321", DEV, &[1; 64]).unwrap();
        let b = ClientPairing::start_with_randomness("654321", DEV, &[1; 64]).unwrap();
        assert_eq!(a.pa, b.pa);
        let h1 = HostPairing::respond_with_randomness("654321", &DEV, &HOST, &a.pa, &[2; 64]).unwrap();
        let h2 = HostPairing::respond_with_randomness("654321", &DEV, &HOST, &a.pa, &[2; 64]).unwrap();
        assert_eq!((h1.pb, h1.cb), (h2.pb, h2.cb));
    }

    #[test]
    fn rejects_bad_points_and_pins() {
        assert_eq!(password_scalar("12345").err(), Some(PairingError::BadPin));
        assert_eq!(password_scalar("12345a").err(), Some(PairingError::BadPin));
        assert_eq!(password_scalar("１２３４５６").err(), Some(PairingError::BadPin));
        // y = 2 is not on the curve (x^2 would be a non-square).
        let mut off_curve = [0u8; 32];
        off_curve[0] = 2;
        assert!(CompressedEdwardsY(off_curve).decompress().is_none());
        assert_eq!(HostPairing::respond("123456", &DEV, &HOST, &off_curve).err(), Some(PairingError::BadPoint));
        // pA = w*M makes K the identity: an attacker probing with a zero ephemeral key.
        let w = password_scalar("123456").unwrap();
        let pa = (w * const_point(M_COMPRESSED)).compress().to_bytes();
        assert_eq!(HostPairing::respond("123456", &DEV, &HOST, &pa).err(), Some(PairingError::BadPoint));
    }

    #[test]
    fn pin_is_six_digits() {
        for _ in 0..100 {
            validate_pin(&generate_pin()).unwrap();
        }
    }

    #[test]
    fn pending_pin_expiry_and_attempts() {
        let t0 = Instant::now();
        let mut p = PendingPin::new("111111".into(), t0);
        assert!(p.is_open(t0));
        for _ in 0..PIN_MAX_ATTEMPTS {
            assert_eq!(p.take_attempt(t0).unwrap(), "111111");
        }
        assert_eq!(p.take_attempt(t0), Err(Status::TooManyAttempts));
        assert!(!p.is_open(t0));
        let mut p = PendingPin::new("222222".into(), t0);
        assert_eq!(p.take_attempt(t0 + PIN_LIFETIME), Err(Status::Unknown));
    }
}
