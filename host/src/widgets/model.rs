//! Widget values and their binary encoding (`WidgetData` format 1, see protocol/PROTOCOL.md).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::net::codec::{Writer, truncate_utf8};

pub const FORMAT_BINARY: u8 = 1;
pub const MAX_NOTIFICATIONS: usize = 5;
pub const MAX_CLAUDE_SESSIONS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum WidgetKind {
    Clock = 1,
    System = 2,
    NowPlaying = 3,
    Notifications = 4,
    Calendar = 5,
    ClaudeCode = 6,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemStats {
    pub cpu_pct: Option<u8>,
    pub gpu_pct: Option<u8>,
    pub ram_used_mb: u32,
    pub ram_total_mb: u32,
    /// Tenths of a degree Celsius.
    pub cpu_temp_dc: Option<i16>,
    pub gpu_temp_dc: Option<i16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlayState {
    None = 0,
    Playing = 1,
    Paused = 2,
    Stopped = 3,
}

/// An encoded image sent once with `WidgetAsset` and cached on the PSP by `id`.
#[derive(Debug, PartialEq, Eq)]
pub struct Asset {
    pub id: u64,
    pub format: u8,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    pub state: PlayState,
    pub title: String,
    pub artist: String,
    pub cover: Option<Arc<Asset>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub app: String,
    pub title: String,
    pub body: String,
    /// Host-formatted local time, e.g. "14:05".
    pub time: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEvent {
    pub title: String,
    /// Host-formatted, e.g. "in 25 min" or "Tue 09:00".
    pub when: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClaudeState {
    Idle = 0,
    Working = 1,
    Waiting = 2,
    Done = 3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSession {
    pub project: String,
    pub state: ClaudeState,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetValue {
    System(SystemStats),
    NowPlaying(NowPlaying),
    Notifications(Vec<Notification>),
    Calendar(Option<CalendarEvent>),
    ClaudeCode(Vec<ClaudeSession>),
}

/// NFC (the PSP font has precomposed Vietnamese letters only), no control characters, bounded length.
pub fn clean_text(s: &str, max: usize) -> String {
    let nfc: String = s.nfc().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let collapsed = nfc.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_utf8(&collapsed, max).to_string()
}

impl WidgetValue {
    pub fn kind(&self) -> WidgetKind {
        match self {
            Self::System(_) => WidgetKind::System,
            Self::NowPlaying(_) => WidgetKind::NowPlaying,
            Self::Notifications(_) => WidgetKind::Notifications,
            Self::Calendar(_) => WidgetKind::Calendar,
            Self::ClaudeCode(_) => WidgetKind::ClaudeCode,
        }
    }

    pub fn asset(&self) -> Option<&Arc<Asset>> {
        match self {
            Self::NowPlaying(n) => n.cover.as_ref(),
            _ => None,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let t = |s: &str, max| clean_text(s, max);
        match self {
            Self::System(s) => {
                w.u8(s.cpu_pct.map_or(255, |v| v.min(100)))
                    .u8(s.gpu_pct.map_or(255, |v| v.min(100)))
                    .u32(s.ram_used_mb)
                    .u32(s.ram_total_mb)
                    .u16(s.cpu_temp_dc.unwrap_or(i16::MIN) as u16)
                    .u16(s.gpu_temp_dc.unwrap_or(i16::MIN) as u16);
            }
            Self::NowPlaying(n) => {
                w.u8(n.state as u8)
                    .u64(n.cover.as_ref().map_or(0, |a| a.id))
                    .str8(&t(&n.title, 96))
                    .str8(&t(&n.artist, 64));
            }
            Self::Notifications(list) => {
                let list = &list[..list.len().min(MAX_NOTIFICATIONS)];
                w.u8(list.len() as u8);
                for n in list {
                    w.str8(&t(&n.app, 32)).str8(&t(&n.title, 64)).str8(&t(&n.body, 160)).str8(&t(&n.time, 16));
                }
            }
            Self::Calendar(ev) => match ev {
                Some(e) => {
                    w.u8(1).str8(&t(&e.title, 96)).str8(&t(&e.when, 32));
                }
                None => {
                    w.u8(0);
                }
            },
            Self::ClaudeCode(list) => {
                let list = &list[..list.len().min(MAX_CLAUDE_SESSIONS)];
                w.u8(list.len() as u8);
                for s in list {
                    w.str8(&t(&s.project, 40)).u8(s.state as u8).str8(&t(&s.detail, 64));
                }
            }
        }
        w.into_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_text() {
        // "Việt" with a combining dot below and circumflex becomes precomposed.
        assert_eq!(clean_text("Vie\u{323}\u{302}t\n  Nam\u{7}", 64), "Việt Nam");
        assert_eq!(clean_text(&"ộ".repeat(50), 10), "ộộộ");
    }

    #[test]
    fn system_layout_is_stable() {
        let v = WidgetValue::System(SystemStats {
            cpu_pct: Some(12),
            gpu_pct: None,
            ram_used_mb: 8000,
            ram_total_mb: 16000,
            cpu_temp_dc: Some(455),
            gpu_temp_dc: None,
        });
        let b = v.encode();
        assert_eq!(b.len(), 14);
        assert_eq!(&b[..2], &[12, 255]);
        assert_eq!(&b[10..12], &455u16.to_le_bytes());
        assert_eq!(&b[12..14], &[0x00, 0x80]);
    }

    #[test]
    fn lists_are_capped() {
        let n = Notification { app: "a".into(), title: "t".into(), body: "b".into(), time: "1".into() };
        let b = WidgetValue::Notifications(vec![n; 9]).encode();
        assert_eq!(b[0] as usize, MAX_NOTIFICATIONS);
        assert_eq!(WidgetValue::Calendar(None).encode(), [0]);
    }
}
