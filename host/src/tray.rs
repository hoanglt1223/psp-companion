//! System tray UI: connection status, live throughput, the pairing PIN, and actions.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{error, info, warn};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use crate::net::message::ByeReason;
use crate::state::{AppState, Snapshot};

const REFRESH: Duration = Duration::from_secs(1);

pub fn kbps(bytes: u64, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 { 0.0 } else { bytes as f64 * 8.0 / 1000.0 / secs }
}

pub struct StatusText {
    pub status: String,
    pub pin: String,
    pub traffic: String,
}

pub fn status_text(s: &Snapshot, kbps_tx: f64, kbps_rx: f64, budget_kbps: u32) -> StatusText {
    let status = match s.sessions.as_slice() {
        [] => format!("No PSP connected ({} paired)", s.paired_devices),
        [one] => format!("Connected: {} ({})", one.device_name, one.addr.ip()),
        many => format!("{} PSPs connected", many.len()),
    };
    let pin = match &s.pin {
        Some((pin, left)) => format!("Pairing PIN: {} {} ({}s left)", &pin[..3], &pin[3..], left.as_secs()),
        None => "Pairing closed".to_string(),
    };
    let traffic = format!("Wi-Fi: out {kbps_tx:.1} / in {kbps_rx:.1} kbps (budget {budget_kbps})");
    StatusText { status, pin, traffic }
}

#[derive(Debug)]
pub enum UserEvent {
    Menu(MenuId),
}

struct Items {
    status: MenuItem,
    pin: MenuItem,
    traffic: MenuItem,
    pair: MenuItem,
    forget: MenuItem,
    logs: MenuItem,
    config: MenuItem,
    quit: MenuItem,
}

struct TrayApp {
    state: Arc<AppState>,
    log_dir: PathBuf,
    config_file: PathBuf,
    items: Items,
    menu: Menu,
    tray: Option<TrayIcon>,
    last: (Instant, u64, u64),
}

pub fn run(state: Arc<AppState>, log_dir: PathBuf, config_file: PathBuf) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().map_err(|e| e.to_string())?;
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = proxy.send_event(UserEvent::Menu(e.id));
    }));

    let items = Items {
        status: MenuItem::new("Starting...", false, None),
        pin: MenuItem::new("Pairing closed", false, None),
        traffic: MenuItem::new("", false, None),
        pair: MenuItem::new("Pair new PSP (show PIN)", true, None),
        forget: MenuItem::new("Forget all paired PSPs", true, None),
        logs: MenuItem::new("Open log folder", true, None),
        config: MenuItem::new("Open config folder", true, None),
        quit: MenuItem::new("Quit", true, None),
    };
    let menu = Menu::new();
    menu.append_items(&[
        &items.status,
        &items.pin,
        &items.traffic,
        &PredefinedMenuItem::separator(),
        &items.pair,
        &items.forget,
        &items.logs,
        &items.config,
        &PredefinedMenuItem::separator(),
        &items.quit,
    ])
    .map_err(|e| e.to_string())?;

    let snap = state.snapshot();
    let mut app = TrayApp {
        state,
        log_dir,
        config_file,
        items,
        menu,
        tray: None,
        last: (Instant::now(), snap.bytes_tx, snap.bytes_rx),
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())
}

fn icon() -> Result<Icon, String> {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            // A dark rounded "PSP body" with a lighter screen in the middle.
            let screen = (8..24).contains(&x) && (10..22).contains(&y);
            let body = (2..30).contains(&x) && (7..25).contains(&y);
            let px = if screen {
                [0x4f, 0xc3, 0xf7, 0xff]
            } else if body {
                [0x26, 0x32, 0x38, 0xff]
            } else {
                [0, 0, 0, 0]
            };
            rgba.extend_from_slice(&px);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| e.to_string())
}

fn open_folder(path: &std::path::Path) {
    if let Err(e) = std::fs::create_dir_all(path) {
        warn!("cannot create {}: {e}", path.display());
    }
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(path).spawn() {
        warn!("cannot open {}: {e}", path.display());
    }
}

impl TrayApp {
    fn refresh(&mut self) {
        let snap = self.state.snapshot();
        let now = Instant::now();
        let (t, tx, rx) = self.last;
        let elapsed = now - t;
        let text = status_text(
            &snap,
            kbps(snap.bytes_tx - tx, elapsed),
            kbps(snap.bytes_rx - rx, elapsed),
            self.state.config().network.budget_kbps,
        );
        self.last = (now, snap.bytes_tx, snap.bytes_rx);
        self.items.status.set_text(&text.status);
        self.items.pin.set_text(&text.pin);
        self.items.traffic.set_text(&text.traffic);
        if let Some(tray) = &self.tray {
            let tip = format!("PSP Companion\n{}\n{}", text.status, text.pin);
            if let Err(e) = tray.set_tooltip(Some(tip)) {
                warn!("tray tooltip update failed: {e}");
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for TrayApp {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if cause == StartCause::Init {
            let built = icon().and_then(|icon| {
                TrayIconBuilder::new()
                    .with_menu(Box::new(self.menu.clone()))
                    .with_tooltip("PSP Companion")
                    .with_icon(icon)
                    .build()
                    .map_err(|e| e.to_string())
            });
            match built {
                Ok(t) => self.tray = Some(t),
                Err(e) => {
                    error!("cannot create tray icon: {e}; use --headless instead");
                    event_loop.exit();
                    return;
                }
            }
        }
        if matches!(cause, StartCause::Init | StartCause::ResumeTimeReached { .. }) {
            self.refresh();
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + REFRESH));
        }
    }

    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Menu(id) = event;
        if id == *self.items.pair.id() {
            self.state.open_pairing();
        } else if id == *self.items.forget.id() {
            match self.state.forget_all_devices() {
                Ok(()) => info!("all paired devices forgotten"),
                Err(e) => error!("cannot forget devices: {e}"),
            }
        } else if id == *self.items.logs.id() {
            open_folder(&self.log_dir);
        } else if id == *self.items.config.id() {
            if let Some(dir) = self.config_file.parent() {
                open_folder(dir);
            }
        } else if id == *self.items.quit.id() {
            self.state.close_all(ByeReason::ShuttingDown);
            self.tray = None;
            event_loop.exit();
            return;
        }
        self.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SessionSummary;

    #[test]
    fn kbps_math() {
        assert_eq!(kbps(37_500, Duration::from_secs(1)), 300.0);
        assert_eq!(kbps(100, Duration::ZERO), 0.0);
    }

    #[test]
    fn status_text_variants() {
        let mut s = Snapshot { pin: None, sessions: vec![], paired_devices: 1, bytes_tx: 0, bytes_rx: 0 };
        let t = status_text(&s, 0.0, 0.0, 300);
        assert_eq!(t.status, "No PSP connected (1 paired)");
        assert_eq!(t.pin, "Pairing closed");
        s.pin = Some(("123456".into(), Duration::from_secs(90)));
        s.sessions.push(SessionSummary {
            device_name: "PSP".into(),
            addr: "192.168.1.5:5000".parse().unwrap(),
            udp_registered: true,
        });
        let t = status_text(&s, 1.5, 0.5, 300);
        assert_eq!(t.status, "Connected: PSP (192.168.1.5)");
        assert_eq!(t.pin, "Pairing PIN: 123 456 (90s left)");
        assert_eq!(t.traffic, "Wi-Fi: out 1.5 / in 0.5 kbps (budget 300)");
    }
}
