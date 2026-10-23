//! The PSP's widget, image, and font code against the Rust host's encoders.

use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::Arc;

use companion_host::imaging::{Indexed, quantize_rgb};
use companion_host::macros::default_macros;
use companion_host::net::message::{Decoded, Message};
use companion_host::widgets::layout::Layout;
use companion_host::widgets::model::*;
use pspc_xcheck::*;

fn layout_text(data: &[u8]) -> (i32, String) {
    shim_text(|out, cap| unsafe { xc_layout(data.as_ptr(), data.len(), out, cap) })
}

fn widget_text(v: &WidgetValue) -> String {
    let data = v.encode();
    let (rc, text) = shim_text(|out, cap| unsafe { xc_widget(v.kind() as u8, data.as_ptr(), data.len(), out, cap) });
    assert_eq!(rc, 0, "{v:?}");
    text
}

#[test]
fn layout_parses_like_the_host_wrote_it() {
    let layout = Layout::default();
    let data = layout.encode(&default_macros());
    let (rc, text) = layout_text(&data);
    assert_eq!(rc, 0);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("pages|Home|Inbox|Claude"));
    for d in &layout.widgets {
        let want = format!(
            "w {} {} {} {} {} {} {} {} {}",
            d.id,
            d.kind as u8,
            d.page,
            d.x,
            d.y,
            d.w,
            d.h,
            u8::from(d.tap.is_some()),
            d.title
        );
        assert_eq!(lines.next(), Some(want.as_str()));
    }
    for m in default_macros() {
        assert_eq!(lines.next(), Some(format!("m {} {}", m.slot, m.label).as_str()));
    }
    assert_eq!(lines.next(), None);

    for cut in [1, data.len() / 2, data.len() - 1] {
        assert_ne!(layout_text(&data[..cut]).0, 0, "truncated at {cut}");
    }
    let mut extra = data.clone();
    extra.push(0);
    assert_ne!(layout_text(&extra).0, 0, "trailing byte");
}

#[test]
fn widget_values_parse_like_the_host_wrote_them() {
    let sys = WidgetValue::System(SystemStats {
        cpu_pct: Some(37),
        gpu_pct: None,
        ram_used_mb: 9123,
        ram_total_mb: 32768,
        cpu_temp_dc: Some(-15),
        gpu_temp_dc: None,
    });
    assert_eq!(widget_text(&sys), "37 255 9123 32768 -15 -32768");

    let cover = Arc::new(Asset { id: 0xfeed_beef_1234, format: 1, data: vec![] });
    let now = WidgetValue::NowPlaying(NowPlaying {
        state: PlayState::Paused,
        title: "Nối vòng tay lớn".into(),
        artist: "Trịnh Công Sơn".into(),
        cover: Some(cover),
    });
    assert_eq!(widget_text(&now), "2 feedbeef1234 Nối vòng tay lớn|Trịnh Công Sơn");

    let notes = WidgetValue::Notifications(vec![
        Notification { app: "CI".into(), title: "Build".into(), body: "Đã xong".into(), time: "09:15".into() },
        Notification { app: "".into(), title: "Hi".into(), body: "".into(), time: "09:16".into() },
    ]);
    assert_eq!(widget_text(&notes), "2\nCI|Build|Đã xong|09:15\n|Hi||09:16");

    let cal = WidgetValue::Calendar(Some(CalendarEvent { title: "Họp".into(), when: "in 5 min".into() }));
    assert_eq!(widget_text(&cal), "1 Họp|in 5 min");
    assert_eq!(widget_text(&WidgetValue::Calendar(None)), "0 |");

    let claude = WidgetValue::ClaudeCode(vec![ClaudeSession {
        project: "anyport".into(),
        state: ClaudeState::Waiting,
        detail: "needs approval".into(),
    }]);
    assert_eq!(widget_text(&claude), "1\nanyport|2|needs approval");

    let data = sys.encode();
    let (rc, _) = shim_text(|o, c| unsafe { xc_widget(WidgetKind::System as u8, data.as_ptr(), data.len() - 1, o, c) });
    assert_ne!(rc, 0);
    let (rc, _) = shim_text(|o, c| unsafe { xc_widget(99, data.as_ptr(), data.len(), o, c) });
    assert_ne!(rc, 0, "unknown kind");
}

#[test]
fn widget_envelopes_and_input_builders() {
    let msg = Message::WidgetData { widget_id: 513, format: 1, data: vec![9, 8, 7] }.encode();
    let (mut id, mut fmt, mut data, mut n) = (0u16, 0u8, std::ptr::null(), 0usize);
    assert_eq!(unsafe { pspc_parse_widget_data(msg.as_ptr(), msg.len(), &mut id, &mut fmt, &mut data, &mut n) }, 0);
    assert_eq!((id, fmt, unsafe { std::slice::from_raw_parts(data, n) }), (513, 1, &[9u8, 8, 7][..]));

    let msg = Message::WidgetAsset { asset_id: u64::MAX - 1, format: 1, data: vec![1] }.encode();
    let mut aid = 0u64;
    assert_eq!(unsafe { pspc_parse_widget_asset(msg.as_ptr(), msg.len(), &mut aid, &mut fmt, &mut data, &mut n) }, 0);
    assert_eq!((aid, n), (u64::MAX - 1, 1));

    let mut out = [0u8; 16];
    let n = unsafe { pspc_build_input_event(out.as_mut_ptr(), 0, 2, 6, 479, 271, 0x8000) };
    assert_eq!(
        Message::decode(&out[..n]).unwrap(),
        Decoded::Known(Message::InputEvent { kind: 0, page: 2, widget_id: 6, x: 479, y: 271, buttons: 0x8000 })
    );
    let n = unsafe { pspc_build_macro_trigger(out.as_mut_ptr(), 11) };
    assert_eq!(Message::decode(&out[..n]).unwrap(), Decoded::Known(Message::MacroTrigger { slot: 11 }));
}

fn c_image(payload: &[u8]) -> Option<(i32, i32, Vec<u32>)> {
    let mut scratch = vec![0u8; IMAGE_MAX_RAW];
    let mut px = vec![0u32; 128 * 128];
    let (mut w, mut h) = (0, 0);
    let rc = unsafe {
        image_decode(payload.as_ptr(), payload.len(), scratch.as_mut_ptr(), px.as_mut_ptr(), px.len(), &mut w, &mut h)
    };
    (rc == 0).then(|| {
        px.truncate((w * h) as usize);
        (w, h, px)
    })
}

#[test]
fn images_decode_to_the_same_pixels() {
    for (w, h) in [(76u32, 86u32), (64, 64), (1, 1), (128, 3)] {
        let rgb: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i * 13 % 251) as u8, (i / w * 40 % 256) as u8, (i % w * 3 % 256) as u8])
            .collect();
        let img = quantize_rgb(&rgb, w, h).unwrap();
        let enc = img.encode();
        let (cw, ch, px) = c_image(&enc).expect("C decodes the host image");
        assert_eq!((cw as u32, ch as u32), (w, h));
        let want: Vec<u32> = img
            .to_rgb()
            .chunks_exact(3)
            .map(|c| 0xFF00_0000 | u32::from(c[2]) << 16 | u32::from(c[1]) << 8 | u32::from(c[0]))
            .collect();
        assert_eq!(px, want, "{w}x{h}");
        assert_eq!(Indexed::decode(&enc).unwrap(), img);
    }
}

#[test]
fn malformed_images_are_rejected() {
    let enc = quantize_rgb(&[5; 3 * 16], 4, 4).unwrap().encode();
    assert!(c_image(&enc).is_some());
    for cut in 0..enc.len() {
        assert!(c_image(&enc[..cut]).is_none(), "truncated at {cut}");
    }
    let mut bad = enc.clone();
    bad[0] = bad[0].wrapping_add(1);
    assert!(c_image(&bad).is_none(), "raw length mismatch");
    // An index past the palette.
    let mut raw = vec![1, 0, 1, 0, 1, 0, 9, 9, 9, 3];
    let mut payload = (raw.len() as u32).to_le_bytes().to_vec();
    payload.extend(lz4_flex::block::compress(&raw));
    assert!(c_image(&payload).is_none());
    raw[9] = 0;
    let mut payload = (raw.len() as u32).to_le_bytes().to_vec();
    payload.extend(lz4_flex::block::compress(&raw));
    assert_eq!(c_image(&payload).unwrap().2, vec![0xFF09_0909]);
}

#[test]
fn lz4_matches_reference_on_repetitive_and_random_data() {
    let mut seed = 0x1234_5678u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as u8
    };
    let inputs: Vec<Vec<u8>> = vec![
        vec![],
        vec![7; 5000],
        (0..3000).map(|_| rnd()).collect(),
        (0..20000u32).map(|i| (i % 37) as u8 ^ if i % 1000 == 0 { 0xff } else { 0 }).collect(),
    ];
    for input in inputs {
        let block = lz4_flex::block::compress(&input);
        let mut out = vec![0u8; input.len()];
        assert_eq!(unsafe { lz4_block_decode(block.as_ptr(), block.len(), out.as_mut_ptr(), out.len()) }, 0);
        assert_eq!(out, input);
        if !input.is_empty() {
            let mut short = vec![0u8; input.len() - 1];
            assert_ne!(unsafe { lz4_block_decode(block.as_ptr(), block.len(), short.as_mut_ptr(), short.len()) }, 0);
        }
    }
}

fn wrap(s: &str, width: i32) -> Vec<String> {
    let c = CString::new(s).unwrap();
    let (mut starts, mut lens) = ([0usize; 16], [0usize; 16]);
    let n = unsafe { font_wrap(c.as_ptr(), width, starts.as_mut_ptr(), lens.as_mut_ptr(), 16) };
    (0..n as usize).map(|i| s[starts[i]..starts[i] + lens[i]].to_string()).collect()
}

#[test]
fn font_measures_and_wraps_vietnamese() {
    let w = |s: &str| unsafe { font_text_width(CString::new(s).unwrap().as_ptr()) };
    assert_eq!(w("abc"), 24);
    assert_eq!(w("Việt Nam"), 64, "precomposed letters are 8 px wide like ASCII");
    assert_eq!(w("\u{1F600}"), w("\u{FFFD}"), "missing glyphs use the replacement glyph");

    assert_eq!(wrap("Xin chào các bạn", 80), ["Xin chào ", "các bạn"]);
    assert_eq!(wrap("abcdefghijkl", 40), ["abcde", "fghij", "kl"], "long words break mid-word");
    assert_eq!(wrap("one\ntwo", 400), ["one", "two"]);
    assert_eq!(wrap("x", 1), ["x"], "always makes progress");
}

#[test]
fn utf8_decoder_rejects_malformed_sequences() {
    let decode = |bytes: &[u8]| {
        let mut buf = bytes.to_vec();
        buf.push(0);
        let mut p: *const c_char = buf.as_ptr().cast();
        let mut out = Vec::new();
        loop {
            let cp = unsafe { utf8_next(&mut p) };
            if cp == 0 {
                break;
            }
            out.push(cp);
        }
        out
    };
    assert_eq!(decode("ạ€😀".as_bytes()), [0x1EA1, 0x20AC, 0x1F600]);
    assert_eq!(decode(&[0xC0, 0x80]), [0xFFFD], "overlong");
    assert_eq!(decode(&[0xED, 0xA0, 0x80]), [0xFFFD], "surrogate");
    assert_eq!(decode(&[0xE1, 0xBA, b'a']), [0xFFFD, u32::from(b'a')], "truncated then resync");
    assert_eq!(decode(&[0xFF, b'b']), [0xFFFD, u32::from(b'b')]);
    assert_eq!(decode(&[0xE1, 0xBA]), [0xFFFD], "stops at the terminator");
}
