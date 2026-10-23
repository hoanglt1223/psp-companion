//! The widget layout file (`layout.toml`) and its `WidgetLayout` encoding (format 1), sent once per session.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::model::{WidgetKind, clean_text};
use crate::actions::{Action, MediaKey};
use crate::macros::MacroSlot;
use crate::net::codec::Writer;

pub const SCREEN_W: u16 = 480;
pub const SCREEN_H: u16 = 272;
pub const MAX_PAGES: usize = 3;
pub const MAX_WIDGETS: usize = 16;
pub const FLAG_TAPPABLE: u8 = 0x01;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetDef {
    pub id: u16,
    pub kind: WidgetKind,
    pub page: u8,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    #[serde(default)]
    pub title: String,
    /// Runs on the host when the widget is tapped on the PSP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap: Option<Action>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub pages: Vec<String>,
    pub widgets: Vec<WidgetDef>,
}

fn w(id: u16, kind: WidgetKind, page: u8, rect: (u16, u16, u16, u16), title: &str) -> WidgetDef {
    WidgetDef { id, kind, page, x: rect.0, y: rect.1, w: rect.2, h: rect.3, title: title.into(), tap: None }
}

impl Default for Layout {
    /// The PSP draws its page title bar in the top 20 pixels, so widgets start below it.
    fn default() -> Self {
        use WidgetKind::*;
        let mut now_playing = w(2, NowPlaying, 0, (244, 24, 228, 116), "Now playing");
        now_playing.tap = Some(Action::Media { key: MediaKey::PlayPause });
        Self {
            pages: vec!["Home".into(), "Inbox".into(), "Claude".into()],
            widgets: vec![
                w(1, Clock, 0, (8, 24, 228, 116), "Clock"),
                now_playing,
                w(3, System, 0, (8, 148, 228, 116), "PC"),
                w(4, Calendar, 0, (244, 148, 228, 116), "Next event"),
                w(5, Notifications, 1, (8, 24, 464, 240), "Notifications"),
                w(6, ClaudeCode, 2, (8, 24, 464, 240), "Claude Code"),
            ],
        }
    }
}

impl Layout {
    pub fn validate(&self) -> Result<(), String> {
        if self.pages.is_empty() || self.pages.len() > MAX_PAGES {
            return Err(format!("layout needs 1-{MAX_PAGES} pages"));
        }
        if self.widgets.len() > MAX_WIDGETS {
            return Err(format!("layout has more than {MAX_WIDGETS} widgets"));
        }
        for (i, d) in self.widgets.iter().enumerate() {
            if self.widgets[..i].iter().any(|o| o.id == d.id) {
                return Err(format!("widget id {} is used twice", d.id));
            }
            if usize::from(d.page) >= self.pages.len() {
                return Err(format!("widget {} is on page {} which does not exist", d.id, d.page));
            }
            if d.w < 16 || d.h < 16 || d.x + d.w > SCREEN_W || d.y + d.h > SCREEN_H {
                return Err(format!("widget {} must be at least 16x16 and fit the 480x272 screen", d.id));
            }
            if let Some(a) = &d.tap {
                a.validate().map_err(|e| format!("widget {} tap: {e}", d.id))?;
            }
        }
        Ok(())
    }

    pub fn widget(&self, id: u16) -> Option<&WidgetDef> {
        self.widgets.iter().find(|d| d.id == id)
    }

    pub fn load_or_create(path: &Path) -> Result<Self, String> {
        let layout = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Layout>(&text).map_err(|e| format!("invalid {}: {e}", path.display()))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let layout = Self::default();
                let text = format!(
                    "# PSP Companion widget layout. Screen is 480x272; the top 20 px hold the page title.\n\
                     # kinds: clock, system, now_playing, notifications, calendar, claude_code\n{}",
                    toml::to_string_pretty(&layout).map_err(|e| e.to_string())?
                );
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
                }
                std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                layout
            }
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        layout.validate().map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(layout)
    }

    /// Pages, widgets, and the macro pad labels in one message.
    pub fn encode(&self, macros: &[MacroSlot]) -> Vec<u8> {
        let mut out = Writer::new();
        out.u8(self.pages.len() as u8);
        for p in &self.pages {
            out.str8(&clean_text(p, 24));
        }
        out.u8(self.widgets.len() as u8);
        for d in &self.widgets {
            out.u16(d.id)
                .u8(d.kind as u8)
                .u8(d.page)
                .u16(d.x)
                .u16(d.y)
                .u16(d.w)
                .u16(d.h)
                .u8(if d.tap.is_some() { FLAG_TAPPABLE } else { 0 })
                .str8(&clean_text(&d.title, 32));
        }
        let macros = &macros[..macros.len().min(12)];
        out.u8(macros.len() as u8);
        for m in macros {
            out.u8(m.slot).str8(&clean_text(&m.label, 24));
        }
        out.into_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macros::default_macros;

    #[test]
    fn default_is_valid_and_round_trips_through_toml() {
        let l = Layout::default();
        l.validate().unwrap();
        let back: Layout = toml::from_str(&toml::to_string_pretty(&l).unwrap()).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn rejects_bad_layouts() {
        let mut l = Layout::default();
        l.widgets[0].x = 400;
        assert!(l.validate().is_err());
        let mut l = Layout::default();
        l.widgets[1].id = 1;
        assert!(l.validate().is_err());
        let mut l = Layout::default();
        l.widgets[0].page = 3;
        assert!(l.validate().is_err());
    }

    #[test]
    fn creates_file_once() {
        let dir = std::env::temp_dir().join(format!("pspc-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("layout.toml");
        let a = Layout::load_or_create(&path).unwrap();
        assert!(path.exists());
        assert_eq!(Layout::load_or_create(&path).unwrap(), a);
        std::fs::write(&path, "pages = []\nwidgets = []\n").unwrap();
        assert!(Layout::load_or_create(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn encoding_header() {
        let b = Layout::default().encode(&default_macros());
        assert_eq!(b[0], 3);
        assert_eq!(&b[1..6], b"\x04Home");
        assert!(b.len() < 600, "layout is small: {}", b.len());
    }
}
