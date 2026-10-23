//! The PSP's audio message codecs and jitter buffer, driven with real MP3 frames from the host encoder.

use companion_host::audio::encode_mp3::Mp3Encoder;
use companion_host::net::message::{Decoded, Message};
use pspc_xcheck::*;

fn frames(n: usize) -> Vec<Vec<u8>> {
    let mut enc = Mp3Encoder::new(2, 96).unwrap();
    let pcm: Vec<i16> = (0..n * 1152 * 2 + 4096).map(|i| ((i as f32 * 0.03).sin() * 8000.0) as i16).collect();
    let mut out = enc.encode(&pcm).unwrap();
    out.extend(enc.flush().unwrap());
    out.truncate(n);
    assert_eq!(out.len(), n);
    out
}

/// A deterministic pseudo-random source for arrival jitter.
struct Rng(u32);

impl Rng {
    fn next(&mut self, max: u32) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 % max
    }
}

/// Arrival time (ms) and packet (seq, pts, frame index).
type Arrivals = Vec<(u32, u32, u32, usize)>;

/// Runs the player every 26.122 ms for `ms` milliseconds; returns the results in order.
fn simulate(jb: &mut Jitter, frames: &[Vec<u8>], mut arrivals: Arrivals, ms: u32) -> Vec<(i32, Vec<u8>)> {
    arrivals.sort_by_key(|a| a.0);
    let mut out = Vec::new();
    let mut next_play_us = 0u64;
    let mut ai = 0;
    for t in 0..ms {
        while ai < arrivals.len() && arrivals[ai].0 <= t {
            let (_, seq, pts, idx) = arrivals[ai];
            jb.put(seq, pts, &frames[idx]);
            ai += 1;
        }
        if u64::from(t) * 1000 >= next_play_us {
            out.push(jb.pull());
            next_play_us += 26_122;
        }
    }
    out
}

fn played_order(out: &[(i32, Vec<u8>)], frames: &[Vec<u8>]) -> Vec<usize> {
    out.iter().filter(|r| r.0 == JB_FRAME).map(|r| frames.iter().position(|f| *f == r.1).unwrap()).collect()
}

#[test]
fn jittery_arrivals_play_in_order_without_loss() {
    let f = frames(200);
    let mut rng = Rng(7);
    let arrivals: Arrivals = (0..200u32).map(|i| (i * 26 + rng.next(120), i, i * 26, i as usize)).collect();
    let mut jb = Jitter::new();
    let out = simulate(&mut jb, &f, arrivals, 200 * 26 + 600);
    assert_eq!(played_order(&out, &f), (0..200).collect::<Vec<_>>());
    let first = out.iter().position(|r| r.0 == JB_FRAME).unwrap();
    assert!((6..=12).contains(&first), "starts after buffering about 180 ms: {first} ticks");
    let [played, lost, late, dropped, underruns, target, _] = jb.stats();
    assert_eq!((played, lost, late, dropped), (200, 0, 0, 0));
    assert!(underruns <= 1, "only the end of the stream: {underruns}");
    assert!((150..=250).contains(&target));
}

#[test]
fn lost_packets_are_concealed_once_each() {
    let f = frames(150);
    let arrivals: Arrivals = (0..150u32).filter(|i| i % 20 != 7).map(|i| (i * 26, i, i * 26, i as usize)).collect();
    let mut jb = Jitter::new();
    let out = simulate(&mut jb, &f, arrivals, 150 * 26 + 600);
    let [played, lost, ..] = jb.stats();
    assert_eq!(lost, 8, "one LOST per missing packet (7, 27, ... 147)");
    assert_eq!(played + lost, 150);
    let lost_at: Vec<usize> = out.iter().enumerate().filter(|(_, r)| r.0 == JB_LOST).map(|(i, _)| i).collect();
    assert_eq!(lost_at.len(), 8);
    let order = played_order(&out, &f);
    assert!(order.windows(2).all(|w| w[1] > w[0]));
}

#[test]
fn host_side_silence_does_not_grow_the_buffer_but_a_stall_does() {
    let f = frames(120);
    // 40 frames, then the host skips 2 s of silence (pts jumps, seq continues), then 40 more frames.
    let mut arrivals: Arrivals = (0..40u32).map(|i| (i * 26, i, i * 26, i as usize)).collect();
    arrivals.extend((40..80u32).map(|i| (i * 26 + 2000, i, i * 26 + 2000, i as usize)));
    let mut jb = Jitter::new();
    simulate(&mut jb, &f, arrivals, 80 * 26 + 3000);
    let s = jb.stats();
    assert_eq!((s[0], s[1]), (80, 0));
    assert_eq!(s[5], 180, "silence is not a network problem; target stays");

    // Same frames, but the network stalls for 500 ms with no pts jump.
    let mut arrivals: Arrivals = (0..40u32).map(|i| (i * 26, i, i * 26, i as usize)).collect();
    arrivals.extend((40..80u32).map(|i| ((i * 26).max(40 * 26 + 500), i, i * 26, i as usize)));
    let mut jb = Jitter::new();
    simulate(&mut jb, &f, arrivals, 80 * 26 + 3000);
    let s = jb.stats();
    assert!(s[4] >= 1, "a stall counts as an underrun");
    assert!(s[5] > 180, "the target grows after a stall: {}", s[5]);
    assert!(s[5] <= 250);
}

#[test]
fn rejects_duplicates_late_and_invalid_packets() {
    let f = frames(20);
    let mut jb = Jitter::new();
    for i in 0..10u32 {
        assert!(jb.put(i, i * 26, &f[i as usize]));
    }
    assert!(!jb.put(3, 78, &f[3]), "duplicate");
    assert_eq!(jb.depth_ms(), 261);
    while jb.pull().0 == JB_FRAME && jb.stats()[0] < 5 {}
    assert!(!jb.put(1, 26, &f[1]), "late");
    assert!(!jb.put(200, 0, &f[0]), "too far ahead");
    assert!(!jb.put(11, 0, &f[0][..100]), "not a whole frame");
    assert!(!jb.put(11, 0, &[0u8; 400]), "not MP3");
    let s = jb.stats();
    assert_eq!((s[2], s[3]), (1, 3));
    assert_eq!(unsafe { mp3_frame_len(f[0].as_ptr(), f[0].len()) }, f[0].len());
}

#[test]
fn audio_messages_both_ways() {
    let start = Message::AudioStreamStart {
        stream_id: 9,
        kind: 1,
        codec: 1,
        sample_rate: 44_100,
        channels: 2,
        bitrate_kbps: 96,
    }
    .encode();
    let mut s = StreamStart::default();
    assert_eq!(unsafe { pspc_parse_stream_start(start.as_ptr(), start.len(), &mut s) }, 0);
    assert_eq!((s.stream_id, s.kind, s.codec, s.sample_rate, s.channels, s.bitrate_kbps), (9, 1, 1, 44_100, 2, 96));
    assert_ne!(unsafe { pspc_parse_stream_start(start.as_ptr(), start.len() - 1, &mut s) }, 0);

    let stop = Message::AudioStreamStop { stream_id: 9, reason: 2 }.encode();
    let (mut id, mut reason) = (0u8, 0u8);
    assert_eq!(unsafe { pspc_parse_stream_stop(stop.as_ptr(), stop.len(), &mut id, &mut reason) }, 0);
    assert_eq!((id, reason), (9, 2));

    let pkt = Message::AudioPacket { stream_id: 3, seq: 70_000, pts_ms: 1_234_567, data: vec![0xFF, 0xFB, 1] }.encode();
    let (mut seq, mut pts, mut data, mut n) = (0u32, 0u32, std::ptr::null(), 0usize);
    assert_eq!(
        unsafe { pspc_parse_audio_packet(pkt.as_ptr(), pkt.len(), &mut id, &mut seq, &mut pts, &mut data, &mut n) },
        0
    );
    assert_eq!((id, seq, pts, n), (3, 70_000, 1_234_567, 3));

    let mut out = [0u8; 8];
    let n = unsafe { pspc_build_stream_request(out.as_mut_ptr(), 1, 1) };
    assert_eq!(
        Message::decode(&out[..n]).unwrap(),
        Decoded::Known(Message::AudioStreamRequest { kind: 1, enable: true })
    );
    let n = unsafe { pspc_build_stream_stop(out.as_mut_ptr(), 4, 1) };
    assert_eq!(
        Message::decode(&out[..n]).unwrap(),
        Decoded::Known(Message::AudioStreamStop { stream_id: 4, reason: 1 })
    );
}
