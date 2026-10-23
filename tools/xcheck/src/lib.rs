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

// Phase 03: widgets, images, font.
unsafe extern "C" {
    pub fn xc_layout(data: *const u8, len: usize, out: *mut c_char, cap: usize) -> c_int;
    pub fn xc_widget(kind: u8, data: *const u8, len: usize, out: *mut c_char, cap: usize) -> c_int;
    pub fn pspc_parse_widget_data(
        buf: *const u8,
        len: usize,
        widget_id: *mut u16,
        format: *mut u8,
        data: *mut *const u8,
        data_len: *mut usize,
    ) -> c_int;
    pub fn pspc_parse_widget_asset(
        buf: *const u8,
        len: usize,
        asset_id: *mut u64,
        format: *mut u8,
        data: *mut *const u8,
        data_len: *mut usize,
    ) -> c_int;
    pub fn pspc_build_input_event(
        out: *mut u8,
        kind: u8,
        page: u8,
        widget_id: u16,
        x: u16,
        y: u16,
        buttons: u32,
    ) -> usize;
    pub fn pspc_build_macro_trigger(out: *mut u8, slot: u8) -> usize;
    pub fn lz4_block_decode(input: *const u8, in_len: usize, out: *mut u8, out_len: usize) -> c_int;
    pub fn image_decode(
        payload: *const u8,
        len: usize,
        scratch: *mut u8,
        out: *mut u32,
        max_pixels: usize,
        width: *mut c_int,
        height: *mut c_int,
    ) -> c_int;
    pub fn font_text_width(s: *const c_char) -> c_int;
    pub fn font_wrap(
        s: *const c_char,
        max_width: c_int,
        starts: *mut usize,
        lens: *mut usize,
        max_lines: c_int,
    ) -> c_int;
    pub fn utf8_next(s: *mut *const c_char) -> u32;
}

pub const IMAGE_MAX_RAW: usize = 6 + 256 * 3 + 128 * 128;

/// Runs a text-producing shim and returns (rc, text).
pub fn shim_text(f: impl FnOnce(*mut c_char, usize) -> c_int) -> (c_int, String) {
    let mut buf = vec![0u8; 8192];
    let rc = f(buf.as_mut_ptr().cast(), buf.len());
    (rc, c_str(&buf))
}

// Phase 04: audio streams and the jitter buffer.
#[repr(C)]
pub struct JitterBuf {
    _opaque: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Default)]
pub struct StreamStart {
    pub stream_id: u8,
    pub kind: u8,
    pub codec: u8,
    pub channels: u8,
    pub sample_rate: u32,
    pub bitrate_kbps: u16,
}

pub const JB_FRAME: c_int = 0;
pub const JB_LOST: c_int = 1;
pub const JB_SILENCE: c_int = 2;

unsafe extern "C" {
    pub fn xc_jb_new() -> *mut JitterBuf;
    pub fn xc_jb_free(jb: *mut JitterBuf);
    pub fn xc_jb_stats(jb: *const JitterBuf, out: *mut u32);
    pub fn jb_put(jb: *mut JitterBuf, seq: u32, pts_ms: u32, frame: *const u8, len: usize) -> c_int;
    pub fn jb_next(jb: *mut JitterBuf, data: *mut *const u8, len: *mut usize) -> c_int;
    pub fn jb_depth_ms(jb: *const JitterBuf) -> u32;
    pub fn mp3_frame_len(p: *const u8, avail: usize) -> usize;
    pub fn pspc_parse_stream_start(buf: *const u8, len: usize, out: *mut StreamStart) -> c_int;
    pub fn pspc_parse_stream_stop(buf: *const u8, len: usize, stream_id: *mut u8, reason: *mut u8) -> c_int;
    pub fn pspc_parse_audio_packet(
        buf: *const u8,
        len: usize,
        stream_id: *mut u8,
        seq: *mut u32,
        pts_ms: *mut u32,
        data: *mut *const u8,
        data_len: *mut usize,
    ) -> c_int;
    pub fn pspc_build_stream_request(out: *mut u8, kind: u8, enable: u8) -> usize;
    pub fn pspc_build_stream_stop(out: *mut u8, stream_id: u8, reason: u8) -> usize;
}

/// Owns a C jitter buffer.
pub struct Jitter(pub *mut JitterBuf);

impl Jitter {
    pub fn new() -> Self {
        let p = unsafe { xc_jb_new() };
        assert!(!p.is_null());
        Self(p)
    }

    pub fn put(&mut self, seq: u32, pts_ms: u32, frame: &[u8]) -> bool {
        unsafe { jb_put(self.0, seq, pts_ms, frame.as_ptr(), frame.len()) == 0 }
    }

    /// (result, frame bytes when JB_FRAME)
    pub fn pull(&mut self) -> (c_int, Vec<u8>) {
        let (mut p, mut n) = (std::ptr::null(), 0usize);
        let r = unsafe { jb_next(self.0, &mut p, &mut n) };
        let data = if r == JB_FRAME { unsafe { std::slice::from_raw_parts(p, n) }.to_vec() } else { Vec::new() };
        (r, data)
    }

    pub fn depth_ms(&self) -> u32 {
        unsafe { jb_depth_ms(self.0) }
    }

    /// played, lost, late, dropped, underruns, target_ms, buffering
    pub fn stats(&self) -> [u32; 7] {
        let mut s = [0u32; 7];
        unsafe { xc_jb_stats(self.0, s.as_mut_ptr()) };
        s
    }
}

impl Default for Jitter {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Jitter {
    fn drop(&mut self) {
        unsafe { xc_jb_free(self.0) };
    }
}
