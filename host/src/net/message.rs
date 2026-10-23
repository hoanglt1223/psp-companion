//! Encrypted-channel messages (`type: u8 || payload`). See protocol/PROTOCOL.md.

use super::codec::{DecodeError, Reader, Writer};

pub const MAX_MESSAGE: usize = 16384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Hello { caps: u32, sw_version: String, device_name: String },
    HelloAck { caps: u32, host_name: String, session_tag: u32, audio_port: u16, keepalive_s: u16, budget_kbps: u16 },
    Ping { id: u32, t_us: u64 },
    Pong { id: u32, t_us: u64 },
    Bye { reason: ByeReason },
    Error { code: u16, text: String },
    WidgetLayout { format: u8, data: Vec<u8> },
    WidgetData { widget_id: u16, format: u8, data: Vec<u8> },
    WidgetAsset { asset_id: u64, format: u8, data: Vec<u8> },
    InputEvent { kind: u8, page: u8, widget_id: u16, x: u16, y: u16, buttons: u32 },
    MacroTrigger { slot: u8 },
    VoiceStart { utterance_id: u32, codec: u8, sample_rate: u32, channels: u8 },
    VoiceChunk { utterance_id: u32, seq: u32, data: Vec<u8> },
    VoiceEnd { utterance_id: u32, reason: u8 },
    VoiceTranscript { utterance_id: u32, is_final: bool, text: String },
    VoiceReplyText { utterance_id: u32, is_final: bool, text: String },
    AudioStreamStart { stream_id: u8, kind: u8, codec: u8, sample_rate: u32, channels: u8, bitrate_kbps: u16 },
    AudioStreamStop { stream_id: u8, reason: u8 },
    AudioStreamRequest { kind: u8, enable: bool },
    AudioPacket { stream_id: u8, seq: u32, pts_ms: u32, data: Vec<u8> },
    AudioRegister,
    AudioRegisterAck,
    DeckLayout { columns: u8, rows: u8, key_w: u8, key_h: u8 },
    DeckKeyImage { key: u8, image_id: u64, format: u8, width: u8, height: u8, data: Vec<u8> },
    DeckKeyPress { key: u8, pressed: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ByeReason {
    Normal = 0,
    ShuttingDown = 1,
    Replaced = 2,
    Unpaired = 3,
}

impl ByeReason {
    fn from_u8(v: u8) -> Result<Self, DecodeError> {
        Ok(match v {
            0 => Self::Normal,
            1 => Self::ShuttingDown,
            2 => Self::Replaced,
            3 => Self::Unpaired,
            _ => return Err(DecodeError::OutOfRange("bye reason")),
        })
    }
}

pub mod error_code {
    pub const PROTOCOL: u16 = 1;
    pub const UNSUPPORTED: u16 = 2;
    pub const RATE_LIMITED: u16 = 3;
    pub const INTERNAL: u16 = 4;
}

pub mod msg_type {
    pub const HELLO: u8 = 0x01;
    pub const HELLO_ACK: u8 = 0x02;
    pub const PING: u8 = 0x03;
    pub const PONG: u8 = 0x04;
    pub const BYE: u8 = 0x05;
    pub const ERROR: u8 = 0x06;
    pub const WIDGET_LAYOUT: u8 = 0x10;
    pub const WIDGET_DATA: u8 = 0x11;
    pub const WIDGET_ASSET: u8 = 0x12;
    pub const INPUT_EVENT: u8 = 0x20;
    pub const MACRO_TRIGGER: u8 = 0x21;
    pub const VOICE_START: u8 = 0x30;
    pub const VOICE_CHUNK: u8 = 0x31;
    pub const VOICE_END: u8 = 0x32;
    pub const VOICE_TRANSCRIPT: u8 = 0x33;
    pub const VOICE_REPLY_TEXT: u8 = 0x34;
    pub const AUDIO_STREAM_START: u8 = 0x40;
    pub const AUDIO_STREAM_STOP: u8 = 0x41;
    pub const AUDIO_STREAM_REQUEST: u8 = 0x42;
    pub const AUDIO_PACKET: u8 = 0x43;
    pub const AUDIO_REGISTER: u8 = 0x44;
    pub const AUDIO_REGISTER_ACK: u8 = 0x45;
    pub const DECK_LAYOUT: u8 = 0x50;
    pub const DECK_KEY_IMAGE: u8 = 0x51;
    pub const DECK_KEY_PRESS: u8 = 0x52;
}

/// Result of decoding: unknown types are surfaced rather than treated as errors,
/// so newer peers can add messages within protocol version 1.
#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    Known(Message),
    Unknown(u8),
}

impl Message {
    pub fn type_id(&self) -> u8 {
        use msg_type::*;
        match self {
            Self::Hello { .. } => HELLO,
            Self::HelloAck { .. } => HELLO_ACK,
            Self::Ping { .. } => PING,
            Self::Pong { .. } => PONG,
            Self::Bye { .. } => BYE,
            Self::Error { .. } => ERROR,
            Self::WidgetLayout { .. } => WIDGET_LAYOUT,
            Self::WidgetData { .. } => WIDGET_DATA,
            Self::WidgetAsset { .. } => WIDGET_ASSET,
            Self::InputEvent { .. } => INPUT_EVENT,
            Self::MacroTrigger { .. } => MACRO_TRIGGER,
            Self::VoiceStart { .. } => VOICE_START,
            Self::VoiceChunk { .. } => VOICE_CHUNK,
            Self::VoiceEnd { .. } => VOICE_END,
            Self::VoiceTranscript { .. } => VOICE_TRANSCRIPT,
            Self::VoiceReplyText { .. } => VOICE_REPLY_TEXT,
            Self::AudioStreamStart { .. } => AUDIO_STREAM_START,
            Self::AudioStreamStop { .. } => AUDIO_STREAM_STOP,
            Self::AudioStreamRequest { .. } => AUDIO_STREAM_REQUEST,
            Self::AudioPacket { .. } => AUDIO_PACKET,
            Self::AudioRegister => AUDIO_REGISTER,
            Self::AudioRegisterAck => AUDIO_REGISTER_ACK,
            Self::DeckLayout { .. } => DECK_LAYOUT,
            Self::DeckKeyImage { .. } => DECK_KEY_IMAGE,
            Self::DeckKeyPress { .. } => DECK_KEY_PRESS,
        }
    }

    pub fn direction(&self) -> Direction {
        use Direction::*;
        match self {
            Self::Hello { .. }
            | Self::Ping { .. }
            | Self::InputEvent { .. }
            | Self::MacroTrigger { .. }
            | Self::VoiceStart { .. }
            | Self::VoiceChunk { .. }
            | Self::VoiceEnd { .. }
            | Self::AudioStreamRequest { .. }
            | Self::AudioRegister
            | Self::DeckKeyPress { .. } => ClientToServer,
            Self::Bye { .. } | Self::Error { .. } | Self::AudioStreamStop { .. } => Both,
            _ => ServerToClient,
        }
    }

    /// Only these two may travel on the UDP audio channel.
    pub fn allowed_on_udp(&self) -> bool {
        matches!(self, Self::AudioRegister | Self::AudioPacket { .. })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.type_id());
        match self {
            Self::Hello { caps, sw_version, device_name } => {
                w.u32(*caps).str8(sw_version).str8(device_name);
            }
            Self::HelloAck { caps, host_name, session_tag, audio_port, keepalive_s, budget_kbps } => {
                w.u32(*caps).str8(host_name).u32(*session_tag).u16(*audio_port).u16(*keepalive_s).u16(*budget_kbps);
            }
            Self::Ping { id, t_us } | Self::Pong { id, t_us } => {
                w.u32(*id).u64(*t_us);
            }
            Self::Bye { reason } => {
                w.u8(*reason as u8);
            }
            Self::Error { code, text } => {
                w.u16(*code).str16(text);
            }
            Self::WidgetLayout { format, data } => {
                w.u8(*format).bytes(data);
            }
            Self::WidgetData { widget_id, format, data } => {
                w.u16(*widget_id).u8(*format).bytes(data);
            }
            Self::WidgetAsset { asset_id, format, data } => {
                w.u64(*asset_id).u8(*format).bytes(data);
            }
            Self::InputEvent { kind, page, widget_id, x, y, buttons } => {
                w.u8(*kind).u8(*page).u16(*widget_id).u16(*x).u16(*y).u32(*buttons);
            }
            Self::MacroTrigger { slot } => {
                w.u8(*slot);
            }
            Self::VoiceStart { utterance_id, codec, sample_rate, channels } => {
                w.u32(*utterance_id).u8(*codec).u32(*sample_rate).u8(*channels);
            }
            Self::VoiceChunk { utterance_id, seq, data } => {
                w.u32(*utterance_id).u32(*seq).bytes(data);
            }
            Self::VoiceEnd { utterance_id, reason } => {
                w.u32(*utterance_id).u8(*reason);
            }
            Self::VoiceTranscript { utterance_id, is_final, text }
            | Self::VoiceReplyText { utterance_id, is_final, text } => {
                w.u32(*utterance_id).u8(*is_final as u8).str16(text);
            }
            Self::AudioStreamStart { stream_id, kind, codec, sample_rate, channels, bitrate_kbps } => {
                w.u8(*stream_id).u8(*kind).u8(*codec).u32(*sample_rate).u8(*channels).u16(*bitrate_kbps);
            }
            Self::AudioStreamStop { stream_id, reason } => {
                w.u8(*stream_id).u8(*reason);
            }
            Self::AudioStreamRequest { kind, enable } => {
                w.u8(*kind).u8(*enable as u8);
            }
            Self::AudioPacket { stream_id, seq, pts_ms, data } => {
                w.u8(*stream_id).u32(*seq).u32(*pts_ms).bytes(data);
            }
            Self::AudioRegister | Self::AudioRegisterAck => {}
            Self::DeckLayout { columns, rows, key_w, key_h } => {
                w.u8(*columns).u8(*rows).u8(*key_w).u8(*key_h);
            }
            Self::DeckKeyImage { key, image_id, format, width, height, data } => {
                w.u8(*key).u64(*image_id).u8(*format).u8(*width).u8(*height).bytes(data);
            }
            Self::DeckKeyPress { key, pressed } => {
                w.u8(*key).u8(*pressed as u8);
            }
        }
        w.into_vec()
    }

    pub fn decode(buf: &[u8]) -> Result<Decoded, DecodeError> {
        use msg_type::*;
        let (&ty, body) = buf.split_first().ok_or(DecodeError::Empty)?;
        let mut r = Reader::new(body);
        let msg = match ty {
            HELLO => Self::Hello { caps: r.u32()?, sw_version: r.str8()?, device_name: r.str8()? },
            HELLO_ACK => Self::HelloAck {
                caps: r.u32()?,
                host_name: r.str8()?,
                session_tag: r.u32()?,
                audio_port: r.u16()?,
                keepalive_s: r.u16()?,
                budget_kbps: r.u16()?,
            },
            PING => Self::Ping { id: r.u32()?, t_us: r.u64()? },
            PONG => Self::Pong { id: r.u32()?, t_us: r.u64()? },
            BYE => Self::Bye { reason: ByeReason::from_u8(r.u8()?)? },
            ERROR => Self::Error { code: r.u16()?, text: r.str16()? },
            WIDGET_LAYOUT => Self::WidgetLayout { format: r.u8()?, data: r.rest() },
            WIDGET_DATA => Self::WidgetData { widget_id: r.u16()?, format: r.u8()?, data: r.rest() },
            WIDGET_ASSET => Self::WidgetAsset { asset_id: r.u64()?, format: r.u8()?, data: r.rest() },
            INPUT_EVENT => Self::InputEvent {
                kind: r.u8()?,
                page: r.u8()?,
                widget_id: r.u16()?,
                x: r.u16()?,
                y: r.u16()?,
                buttons: r.u32()?,
            },
            MACRO_TRIGGER => {
                let slot = r.u8()?;
                if slot > 11 {
                    return Err(DecodeError::OutOfRange("macro slot"));
                }
                Self::MacroTrigger { slot }
            }
            VOICE_START => {
                Self::VoiceStart { utterance_id: r.u32()?, codec: r.u8()?, sample_rate: r.u32()?, channels: r.u8()? }
            }
            VOICE_CHUNK => Self::VoiceChunk { utterance_id: r.u32()?, seq: r.u32()?, data: r.rest() },
            VOICE_END => Self::VoiceEnd { utterance_id: r.u32()?, reason: r.u8()? },
            VOICE_TRANSCRIPT => {
                Self::VoiceTranscript { utterance_id: r.u32()?, is_final: bool_u8(r.u8()?)?, text: r.str16()? }
            }
            VOICE_REPLY_TEXT => {
                Self::VoiceReplyText { utterance_id: r.u32()?, is_final: bool_u8(r.u8()?)?, text: r.str16()? }
            }
            AUDIO_STREAM_START => Self::AudioStreamStart {
                stream_id: r.u8()?,
                kind: r.u8()?,
                codec: r.u8()?,
                sample_rate: r.u32()?,
                channels: r.u8()?,
                bitrate_kbps: r.u16()?,
            },
            AUDIO_STREAM_STOP => Self::AudioStreamStop { stream_id: r.u8()?, reason: r.u8()? },
            AUDIO_STREAM_REQUEST => Self::AudioStreamRequest { kind: r.u8()?, enable: bool_u8(r.u8()?)? },
            AUDIO_PACKET => Self::AudioPacket { stream_id: r.u8()?, seq: r.u32()?, pts_ms: r.u32()?, data: r.rest() },
            AUDIO_REGISTER => Self::AudioRegister,
            AUDIO_REGISTER_ACK => Self::AudioRegisterAck,
            DECK_LAYOUT => Self::DeckLayout { columns: r.u8()?, rows: r.u8()?, key_w: r.u8()?, key_h: r.u8()? },
            DECK_KEY_IMAGE => Self::DeckKeyImage {
                key: r.u8()?,
                image_id: r.u64()?,
                format: r.u8()?,
                width: r.u8()?,
                height: r.u8()?,
                data: r.rest(),
            },
            DECK_KEY_PRESS => Self::DeckKeyPress { key: r.u8()?, pressed: bool_u8(r.u8()?)? },
            other => return Ok(Decoded::Unknown(other)),
        };
        r.finish()?;
        Ok(Decoded::Known(msg))
    }
}

fn bool_u8(v: u8) -> Result<bool, DecodeError> {
    match v {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DecodeError::OutOfRange("boolean")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_messages() -> Vec<Message> {
        vec![
            Message::Hello { caps: 1, sw_version: "0.1.0".into(), device_name: "PSP-3000".into() },
            Message::HelloAck {
                caps: 0,
                host_name: "DESKTOP".into(),
                session_tag: 0xdead_beef,
                audio_port: 47862,
                keepalive_s: 15,
                budget_kbps: 300,
            },
            Message::Ping { id: 7, t_us: 123_456_789 },
            Message::Pong { id: 7, t_us: 123_456_789 },
            Message::Bye { reason: ByeReason::Replaced },
            Message::Error { code: error_code::PROTOCOL, text: "bad".into() },
            Message::WidgetLayout { format: 0, data: b"{}".to_vec() },
            Message::WidgetData { widget_id: 3, format: 0, data: br#"{"cpu":12}"#.to_vec() },
            Message::WidgetAsset { asset_id: 99, format: 1, data: vec![1, 2, 3] },
            Message::InputEvent { kind: 0, page: 1, widget_id: 2, x: 240, y: 136, buttons: 0x4000 },
            Message::MacroTrigger { slot: 11 },
            Message::VoiceStart { utterance_id: 1, codec: 1, sample_rate: 11025, channels: 1 },
            Message::VoiceChunk { utterance_id: 1, seq: 0, data: vec![0; 64] },
            Message::VoiceEnd { utterance_id: 1, reason: 0 },
            Message::VoiceTranscript { utterance_id: 1, is_final: true, text: "xin chào".into() },
            Message::VoiceReplyText { utterance_id: 1, is_final: false, text: "Hello".into() },
            Message::AudioStreamStart {
                stream_id: 1,
                kind: 1,
                codec: 1,
                sample_rate: 44100,
                channels: 2,
                bitrate_kbps: 96,
            },
            Message::AudioStreamStop { stream_id: 1, reason: 2 },
            Message::AudioStreamRequest { kind: 1, enable: true },
            Message::AudioPacket { stream_id: 1, seq: 5, pts_ms: 100, data: vec![0xff; 32] },
            Message::AudioRegister,
            Message::AudioRegisterAck,
            Message::DeckLayout { columns: 6, rows: 3, key_w: 76, key_h: 86 },
            Message::DeckKeyImage { key: 4, image_id: 42, format: 1, width: 76, height: 86, data: vec![] },
            Message::DeckKeyPress { key: 4, pressed: true },
        ]
    }

    #[test]
    fn every_message_round_trips() {
        for m in all_messages() {
            let bytes = m.encode();
            assert_eq!(bytes[0], m.type_id());
            assert!(bytes.len() <= MAX_MESSAGE);
            assert_eq!(Message::decode(&bytes).unwrap(), Decoded::Known(m.clone()), "{m:?}");
        }
    }

    #[test]
    fn ping_layout_is_stable() {
        let bytes = Message::Ping { id: 0x0102_0304, t_us: 0x1122_3344_5566_7788 }.encode();
        assert_eq!(bytes, [0x03, 4, 3, 2, 1, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]);
    }

    #[test]
    fn unknown_type_is_not_an_error() {
        assert_eq!(Message::decode(&[0x7f, 1, 2]).unwrap(), Decoded::Unknown(0x7f));
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(Message::decode(&[]), Err(DecodeError::Empty));
        assert_eq!(Message::decode(&[msg_type::PING, 1, 2]), Err(DecodeError::Truncated));
        assert!(matches!(Message::decode(&[msg_type::AUDIO_REGISTER, 0]), Err(DecodeError::Trailing(1))));
        assert!(Message::decode(&[msg_type::MACRO_TRIGGER, 12]).is_err());
        assert!(Message::decode(&[msg_type::DECK_KEY_PRESS, 1, 2]).is_err());
        assert!(Message::decode(&[msg_type::BYE, 9]).is_err());
    }

    #[test]
    fn directions() {
        assert_eq!(Message::AudioRegister.direction(), Direction::ClientToServer);
        assert_eq!(Message::Pong { id: 0, t_us: 0 }.direction(), Direction::ServerToClient);
        assert_eq!(Message::Bye { reason: ByeReason::Normal }.direction(), Direction::Both);
        assert!(!Message::Ping { id: 0, t_us: 0 }.allowed_on_udp());
    }
}
