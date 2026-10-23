//! Host actions run by macro slots, widget taps, and the Stream Deck fallback.
//! Only the slot or widget ID crosses the network; the action itself lives in the host config.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

#[cfg(windows)]
mod windows;

#[derive(Debug, Error)]
pub enum ActionError {
    #[error("not supported on this platform")]
    Unsupported,
    #[error("{0}")]
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKey {
    PlayPause,
    Next,
    Previous,
    Stop,
    VolumeUp,
    VolumeDown,
    Mute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Media {
        key: MediaKey,
    },
    /// Toggles the default communications microphone.
    MuteMic,
    /// Starts a program with arguments; no shell is involved.
    Command {
        program: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Opens a file, folder, URL, or app through the shell (like double-clicking it).
    Open {
        target: String,
    },
    /// A key chord such as "ctrl+shift+m".
    Hotkey {
        keys: Chord,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Ctrl,
    Shift,
    Alt,
    Win,
    /// `A`-`Z` or `0`-`9`.
    Char(char),
    /// F1-F24.
    F(u8),
    Enter,
    Escape,
    Tab,
    Space,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    PrintScreen,
}

impl Key {
    pub fn is_modifier(self) -> bool {
        matches!(self, Self::Ctrl | Self::Shift | Self::Alt | Self::Win)
    }

    fn parse(name: &str) -> Option<Self> {
        let lower = name.trim().to_ascii_lowercase();
        Some(match lower.as_str() {
            "ctrl" | "control" => Self::Ctrl,
            "shift" => Self::Shift,
            "alt" => Self::Alt,
            "win" | "super" | "meta" => Self::Win,
            "enter" | "return" => Self::Enter,
            "esc" | "escape" => Self::Escape,
            "tab" => Self::Tab,
            "space" => Self::Space,
            "backspace" => Self::Backspace,
            "delete" | "del" => Self::Delete,
            "insert" | "ins" => Self::Insert,
            "home" => Self::Home,
            "end" => Self::End,
            "pageup" | "pgup" => Self::PageUp,
            "pagedown" | "pgdn" => Self::PageDown,
            "up" => Self::Up,
            "down" => Self::Down,
            "left" => Self::Left,
            "right" => Self::Right,
            "printscreen" | "prtsc" => Self::PrintScreen,
            s if s.len() == 1 && s.chars().all(|c| c.is_ascii_alphanumeric()) => {
                Self::Char(s.chars().next()?.to_ascii_uppercase())
            }
            s if s.starts_with('f') => match s[1..].parse::<u8>() {
                Ok(n @ 1..=24) => Self::F(n),
                _ => return None,
            },
            _ => return None,
        })
    }

    fn name(self) -> String {
        match self {
            Self::Char(c) => c.to_ascii_lowercase().to_string(),
            Self::F(n) => format!("f{n}"),
            other => format!("{other:?}").to_ascii_lowercase(),
        }
    }
}

/// Modifiers followed by exactly one non-modifier key (or modifiers alone, e.g. "win").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord(pub Vec<Key>);

impl FromStr for Chord {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let keys = s
            .split('+')
            .map(|part| Key::parse(part).ok_or_else(|| format!("unknown key {:?} in hotkey {s:?}", part.trim())))
            .collect::<Result<Vec<_>, _>>()?;
        let plain = keys.iter().filter(|k| !k.is_modifier()).count();
        if keys.is_empty() || plain > 1 || keys.iter().skip_while(|k| k.is_modifier()).any(|k| k.is_modifier()) {
            return Err(format!("hotkey {s:?} must be modifiers followed by at most one key"));
        }
        for (i, k) in keys.iter().enumerate() {
            if keys[..i].contains(k) {
                return Err(format!("hotkey {s:?} repeats a key"));
            }
        }
        Ok(Self(keys))
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<String> = self.0.iter().map(|k| k.name()).collect();
        f.write_str(&names.join("+"))
    }
}

impl Serialize for Chord {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Chord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

impl Action {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Command { program, .. } if program.trim().is_empty() => Err("command program is empty".into()),
            Self::Open { target } if target.trim().is_empty() => Err("open target is empty".into()),
            _ => Ok(()),
        }
    }
}

/// The side effects behind actions. Tests use a recording implementation so nothing touches the desktop.
pub trait Executor: Send + Sync {
    fn key_chord(&self, keys: &[Key]) -> Result<(), ActionError>;
    fn media_key(&self, key: MediaKey) -> Result<(), ActionError>;
    /// Returns true when the microphone is muted afterwards.
    fn toggle_mic_mute(&self) -> Result<bool, ActionError>;
    fn spawn(&self, program: &str, args: &[String]) -> Result<(), ActionError>;
    fn open(&self, target: &str) -> Result<(), ActionError>;
}

pub fn run(exec: &dyn Executor, action: &Action) -> Result<(), ActionError> {
    match action {
        Action::Media { key } => exec.media_key(*key),
        Action::MuteMic => exec.toggle_mic_mute().map(|_| ()),
        Action::Command { program, args } => exec.spawn(program, args),
        Action::Open { target } => exec.open(target),
        Action::Hotkey { keys } => exec.key_chord(&keys.0),
    }
}

/// Runs an action on a blocking thread (input injection and COM calls block briefly).
pub async fn run_async(exec: Arc<dyn Executor>, action: Action) -> Result<(), ActionError> {
    tokio::task::spawn_blocking(move || run(exec.as_ref(), &action))
        .await
        .map_err(|e| ActionError::Failed(format!("action task failed: {e}")))?
}

/// The real desktop executor for this platform.
pub fn system_executor() -> Arc<dyn Executor> {
    #[cfg(windows)]
    {
        Arc::new(windows::WindowsExecutor)
    }
    #[cfg(not(windows))]
    {
        Arc::new(PortableExecutor)
    }
}

/// Outside Windows only process actions work; key injection and mic mute report Unsupported.
#[cfg(not(windows))]
struct PortableExecutor;

#[cfg(not(windows))]
impl Executor for PortableExecutor {
    fn key_chord(&self, _: &[Key]) -> Result<(), ActionError> {
        Err(ActionError::Unsupported)
    }
    fn media_key(&self, _: MediaKey) -> Result<(), ActionError> {
        Err(ActionError::Unsupported)
    }
    fn toggle_mic_mute(&self) -> Result<bool, ActionError> {
        Err(ActionError::Unsupported)
    }
    fn spawn(&self, program: &str, args: &[String]) -> Result<(), ActionError> {
        spawn_detached(program, args)
    }
    fn open(&self, target: &str) -> Result<(), ActionError> {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        spawn_detached(opener, &[target.to_string()])
    }
}

pub(crate) fn spawn_detached(program: &str, args: &[String]) -> Result<(), ActionError> {
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| ActionError::Failed(format!("cannot start {program}: {e}")))
}

#[cfg(test)]
pub mod testing {
    use std::sync::Mutex;

    use super::*;

    /// Records what would have happened instead of touching the desktop.
    #[derive(Default)]
    pub struct RecordingExecutor {
        pub log: Mutex<Vec<String>>,
        muted: Mutex<bool>,
    }

    impl RecordingExecutor {
        pub fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.log.lock().unwrap())
        }
        fn push(&self, s: String) {
            self.log.lock().unwrap().push(s);
        }
    }

    impl Executor for RecordingExecutor {
        fn key_chord(&self, keys: &[Key]) -> Result<(), ActionError> {
            self.push(format!("keys {}", Chord(keys.to_vec())));
            Ok(())
        }
        fn media_key(&self, key: MediaKey) -> Result<(), ActionError> {
            self.push(format!("media {key:?}"));
            Ok(())
        }
        fn toggle_mic_mute(&self) -> Result<bool, ActionError> {
            let mut m = self.muted.lock().unwrap();
            *m = !*m;
            self.push(format!("mic muted={}", *m));
            Ok(*m)
        }
        fn spawn(&self, program: &str, args: &[String]) -> Result<(), ActionError> {
            self.push(format!("spawn {program} {}", args.join(" ")));
            Ok(())
        }
        fn open(&self, target: &str) -> Result<(), ActionError> {
            self.push(format!("open {target}"));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::RecordingExecutor;
    use super::*;

    #[test]
    fn parses_chords() {
        let c: Chord = "Ctrl+Shift+M".parse().unwrap();
        assert_eq!(c.0, vec![Key::Ctrl, Key::Shift, Key::Char('M')]);
        assert_eq!(c.to_string(), "ctrl+shift+m");
        assert_eq!("win".parse::<Chord>().unwrap().0, vec![Key::Win]);
        assert_eq!("alt+f4".parse::<Chord>().unwrap().0, vec![Key::Alt, Key::F(4)]);
        assert!("ctrl+a+b".parse::<Chord>().is_err());
        assert!("a+ctrl".parse::<Chord>().is_err());
        assert!("ctrl+ctrl".parse::<Chord>().is_err());
        assert!("f25".parse::<Chord>().is_err());
        assert!("ctrl+nope".parse::<Chord>().is_err());
    }

    #[test]
    fn actions_deserialize_from_toml() {
        #[derive(Deserialize)]
        struct T {
            a: Vec<Action>,
        }
        let t: T = toml::from_str(
            r#"a = [
                { type = "media", key = "play_pause" },
                { type = "mute_mic" },
                { type = "command", program = "notepad.exe", args = ["a.txt"] },
                { type = "open", target = "https://example.com" },
                { type = "hotkey", keys = "win+d" },
            ]"#,
        )
        .unwrap();
        assert_eq!(t.a[0], Action::Media { key: MediaKey::PlayPause });
        assert_eq!(t.a[4], Action::Hotkey { keys: "win+d".parse().unwrap() });
        assert!(toml::from_str::<T>(r#"a = [{ type = "hotkey", keys = "ctrl+bogus" }]"#).is_err());
    }

    #[test]
    fn run_dispatches_to_executor() {
        let rec = RecordingExecutor::default();
        run(&rec, &Action::Media { key: MediaKey::Next }).unwrap();
        run(&rec, &Action::MuteMic).unwrap();
        run(&rec, &Action::Command { program: "tool".into(), args: vec!["-x".into()] }).unwrap();
        run(&rec, &Action::Open { target: "calc.exe".into() }).unwrap();
        run(&rec, &Action::Hotkey { keys: "ctrl+c".parse().unwrap() }).unwrap();
        assert_eq!(
            rec.take(),
            ["media Next", "mic muted=true", "spawn tool -x", "open calc.exe", "keys ctrl+c"].map(String::from)
        );
    }
}
