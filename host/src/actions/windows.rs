//! Windows side effects: SendInput for keys, Core Audio for the microphone, ShellExecute for open.

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eCommunications};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, w};

use super::{ActionError, Executor, Key, MediaKey, spawn_detached};

pub struct WindowsExecutor;

fn vk(key: Key) -> (u16, bool) {
    // (virtual-key code, needs the extended-key flag)
    match key {
        Key::Ctrl => (0x11, false),
        Key::Shift => (0x10, false),
        Key::Alt => (0x12, false),
        Key::Win => (0x5B, true),
        Key::Char(c) => (c.to_ascii_uppercase() as u16, false),
        Key::F(n) => (0x6F + u16::from(n), false),
        Key::Enter => (0x0D, false),
        Key::Escape => (0x1B, false),
        Key::Tab => (0x09, false),
        Key::Space => (0x20, false),
        Key::Backspace => (0x08, false),
        Key::Delete => (0x2E, true),
        Key::Insert => (0x2D, true),
        Key::Home => (0x24, true),
        Key::End => (0x23, true),
        Key::PageUp => (0x21, true),
        Key::PageDown => (0x22, true),
        Key::Up => (0x26, true),
        Key::Down => (0x28, true),
        Key::Left => (0x25, true),
        Key::Right => (0x27, true),
        Key::PrintScreen => (0x2C, true),
    }
}

fn media_vk(key: MediaKey) -> u16 {
    match key {
        MediaKey::PlayPause => 0xB3,
        MediaKey::Next => 0xB0,
        MediaKey::Previous => 0xB1,
        MediaKey::Stop => 0xB2,
        MediaKey::VolumeUp => 0xAF,
        MediaKey::VolumeDown => 0xAE,
        MediaKey::Mute => 0xAD,
    }
}

fn key_input(code: u16, extended: bool, up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(code), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

/// Presses all keys in order, then releases them in reverse order, in one SendInput call.
fn send_chord(codes: &[(u16, bool)]) -> Result<(), ActionError> {
    let mut inputs: Vec<INPUT> = codes.iter().map(|&(c, e)| key_input(c, e, false)).collect();
    inputs.extend(codes.iter().rev().map(|&(c, e)| key_input(c, e, true)));
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(ActionError::Failed(format!(
            "SendInput injected {sent} of {} events (blocked by a higher-integrity window?)",
            inputs.len()
        )))
    }
}

fn com_init() -> Result<(), ActionError> {
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_ok() || hr == RPC_E_CHANGED_MODE {
        Ok(())
    } else {
        Err(ActionError::Failed(format!("COM init failed: {hr:?}")))
    }
}

fn win_err(what: &str) -> impl Fn(windows::core::Error) -> ActionError + '_ {
    move |e| ActionError::Failed(format!("{what}: {e}"))
}

impl Executor for WindowsExecutor {
    fn key_chord(&self, keys: &[Key]) -> Result<(), ActionError> {
        let codes: Vec<(u16, bool)> = keys.iter().map(|&k| vk(k)).collect();
        send_chord(&codes)
    }

    fn media_key(&self, key: MediaKey) -> Result<(), ActionError> {
        send_chord(&[(media_vk(key), true)])
    }

    fn toggle_mic_mute(&self) -> Result<bool, ActionError> {
        com_init()?;
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(win_err("audio devices"))?;
            let device =
                enumerator.GetDefaultAudioEndpoint(eCapture, eCommunications).map_err(win_err("no microphone"))?;
            let volume: IAudioEndpointVolume =
                device.Activate(CLSCTX_ALL, None).map_err(win_err("microphone volume"))?;
            let muted = volume.GetMute().map_err(win_err("read mute"))?.as_bool();
            volume.SetMute(!muted, std::ptr::null()).map_err(win_err("set mute"))?;
            Ok(!muted)
        }
    }

    fn spawn(&self, program: &str, args: &[String]) -> Result<(), ActionError> {
        spawn_detached(program, args)
    }

    fn open(&self, target: &str) -> Result<(), ActionError> {
        let r = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(target), None, None, SW_SHOWNORMAL) };
        // ShellExecute reports success as a value greater than 32.
        if r.0 as isize > 32 {
            Ok(())
        } else {
            Err(ActionError::Failed(format!("cannot open {target} (code {})", r.0 as isize)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_key_codes() {
        assert_eq!(vk(Key::Char('a')), (0x41, false));
        assert_eq!(vk(Key::Char('7')), (0x37, false));
        assert_eq!(vk(Key::F(1)), (0x70, false));
        assert_eq!(vk(Key::F(24)), (0x87, false));
        assert_eq!(media_vk(MediaKey::PlayPause), 0xB3);
    }
}
