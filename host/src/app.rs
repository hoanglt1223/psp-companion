//! Feature router: consumes network events and hands them to widgets, macros, audio, voice, and deck.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::actions::Executor;
use crate::audio::capture::CaptureSource;
use crate::audio::stream::{self, AudioCmd, AudioHandle};
use crate::config::Config;
use crate::macros::MacroRunner;
use crate::net::message::Message;
use crate::state::{AppState, HostEvent};
use crate::widgets::layout::Layout;
use crate::widgets::{self, HubCmd, WidgetHandle};

pub const INPUT_TAP: u8 = 0;

/// The desktop-facing parts, swapped for recording or generated ones in tests.
pub struct Backends {
    pub exec: Arc<dyn Executor>,
    pub capture: Arc<dyn CaptureSource>,
}

pub struct Features {
    pub layout: Arc<Layout>,
    pub widgets: Option<WidgetHandle>,
    pub macros: MacroRunner,
    pub audio: AudioHandle,
}

impl Features {
    /// Builds the hub, data sources, and audio manager from the config. Must run inside the tokio runtime.
    pub async fn start(state: &Arc<AppState>, config: &Config, layout: Layout, backends: Backends) -> Self {
        let layout = Arc::new(layout);
        let w = &config.widgets;
        let widgets = if w.enabled {
            let handle = widgets::spawn_hub(state.clone(), layout.clone(), layout.encode(&config.macros));
            widgets::system::spawn(handle.clone(), Duration::from_secs(u64::from(w.system_interval_secs)));
            #[cfg(windows)]
            if w.now_playing {
                widgets::media::spawn(handle.clone());
            }
            if !w.calendar_ics.trim().is_empty() {
                widgets::calendar::spawn(
                    handle.clone(),
                    w.calendar_ics.trim().to_string(),
                    Duration::from_secs(u64::from(w.calendar_refresh_mins) * 60),
                );
            }
            if w.ingest_port != 0 {
                match widgets::ingest::bind(w.ingest_port).await {
                    Ok(l) => widgets::ingest::spawn(l, handle.clone(), w.ingest_token.clone()),
                    Err(e) => warn!(port = w.ingest_port, "widget ingest endpoint disabled: {e}"),
                }
            }
            Some(handle)
        } else {
            None
        };
        let audio = stream::spawn_manager(state.clone(), backends.capture);
        Self { layout, widgets, macros: MacroRunner::new(config.macros.clone(), backends.exec), audio }
    }

    async fn on_message(&mut self, device: crate::net::handshake::Id16, msg: Message) {
        match msg {
            Message::MacroTrigger { slot } => self.macros.trigger(slot).await,
            Message::InputEvent { kind: INPUT_TAP, widget_id, .. } => {
                match self.layout.widget(widget_id).and_then(|d| d.tap.clone().map(|a| (d.title.clone(), a))) {
                    Some((title, action)) => self.macros.run(&title, action).await,
                    None => debug!(widget_id, "tap on a widget without an action"),
                }
            }
            Message::InputEvent { .. } => {}
            Message::AudioStreamRequest { kind, enable } => {
                self.audio.send(AudioCmd::Request { device, kind, enable }).await;
            }
            Message::AudioStreamStop { stream_id, .. } => {
                self.audio.send(AudioCmd::PspStop { device, stream_id }).await;
            }
            other => debug!(type_id = other.type_id(), "message not handled by this host version"),
        }
    }
}

pub async fn run(mut events: mpsc::Receiver<HostEvent>, mut features: Features) {
    while let Some(ev) = events.recv().await {
        match ev {
            HostEvent::SessionUp { device, session_id, name } => {
                info!(device = %name, "PSP connected");
                features.audio.send(AudioCmd::SessionUp { device, session_id }).await;
                if let Some(w) = &features.widgets {
                    w.send(HubCmd::SessionUp(device, session_id)).await;
                }
            }
            HostEvent::SessionDown { device, session_id } => {
                features.audio.send(AudioCmd::SessionDown { device, session_id }).await;
                if let Some(w) = &features.widgets {
                    w.send(HubCmd::SessionDown(device, session_id)).await;
                }
            }
            HostEvent::Message { device, msg } => features.on_message(device, msg).await,
        }
    }
}
