//! FFI bindings to `psp/crypto.c`, used only by this crate's tests.

use std::os::raw::{c_char, c_int};

#[repr(C)]
pub struct PairClient {
    pub x: [u8; 32],
    pub w: [u8; 32],
    pub device_id: [u8; 16],
    pub pa: [u8; 32],
}

unsafe extern "C" {
    pub fn pspc_sha256(data: *const u8, n: usize, out: *mut u8);
    pub fn pspc_sha512(data: *const u8, n: usize, out: *mut u8);
    pub fn pspc_hmac_sha256(key: *const u8, key_len: usize, msg: *const u8, msg_len: usize, out: *mut u8);
    pub fn pspc_hkdf_sha256(
        salt: *const u8,
        salt_len: usize,
        ikm: *const u8,
        ikm_len: usize,
        info: *const u8,
        info_len: usize,
        out: *mut u8,
        out_len: usize,
    ) -> c_int;
    pub fn pspc_poly1305(key: *const u8, msg: *const u8, n: usize, tag: *mut u8);
    pub fn pspc_aead_seal(
        key: *const u8,
        nonce: *const u8,
        aad: *const u8,
        aad_len: usize,
        pt: *const u8,
        len: usize,
        out: *mut u8,
    );
    pub fn pspc_aead_open(
        key: *const u8,
        nonce: *const u8,
        aad: *const u8,
        aad_len: usize,
        ct: *const u8,
        ct_len: usize,
        out: *mut u8,
    ) -> c_int;
    pub fn pspc_nonce(counter: u64, out: *mut u8);
    pub fn pspc_pair_start(c: *mut PairClient, pin: *const c_char, device_id: *const u8, x_wide: *const u8) -> c_int;
    pub fn pspc_pair_finish(
        c: *mut PairClient,
        host_id: *const u8,
        pb: *const u8,
        cb: *const u8,
        ca_out: *mut u8,
        key_out: *mut u8,
    ) -> c_int;
    pub fn pspc_session_keys(
        pairing_key: *const u8,
        hello: *const u8,
        hello_len: usize,
        reply: *const u8,
        reply_len: usize,
        out: *mut u8,
    );
}

#[repr(C)]
#[derive(Debug)]
pub struct Announce {
    pub nonce: [u8; 8],
    pub host_id: [u8; 16],
    pub control_port: u16,
    pub audio_port: u16,
    pub flags: u8,
    pub host_name: [u8; 33],
}

#[repr(C)]
#[derive(Debug)]
pub struct HelloAck {
    pub caps: u32,
    pub host_name: [u8; 33],
    pub session_tag: u32,
    pub audio_port: u16,
    pub keepalive_s: u16,
    pub budget_kbps: u16,
}

unsafe extern "C" {
    pub fn pspc_build_discover(out: *mut u8, nonce: *const u8) -> usize;
    pub fn pspc_parse_announce(buf: *const u8, len: usize, out: *mut Announce) -> c_int;
    pub fn pspc_build_session_hello(out: *mut u8, device_id: *const u8, nonce: *const u8) -> usize;
    pub fn pspc_parse_session_reply(
        buf: *const u8,
        len: usize,
        status: *mut u8,
        host_id: *mut u8,
        nonce: *mut u8,
    ) -> c_int;
    pub fn pspc_build_pair_request(out: *mut u8, device_id: *const u8, pa: *const u8, name: *const c_char) -> usize;
    pub fn pspc_parse_pair_challenge(
        buf: *const u8,
        len: usize,
        status: *mut u8,
        host_id: *mut u8,
        pb: *mut u8,
        cb: *mut u8,
    ) -> c_int;
    pub fn pspc_build_pair_confirm(out: *mut u8, ca: *const u8) -> usize;
    pub fn pspc_parse_pair_result(buf: *const u8, len: usize, status: *mut u8) -> c_int;
    pub fn pspc_build_hello(out: *mut u8, caps: u32, sw: *const c_char, name: *const c_char) -> usize;
    pub fn pspc_build_ping(out: *mut u8, id: u32, t_us: u64) -> usize;
    pub fn pspc_build_bye(out: *mut u8, reason: u8) -> usize;
    pub fn pspc_build_audio_register(out: *mut u8) -> usize;
    pub fn pspc_parse_hello_ack(buf: *const u8, len: usize, out: *mut HelloAck) -> c_int;
    pub fn pspc_parse_pong(buf: *const u8, len: usize, id: *mut u32, t_us: *mut u64) -> c_int;
    pub fn pspc_parse_bye(buf: *const u8, len: usize, reason: *mut u8) -> c_int;
    pub fn pspc_parse_error(buf: *const u8, len: usize, code: *mut u16, text: *mut c_char, cap: usize) -> c_int;
}

/// NUL-terminated C string buffer to a Rust string.
pub fn c_str(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

impl Default for Announce {
    fn default() -> Self {
        Self { nonce: [0; 8], host_id: [0; 16], control_port: 0, audio_port: 0, flags: 0, host_name: [0; 33] }
    }
}

impl Default for HelloAck {
    fn default() -> Self {
        Self { caps: 0, host_name: [0; 33], session_tag: 0, audio_port: 0, keepalive_s: 0, budget_kbps: 0 }
    }
}
