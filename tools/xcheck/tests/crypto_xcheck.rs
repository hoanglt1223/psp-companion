//! The PSP's C crypto must produce byte-identical results to the host's Rust crypto.

use std::ffi::CString;

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use companion_host::net::crypto::{FrameCipher, SessionKeys, random_array};
use companion_host::net::handshake::Id16;
use companion_host::pairing::{ClientPairing, HostPairing};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use pspc_xcheck::*;
use sha2::{Digest, Sha256, Sha512};

fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

#[test]
fn sha2_matches() {
    for n in [0, 1, 55, 56, 63, 64, 65, 111, 112, 127, 128, 129, 1000] {
        let m = bytes(n, 7);
        let (mut a, mut b) = ([0u8; 32], [0u8; 64]);
        unsafe {
            pspc_sha256(m.as_ptr(), n, a.as_mut_ptr());
            pspc_sha512(m.as_ptr(), n, b.as_mut_ptr());
        }
        assert_eq!(a[..], Sha256::digest(&m)[..], "sha256 len {n}");
        assert_eq!(b[..], Sha512::digest(&m)[..], "sha512 len {n}");
    }
}

#[test]
fn hmac_and_hkdf_match() {
    for (klen, mlen) in [(0, 0), (16, 3), (32, 64), (64, 100), (100, 1)] {
        let k = bytes(klen, 1);
        let m = bytes(mlen, 2);
        let mut out = [0u8; 32];
        unsafe { pspc_hmac_sha256(k.as_ptr(), klen, m.as_ptr(), mlen, out.as_mut_ptr()) };
        let mut mac = Hmac::<Sha256>::new_from_slice(&k).unwrap();
        mac.update(&m);
        assert_eq!(out[..], mac.finalize().into_bytes()[..], "hmac {klen}/{mlen}");
    }
    for len in [1, 32, 33, 64, 128, 200] {
        let (salt, ikm, info) = (bytes(13, 3), bytes(32, 4), b"info");
        let mut c = vec![0u8; len];
        let mut c_nosalt = vec![0u8; len];
        let rc = unsafe {
            pspc_hkdf_sha256(salt.as_ptr(), salt.len(), ikm.as_ptr(), 32, info.as_ptr(), 4, c.as_mut_ptr(), len)
                | pspc_hkdf_sha256(std::ptr::null(), 0, ikm.as_ptr(), 32, info.as_ptr(), 4, c_nosalt.as_mut_ptr(), len)
        };
        assert_eq!(rc, 0);
        let mut r = vec![0u8; len];
        Hkdf::<Sha256>::new(Some(&salt), &ikm).expand(info, &mut r).unwrap();
        assert_eq!(c, r, "hkdf {len}");
        Hkdf::<Sha256>::new(None, &ikm).expand(info, &mut r).unwrap();
        assert_eq!(c_nosalt, r, "hkdf no salt {len}");
    }
    let mut big = vec![0u8; 255 * 32 + 1];
    assert_eq!(
        unsafe { pspc_hkdf_sha256(std::ptr::null(), 0, big.as_ptr(), 1, big.as_ptr(), 0, big.as_mut_ptr(), big.len()) },
        -1
    );
}

#[test]
fn poly1305_rfc8439_vector() {
    // RFC 8439 section 2.5.2.
    let key = hex::decode("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b").unwrap();
    let msg = b"Cryptographic Forum Research Group";
    let mut tag = [0u8; 16];
    unsafe { pspc_poly1305(key.as_ptr(), msg.as_ptr(), msg.len(), tag.as_mut_ptr()) };
    assert_eq!(hex::encode(tag), "a8061dc1305136c6c22b8baf0c0127a9");
}

#[test]
fn aead_matches_both_ways() {
    let key: [u8; 32] = random_array();
    for len in [0usize, 1, 15, 16, 17, 63, 64, 65, 300, 1184, 16384] {
        for aad_len in [0usize, 2, 13, 16, 17] {
            let pt = bytes(len, 9);
            let aad = bytes(aad_len, 5);
            let mut nonce = [0u8; 12];
            unsafe { pspc_nonce(len as u64 * 1000 + aad_len as u64, nonce.as_mut_ptr()) };
            assert_eq!(nonce[..4], [0; 4]);

            let mut c_ct = vec![0u8; len + 16];
            unsafe {
                pspc_aead_seal(key.as_ptr(), nonce.as_ptr(), aad.as_ptr(), aad_len, pt.as_ptr(), len, c_ct.as_mut_ptr())
            };
            let r_ct =
                ChaCha20Poly1305::new(&key.into()).encrypt(&nonce.into(), Payload { msg: &pt, aad: &aad }).unwrap();
            assert_eq!(c_ct, r_ct, "seal len {len} aad {aad_len}");

            let mut out = vec![0u8; len];
            let rc = unsafe {
                pspc_aead_open(
                    key.as_ptr(),
                    nonce.as_ptr(),
                    aad.as_ptr(),
                    aad_len,
                    r_ct.as_ptr(),
                    r_ct.len(),
                    out.as_mut_ptr(),
                )
            };
            assert_eq!((rc, &out), (0, &pt));

            let mut bad = r_ct.clone();
            *bad.last_mut().unwrap() ^= 0x80;
            let rc = unsafe {
                pspc_aead_open(
                    key.as_ptr(),
                    nonce.as_ptr(),
                    aad.as_ptr(),
                    aad_len,
                    bad.as_ptr(),
                    bad.len(),
                    out.as_mut_ptr(),
                )
            };
            assert_eq!(rc, -1);
        }
    }
    let mut out = [0u8; 1];
    assert_eq!(
        unsafe { pspc_aead_open(key.as_ptr(), key.as_ptr(), key.as_ptr(), 0, key.as_ptr(), 15, out.as_mut_ptr()) },
        -1
    );
}

fn c_start(pin: &str, device_id: &Id16, x_wide: &[u8; 64]) -> (i32, PairClient) {
    let mut c = PairClient { x: [0; 32], w: [0; 32], device_id: [0; 16], pa: [0; 32] };
    let pin = CString::new(pin).unwrap();
    let rc = unsafe { pspc_pair_start(&mut c, pin.as_ptr(), device_id.as_ptr(), x_wide.as_ptr()) };
    (rc, c)
}

#[test]
fn spake2_c_client_pairs_with_rust_host() {
    for _ in 0..8 {
        let pin = companion_host::pairing::generate_pin();
        let (device_id, host_id): (Id16, Id16) = (random_array(), random_array());
        let (x_wide, y_wide): ([u8; 64], [u8; 64]) = (random_array(), random_array());

        let (rc, mut c) = c_start(&pin, &device_id, &x_wide);
        assert_eq!(rc, 0);
        let rust_client = ClientPairing::start_with_randomness(&pin, device_id, &x_wide).unwrap();
        assert_eq!(c.pa, rust_client.pa, "pA must match the Rust client");

        let host = HostPairing::respond_with_randomness(&pin, &device_id, &host_id, &c.pa, &y_wide).unwrap();
        let (mut ca, mut key) = ([0u8; 32], [0u8; 32]);
        let rc = unsafe {
            pspc_pair_finish(
                &mut c,
                host_id.as_ptr(),
                host.pb.as_ptr(),
                host.cb.as_ptr(),
                ca.as_mut_ptr(),
                key.as_mut_ptr(),
            )
        };
        assert_eq!(rc, 0, "C client must accept the host's cB");
        let rust_paired = rust_client.finish(&host_id, &host.pb, &host.cb).unwrap();
        assert_eq!(ca, rust_paired.ca);
        assert_eq!(key, rust_paired.pairing_key);
        assert_eq!(host.verify(&ca).unwrap(), key, "host accepts the C client's cA");
    }
}

#[test]
fn spake2_c_client_rejects_wrong_pin_and_bad_points() {
    let (device_id, host_id): (Id16, Id16) = (random_array(), random_array());
    let (rc, mut c) = c_start("123456", &device_id, &random_array());
    assert_eq!(rc, 0);
    let host = HostPairing::respond("654321", &device_id, &host_id, &c.pa).unwrap();
    let (mut ca, mut key) = ([0u8; 32], [0u8; 32]);
    let rc = unsafe {
        pspc_pair_finish(
            &mut c,
            host_id.as_ptr(),
            host.pb.as_ptr(),
            host.cb.as_ptr(),
            ca.as_mut_ptr(),
            key.as_mut_ptr(),
        )
    };
    assert_eq!(rc, -3);

    let (_, mut c) = c_start("123456", &device_id, &random_array());
    let mut off_curve = [0u8; 32];
    off_curve[0] = 2;
    let rc = unsafe {
        pspc_pair_finish(
            &mut c,
            host_id.as_ptr(),
            off_curve.as_ptr(),
            [0u8; 32].as_ptr(),
            ca.as_mut_ptr(),
            key.as_mut_ptr(),
        )
    };
    assert_eq!(rc, -2);

    for bad in ["12345", "1234567", "12a456", ""] {
        assert_eq!(c_start(bad, &device_id, &[0; 64]).0, -1, "{bad:?}");
    }
}

#[test]
fn session_keys_and_frames_interoperate() {
    let pairing_key: [u8; 32] = random_array();
    let (hello, reply) = (bytes(38, 1), bytes(39, 2));
    let mut c_keys = [0u8; 128];
    unsafe {
        pspc_session_keys(
            pairing_key.as_ptr(),
            hello.as_ptr(),
            hello.len(),
            reply.as_ptr(),
            reply.len(),
            c_keys.as_mut_ptr(),
        )
    };
    let r = SessionKeys::derive(&pairing_key, &hello, &reply);
    assert_eq!(c_keys[..32], r.tcp_c2s);
    assert_eq!(c_keys[32..64], r.tcp_s2c);
    assert_eq!(c_keys[64..96], r.udp_c2s);
    assert_eq!(c_keys[96..], r.udp_s2c);

    // A C-sealed frame (counter 0 then 1, AAD = u16 length) opens on the host.
    let mut host_rx = FrameCipher::new(&r.tcp_c2s);
    for (counter, msg) in [&b"\x03\x01\x00\x00\x00\x02\x00\x00\x00\x00\x00\x00\x00"[..], b"\x05\x00"].iter().enumerate()
    {
        let body_len = (msg.len() + 16) as u16;
        let mut nonce = [0u8; 12];
        let mut body = vec![0u8; msg.len() + 16];
        unsafe {
            pspc_nonce(counter as u64, nonce.as_mut_ptr());
            pspc_aead_seal(
                c_keys.as_ptr(),
                nonce.as_ptr(),
                body_len.to_le_bytes().as_ptr(),
                2,
                msg.as_ptr(),
                msg.len(),
                body.as_mut_ptr(),
            );
        }
        assert_eq!(host_rx.open(&body).unwrap(), *msg);
    }
}
