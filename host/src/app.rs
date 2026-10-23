//! Feature router: consumes network events and hands them to widgets, macros, audio, voice, and deck.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::macros::MacroRunner;
use crate::net::message::Message;
use crate::state::{AppState, HostEvent};
use crate::widgets::layout::Layout;
use crate::widgets::{self, HubCmd, WidgetHandle};

pub const INPUT_TAP: u8 = 0;

pub struct Features {
    pub layout: Arc<Layout>,
    pub widgets: Option<WidgetHandle>,
    pub macros: MacroRunner,
}

impl Features {
    /// Builds the hub and data sources from the config. Must run inside the tokio runtime.
    pub async fn start(
        state: &Arc<AppState>,
        config: &Config,
        layout: Layout,
        exec: Arc<dyn crate::actions::Executor>,
    ) -> Self {
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
        Self { layout, widgets, macros: MacroRunner::new(config.macros.clone(), exec) }
    }

    async fn on_message(&mut self, msg: Message) {
        match msg {
            Message::MacroTrigger { slot } => self.macros.trigger(slot).await,
            Message::InputEvent { kind: INPUT_TAP, widget_id, .. } => {
                match self.layout.widget(widget_id).and_then(|d| d.tap.clone().map(|a| (d.title.clone(), a))) {
                    Some((title, action)) => self.macros.run(&title, action).await,
                    None => debug!(widget_id, "tap on a widget without an action"),
                }
            }
            Message::InputEvent { .. } => {}
            other => debug!(type_id = other.type_id(), "message not handled by this host version"),
        }
    }
}

pub async fn run(mut events: mpsc::Receiver<HostEvent>, mut features: Features) {
    while let Some(ev) = events.recv().await {
        match ev {
            HostEvent::SessionUp { device, session_id, name } => {
                info!(device = %name, "PSP connected; sending widgets");
                if let Some(w) = &features.widgets {
                    w.send(HubCmd::SessionUp(device, session_id)).await;
                }
            }
            HostEvent::SessionDown { device, session_id } => {
                if let Some(w) = &features.widgets {
                    w.send(HubCmd::SessionDown(device, session_id)).await;
                }
            }
            HostEvent::Message { msg, .. } => features.on_message(msg).await,
        }
    }
}
