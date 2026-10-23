//! The PSP's ADPCM encoder and voice messages against the Rust host.

use companion_host::net::message::{Decoded, Message, msg_type};
use companion_host::voice::adpcm;
use pspc_xcheck::*;

fn speech_like(n: usize) -> Vec<i16> {
    (0..n)
        .map(|i| {
            let t = i as f32 / 11_025.0;
            ((t * 180.0 * std::f32::consts::TAU).sin() * 9000.0 * (t * 3.0).sin().abs()
                + (t * 1300.0 * std::f32::consts::TAU).sin() * 2500.0) as i16
        })
        .collect()
}

/// Encodes like the PSP: 1024-sample chunks, state carried from chunk to chunk.
fn c_chunks(pcm: &[i16]) -> Vec<Vec<u8>> {
    let mut st = AdpcmState::default();
    unsafe { adpcm_init(&mut st) };
    pcm.chunks(1024)
        .map(|c| {
            let mut out = vec![0u8; 4 + c.len().div_ceil(2)];
            let n = unsafe { adpcm_encode_chunk(&mut st, c.as_ptr(), c.len(), out.as_mut_ptr()) };
            assert_eq!(n, out.len());
            out
        })
        .collect()
}

#[test]
fn c_encoder_matches_the_reference_byte_for_byte_and_decodes_on_the_host() {
    let pcm = speech_like(11_025 * 2 + 77);
    let chunks = c_chunks(&pcm);
    let mut decoded = Vec::new();
    for (chunk, src) in chunks.iter().zip(pcm.chunks(1024)) {
        let predictor = i16::from_le_bytes([chunk[0], chunk[1]]);
        assert_eq!(*chunk, adpcm::encode_chunk(src, predictor, chunk[2]), "same algorithm as the reference");
        decoded.extend(adpcm::decode_chunk(chunk).unwrap());
    }
    decoded.truncate(pcm.len());
    let err: f64 = pcm.iter().zip(&decoded).map(|(a, b)| f64::from(a.abs_diff(*b))).sum::<f64>() / pcm.len() as f64;
    assert!(err < 500.0, "mean error {err}");
    // Each chunk decodes on its own: decoding chunk 5 alone equals its part of the stream.
    assert_eq!(adpcm::decode_chunk(&chunks[5]).unwrap(), decoded[5 * 1024..6 * 1024]);
}

#[test]
fn extreme_input_stays_in_range() {
    let pcm: Vec<i16> = (0..4096).map(|i| if i % 2 == 0 { i16::MAX } else { i16::MIN }).collect();
    for chunk in c_chunks(&pcm) {
        assert!(chunk[2] <= 88);
        adpcm::decode_chunk(&chunk).unwrap();
    }
}

#[test]
fn voice_messages_both_ways() {
    let mut out = [0u8; 32];
    let n = unsafe { pspc_build_voice_start(out.as_mut_ptr(), 7, 11_025) };
    assert_eq!(
        Message::decode(&out[..n]).unwrap(),
        Decoded::Known(Message::VoiceStart { utterance_id: 7, codec: 1, sample_rate: 11_025, channels: 1 })
    );
    let mut buf = vec![0u8; 9];
    let n = unsafe { pspc_build_voice_chunk_header(buf.as_mut_ptr(), 7, 3) };
    buf.truncate(n);
    buf.extend_from_slice(&[1, 2, 3, 4, 5]);
    assert_eq!(
        Message::decode(&buf).unwrap(),
        Decoded::Known(Message::VoiceChunk { utterance_id: 7, seq: 3, data: vec![1, 2, 3, 4, 5] })
    );
    let n = unsafe { pspc_build_voice_end(out.as_mut_ptr(), 7, 2) };
    assert_eq!(Message::decode(&out[..n]).unwrap(), Decoded::Known(Message::VoiceEnd { utterance_id: 7, reason: 2 }));

    for (msg, ty) in [
        (
            Message::VoiceTranscript { utterance_id: 9, is_final: true, text: "Xin chào".into() },
            msg_type::VOICE_TRANSCRIPT,
        ),
        (
            Message::VoiceReplyText { utterance_id: 9, is_final: false, text: "Chào bạn! ".into() },
            msg_type::VOICE_REPLY_TEXT,
        ),
    ] {
        let bytes = msg.encode();
        let (mut t, mut id, mut fin) = (0u8, 0u32, 0u8);
        let (rc, text) = shim_text(|o, c| unsafe {
            pspc_parse_voice_text(bytes.as_ptr(), bytes.len(), &mut t, &mut id, &mut fin, o, c)
        });
        assert_eq!(rc, 0);
        assert_eq!((t, id), (ty, 9));
        match msg {
            Message::VoiceTranscript { text: want, .. } => assert_eq!((text, fin), (want, 1)),
            Message::VoiceReplyText { text: want, .. } => assert_eq!((text, fin), (want, 0)),
            _ => unreachable!(),
        }
    }
    let ping = Message::Ping { id: 1, t_us: 2 }.encode();
    let (mut t, mut id, mut fin) = (0u8, 0u32, 0u8);
    let (rc, _) =
        shim_text(|o, c| unsafe { pspc_parse_voice_text(ping.as_ptr(), ping.len(), &mut t, &mut id, &mut fin, o, c) });
    assert_ne!(rc, 0);
}
