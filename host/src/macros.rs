//! The twelve macro slots of the PSP macro pad. Configured on the host; the PSP sends only the slot number.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::actions::{self, Action, Executor, MediaKey};

pub const SLOTS: u8 = 12;
pub const MAX_LABEL: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacroSlot {
    pub slot: u8,
    pub label: String,
    pub action: Action,
}

fn slot(slot: u8, label: &str, action: Action) -> MacroSlot {
    MacroSlot { slot, label: label.to_string(), action }
}

fn hotkey(keys: &str) -> Action {
    Action::Hotkey { keys: keys.parse().expect("built-in hotkey") }
}

pub fn default_macros() -> Vec<MacroSlot> {
    use MediaKey::*;
    vec![
        slot(0, "Play/Pause", Action::Media { key: PlayPause }),
        slot(1, "Previous", Action::Media { key: Previous }),
        slot(2, "Next", Action::Media { key: Next }),
        slot(3, "Volume -", Action::Media { key: VolumeDown }),
        slot(4, "Volume +", Action::Media { key: VolumeUp }),
        slot(5, "Mute", Action::Media { key: Mute }),
        slot(6, "Mic mute", Action::MuteMic),
        slot(7, "Desktop", hotkey("win+d")),
        slot(8, "Task Mgr", hotkey("ctrl+shift+escape")),
        slot(9, "Snip", hotkey("win+shift+s")),
        slot(10, "Calculator", Action::Open { target: "calc.exe".into() }),
        slot(11, "Notepad", Action::Command { program: "notepad.exe".into(), args: vec![] }),
    ]
}

pub fn validate(macros: &[MacroSlot]) -> Result<(), String> {
    for (i, m) in macros.iter().enumerate() {
        if m.slot >= SLOTS {
            return Err(format!("macro slot {} is out of range 0-{}", m.slot, SLOTS - 1));
        }
        if macros[..i].iter().any(|o| o.slot == m.slot) {
            return Err(format!("macro slot {} is listed twice", m.slot));
        }
        if m.label.trim().is_empty() || m.label.len() > MAX_LABEL {
            return Err(format!("macro slot {} label must be 1-{MAX_LABEL} bytes", m.slot));
        }
        m.action.validate().map_err(|e| format!("macro slot {}: {e}", m.slot))?;
    }
    Ok(())
}

/// Runs slot actions, ignoring bursts beyond a few per second (a stuck button must not spam the desktop).
pub struct MacroRunner {
    macros: Vec<MacroSlot>,
    exec: Arc<dyn Executor>,
    recent: Vec<Instant>,
}

const BURST: usize = 5;
const BURST_WINDOW: Duration = Duration::from_secs(1);

impl MacroRunner {
    pub fn new(macros: Vec<MacroSlot>, exec: Arc<dyn Executor>) -> Self {
        Self { macros, exec, recent: Vec::new() }
    }

    pub fn executor(&self) -> Arc<dyn Executor> {
        self.exec.clone()
    }

    /// Admits an action under the burst limit; returns false when it must be dropped.
    pub fn admit(&mut self, now: Instant) -> bool {
        self.recent.retain(|t| now.duration_since(*t) < BURST_WINDOW);
        if self.recent.len() >= BURST {
            return false;
        }
        self.recent.push(now);
        true
    }

    pub fn action_for(&self, slot: u8) -> Option<&MacroSlot> {
        self.macros.iter().find(|m| m.slot == slot)
    }

    pub async fn trigger(&mut self, slot: u8) {
        let Some(m) = self.action_for(slot).cloned() else {
            info!(slot, "macro slot is empty");
            return;
        };
        self.run(&m.label, m.action).await;
    }

    pub async fn run(&mut self, label: &str, action: Action) {
        if !self.admit(Instant::now()) {
            warn!(label, "action dropped: too many in one second");
            return;
        }
        match actions::run_async(self.exec.clone(), action).await {
            Ok(()) => info!(label, "action ran"),
            Err(e) => warn!(label, "action failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::testing::RecordingExecutor;

    #[test]
    fn defaults_are_valid_and_cover_all_slots() {
        let d = default_macros();
        validate(&d).unwrap();
        assert_eq!(d.len(), SLOTS as usize);
    }

    #[test]
    fn rejects_bad_slots() {
        let mut d = default_macros();
        d[1].slot = 0;
        assert!(validate(&d).is_err());
        let mut d = default_macros();
        d[0].slot = 12;
        assert!(validate(&d).is_err());
        let mut d = default_macros();
        d[0].label = "x".repeat(25);
        assert!(validate(&d).is_err());
    }

    #[tokio::test]
    async fn triggers_slot_and_limits_bursts() {
        let rec = Arc::new(RecordingExecutor::default());
        let mut r = MacroRunner::new(default_macros(), rec.clone());
        r.trigger(0).await;
        r.trigger(7).await;
        assert_eq!(rec.take(), ["media PlayPause", "keys win+d"].map(String::from));
        for _ in 0..10 {
            r.trigger(2).await;
        }
        assert_eq!(rec.take().len(), BURST - 2, "burst limit counts the earlier two");
    }
}
