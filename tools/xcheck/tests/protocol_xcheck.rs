//! The PSP's C encoders must produce what the Rust host decodes, and vice versa.

use std::ffi::CString;

use companion_host::net::discovery::{self, Announce as RAnnounce};
use companion_host::net::handshake::{Handshake, ParsedHandshake, Status};
use companion_host::net::message::{ByeReason, Decoded, Message};
use pspc_xcheck::*;

fn known(b: &[u8]) -> Message {
    match Message::decode(b).unwrap() {
        Decoded::Known(m) => m,
        Decoded::Unknown(t) => panic!("unknown {t}"),
    }
}

fn hs(b: &[u8]) -> Handshake {
    match Handshake::decode(b).unwrap() {
        ParsedHandshake::Message(m) => m,
        other => panic!("{other:?}"),
    }
}

#[test]
fn discovery_both_ways() {
    let mut out = [0u8; 14];
    let n = unsafe { pspc_build_discover(out.as_mut_ptr(), [7u8; 8].as_ptr()) };
    assert_eq!(n, 14);
    assert_eq!(discovery::decode_request(&out).unwrap(), [7; 8]);

    let a = RAnnounce {
        nonce: [1; 8],
        host_id: [2; 16],
        control_port: 47861,
        audio_port: 47862,
        flags: 1,
        host_name: "Máy tính".into(),
    };
    let bytes = a.encode();
    let mut c = Announce::default();
    assert_eq!(unsafe { pspc_parse_announce(bytes.as_ptr(), bytes.len(), &mut c) }, 0);
    assert_eq!((c.nonce, c.host_id, c.control_port, c.audio_port, c.flags), ([1; 8], [2; 16], 47861, 47862, 1));
    assert_eq!(c_str(&c.host_name), "Máy tính");
    assert_eq!(unsafe { pspc_parse_announce(bytes.as_ptr(), bytes.len() - 1, &mut c) }, -1);
    assert_eq!(unsafe { pspc_parse_announce(out.as_ptr(), out.len(), &mut c) }, -1, "a request is not an announce");
}

#[test]
fn handshake_both_ways() {
    let mut out = [0u8; 320];
    let n = unsafe { pspc_build_session_hello(out.as_mut_ptr(), [3u8; 16].as_ptr(), [4u8; 16].as_ptr()) };
    assert_eq!(hs(&out[..n]), Handshake::SessionHello { device_id: [3; 16], client_nonce: [4; 16] });

    let long_name = CString::new("PSP-3000 ".repeat(5) + "ộ").unwrap();
    let n = unsafe {
        pspc_build_pair_request(out.as_mut_ptr(), [5u8; 16].as_ptr(), [6u8; 32].as_ptr(), long_name.as_ptr())
    };
    match hs(&out[..n]) {
        Handshake::PairRequest { device_id, pa, device_name } => {
            assert_eq!((device_id, pa), ([5; 16], [6; 32]));
            assert_eq!(device_name.len(), 32, "truncated to 32 bytes on a UTF-8 boundary");
        }
        other => panic!("{other:?}"),
    }

    let n = unsafe { pspc_build_pair_confirm(out.as_mut_ptr(), [8u8; 32].as_ptr()) };
    assert_eq!(hs(&out[..n]), Handshake::PairConfirm { ca: [8; 32] });

    let reply = Handshake::SessionReply { status: Status::Unknown, host_id: [9; 16], server_nonce: [10; 16] }.encode();
    let (mut st, mut id, mut nonce) = (0u8, [0u8; 16], [0u8; 16]);
    assert_eq!(
        unsafe { pspc_parse_session_reply(reply.as_ptr(), reply.len(), &mut st, id.as_mut_ptr(), nonce.as_mut_ptr()) },
        0
    );
    assert_eq!((st, id, nonce), (1, [9; 16], [10; 16]));
    let mut v2 = reply.clone();
    v2[4] = 2;
    assert_eq!(
        unsafe { pspc_parse_session_reply(v2.as_ptr(), v2.len(), &mut st, id.as_mut_ptr(), nonce.as_mut_ptr()) },
        -2
    );

    let ch = Handshake::PairChallenge { status: Status::Ok, host_id: [11; 16], pb: [12; 32], cb: [13; 32] }.encode();
    let (mut pb, mut cb) = ([0u8; 32], [0u8; 32]);
    assert_eq!(
        unsafe {
            pspc_parse_pair_challenge(ch.as_ptr(), ch.len(), &mut st, id.as_mut_ptr(), pb.as_mut_ptr(), cb.as_mut_ptr())
        },
        0
    );
    assert_eq!((st, id, pb, cb), (0, [11; 16], [12; 32], [13; 32]));

    let res = Handshake::PairResult { status: Status::WrongPin }.encode();
    assert_eq!(unsafe { pspc_parse_pair_result(res.as_ptr(), res.len(), &mut st) }, 0);
    assert_eq!(st, 4);
    assert_eq!(unsafe { pspc_parse_pair_result(ch.as_ptr(), ch.len(), &mut st) }, -1, "wrong kind");
}

#[test]
fn messages_both_ways() {
    let mut out = [0u8; 600];
    let (sw, name) = (CString::new("0.1.0").unwrap(), CString::new("Henry's PSP").unwrap());
    let n = unsafe { pspc_build_hello(out.as_mut_ptr(), 5, sw.as_ptr(), name.as_ptr()) };
    assert_eq!(
        known(&out[..n]),
        Message::Hello { caps: 5, sw_version: "0.1.0".into(), device_name: "Henry's PSP".into() }
    );
    let n = unsafe { pspc_build_ping(out.as_mut_ptr(), 77, 1 << 40) };
    assert_eq!(known(&out[..n]), Message::Ping { id: 77, t_us: 1 << 40 });
    let n = unsafe { pspc_build_bye(out.as_mut_ptr(), 0) };
    assert_eq!(known(&out[..n]), Message::Bye { reason: ByeReason::Normal });
    let n = unsafe { pspc_build_audio_register(out.as_mut_ptr()) };
    assert_eq!(known(&out[..n]), Message::AudioRegister);

    let ack = Message::HelloAck {
        caps: 1,
        host_name: "DESKTOP".into(),
        session_tag: 0x1234_5678,
        audio_port: 47862,
        keepalive_s: 15,
        budget_kbps: 300,
    }
    .encode();
    let mut c = HelloAck::default();
    assert_eq!(unsafe { pspc_parse_hello_ack(ack.as_ptr(), ack.len(), &mut c) }, 0);
    assert_eq!((c.caps, c.session_tag, c.audio_port, c.keepalive_s, c.budget_kbps), (1, 0x1234_5678, 47862, 15, 300));
    assert_eq!(c_str(&c.host_name), "DESKTOP");

    let pong = Message::Pong { id: 3, t_us: 99 }.encode();
    let (mut id, mut t) = (0u32, 0u64);
    assert_eq!(unsafe { pspc_parse_pong(pong.as_ptr(), pong.len(), &mut id, &mut t) }, 0);
    assert_eq!((id, t), (3, 99));
    assert_eq!(unsafe { pspc_parse_pong(ack.as_ptr(), ack.len(), &mut id, &mut t) }, -1);

    let bye = Message::Bye { reason: ByeReason::Replaced }.encode();
    let mut reason = 0u8;
    assert_eq!(unsafe { pspc_parse_bye(bye.as_ptr(), bye.len(), &mut reason) }, 0);
    assert_eq!(reason, 2);

    let err = Message::Error { code: 1, text: "message 0x04 not allowed".into() }.encode();
    let (mut code, mut text) = (0u16, [0u8; 10]);
    assert_eq!(
        unsafe { pspc_parse_error(err.as_ptr(), err.len(), &mut code, text.as_mut_ptr().cast(), text.len()) },
        0
    );
    assert_eq!((code, c_str(&text)), (1, "message 0".to_string()), "truncated to the buffer");
}
