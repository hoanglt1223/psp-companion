//! The PSP's Stream Deck messages and key images against the Rust host's bridge.

use companion_host::net::message::{Decoded, Message};
use companion_host::streamdeck::bridge::{Deck, KEY_PAGE_NEXT, KEY_PAGE_PREV};
use pspc_xcheck::*;
use tokio::time::Instant;

#[tokio::test(start_paused = true)]
async fn key_images_from_the_bridge_decode_on_the_psp() {
    let mut deck = Deck::new(76);
    let rgb: Vec<u8> = (0..76 * 76).flat_map(|i| [(i % 76 * 3) as u8, (i / 76 * 3) as u8, 0x40]).collect();
    deck.set(7, Some(&rgb), Instant::now());
    let msgs = deck.tick(Instant::now());
    let bytes = msgs[0].encode();

    let (mut key, mut id, mut format, mut w, mut h, mut data, mut n) =
        (0u8, 0u64, 0u8, 0u8, 0u8, std::ptr::null(), 0usize);
    let rc = unsafe {
        pspc_parse_deck_key_image(
            bytes.as_ptr(),
            bytes.len(),
            &mut key,
            &mut id,
            &mut format,
            &mut w,
            &mut h,
            &mut data,
            &mut n,
        )
    };
    assert_eq!(rc, 0);
    assert_eq!((key, format, w, h), (7, 1, 76, 76));
    let Message::DeckKeyImage { image_id, .. } = &msgs[0] else { panic!() };
    assert_eq!(id, *image_id);

    let payload = unsafe { std::slice::from_raw_parts(data, n) };
    let mut scratch = vec![0u8; IMAGE_MAX_RAW];
    let mut px = vec![0u32; 128 * 128];
    let (mut iw, mut ih) = (0, 0);
    let rc = unsafe {
        image_decode(payload.as_ptr(), payload.len(), scratch.as_mut_ptr(), px.as_mut_ptr(), px.len(), &mut iw, &mut ih)
    };
    assert_eq!((rc, iw, ih), (0, 76, 76));
    // The PSP shows what the host quantised: compare against the host's own decode.
    let host = companion_host::imaging::Indexed::decode(payload).unwrap().to_rgb();
    for (i, p) in px[..76 * 76].iter().enumerate() {
        let c = &host[i * 3..i * 3 + 3];
        assert_eq!(*p, 0xFF00_0000 | u32::from(c[2]) << 16 | u32::from(c[1]) << 8 | u32::from(c[0]));
    }
}

#[test]
fn deck_messages_both_ways() {
    let layout = Message::DeckLayout { columns: 6, rows: 3, key_w: 76, key_h: 76 }.encode();
    let (mut c, mut r, mut w, mut h) = (0u8, 0u8, 0u8, 0u8);
    assert_eq!(unsafe { pspc_parse_deck_layout(layout.as_ptr(), layout.len(), &mut c, &mut r, &mut w, &mut h) }, 0);
    assert_eq!((c, r, w, h), (6, 3, 76, 76));
    assert_ne!(unsafe { pspc_parse_deck_layout(layout.as_ptr(), 4, &mut c, &mut r, &mut w, &mut h) }, 0);

    let cached =
        Message::DeckKeyImage { key: 2, image_id: 99, format: 1, width: 76, height: 76, data: vec![] }.encode();
    let (mut key, mut id, mut f, mut data, mut n) = (0u8, 0u64, 0u8, std::ptr::null(), 1usize);
    let rc = unsafe {
        pspc_parse_deck_key_image(
            cached.as_ptr(),
            cached.len(),
            &mut key,
            &mut id,
            &mut f,
            &mut w,
            &mut h,
            &mut data,
            &mut n,
        )
    };
    assert_eq!((rc, key, id, n), (0, 2, 99, 0), "empty data: reuse the cached image");

    let mut out = [0u8; 4];
    for (k, pressed) in [(4u8, true), (4, false), (KEY_PAGE_PREV, true), (KEY_PAGE_NEXT, true)] {
        let n = unsafe { pspc_build_deck_key_press(out.as_mut_ptr(), k, u8::from(pressed)) };
        assert_eq!(Message::decode(&out[..n]).unwrap(), Decoded::Known(Message::DeckKeyPress { key: k, pressed }));
    }
}
