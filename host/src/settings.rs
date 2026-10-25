//! Local settings page at `http://127.0.0.1:<ingest_port>/settings`: speech-to-text, text-to-speech and Claude
//! model, PC speaker device, Stream Deck, widget sources, and the twelve macro slots; it can also open pairing and
//! show the PIN (for a host without a tray). It edits config.toml
//! through [`AppState::update_config`]; changes apply when the host restarts.
//!
//! The page can start programs (macro commands, whisper/Piper paths), so it is guarded: the listener is
//! loopback-only, the Host header must name the loopback address (no DNS rebinding), and writes need a
//! per-run token that only a same-origin page can read.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::{AudioConfig, WidgetsConfig};
use crate::macros::MacroSlot;
use crate::state::AppState;
use crate::streamdeck::satellite::StreamDeckConfig;
use crate::voice::{Secret, VoiceConfig};

pub const TOKEN_HEADER: &str = "x-companion-settings";
const PAGE: &str = include_str!("settings.html");

/// The parts of config.toml the page edits. Network settings and paired devices are left alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Editable {
    pub voice: VoiceConfig,
    pub audio: AudioConfig,
    pub streamdeck: StreamDeckConfig,
    pub widgets: WidgetsConfig,
    pub macros: Vec<MacroSlot>,
}

#[derive(Clone)]
struct Ctx {
    state: Arc<AppState>,
    token: Arc<String>,
    port: u16,
}

pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/settings")
}

pub fn router(state: Arc<AppState>, port: u16) -> Router {
    let mut token = [0u8; 16];
    getrandom::fill(&mut token).expect("OS random number generator");
    let ctx = Ctx { state, token: Arc::new(hex::encode(token)), port };
    Router::new()
        .route("/settings", get(page))
        .route("/settings/config", get(read).post(write))
        .route("/settings/pair", post(pair))
        .layer(middleware::from_fn_with_state(ctx.clone(), loopback_host_only))
        .with_state(ctx)
}

async fn loopback_host_only(State(ctx): State<Ctx>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let allowed = [format!("127.0.0.1:{}", ctx.port), format!("localhost:{}", ctx.port)];
    if !allowed.iter().any(|a| a.eq_ignore_ascii_case(host)) {
        return (StatusCode::FORBIDDEN, "open the settings page at 127.0.0.1").into_response();
    }
    next.run(req).await
}

async fn page(State(ctx): State<Ctx>) -> Response {
    let html = PAGE.replace("{{TOKEN}}", &ctx.token);
    ([(header::CACHE_CONTROL, "no-store")], Html(html)).into_response()
}

fn editable(ctx: &Ctx) -> Editable {
    let c = ctx.state.config();
    let mut voice = c.voice;
    // The API key is never shown; the page says to use ANTHROPIC_API_KEY or edit config.toml.
    voice.api_key = Secret::default();
    Editable { voice, audio: c.audio, streamdeck: c.streamdeck, widgets: c.widgets, macros: c.macros }
}

async fn read(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if !token_ok(&ctx, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut body = serde_json::to_value(editable(&ctx)).unwrap_or_default();
    body["api_key_set"] = serde_json::Value::Bool(!ctx.state.config().voice.api_key.0.is_empty());
    ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

#[derive(Serialize)]
struct WriteResult {
    ok: bool,
    message: String,
}

async fn write(State(ctx): State<Ctx>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if !token_ok(&ctx, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let fail = |message: String| (StatusCode::BAD_REQUEST, Json(WriteResult { ok: false, message })).into_response();
    let edit: Editable = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(e) => return fail(format!("invalid settings: {e}")),
    };
    let result = ctx.state.update_config(|c| {
        let key = std::mem::take(&mut c.voice.api_key);
        c.voice = edit.voice;
        c.voice.api_key = key;
        c.audio = edit.audio;
        c.streamdeck = edit.streamdeck;
        c.widgets = edit.widgets;
        c.macros = edit.macros;
    });
    match result {
        Ok(()) => {
            info!("settings saved from the settings page");
            Json(WriteResult { ok: true, message: "Saved. Restart companion-host to apply.".into() }).into_response()
        }
        Err(e) => fail(e.to_string()),
    }
}

/// Opens pairing and returns the PIN, so a host without a tray (Linux service) can pair without a restart.
async fn pair(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if !token_ok(&ctx, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let pin = ctx.state.open_pairing();
    ([(header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({ "pin": pin }))).into_response()
}

fn token_ok(ctx: &Ctx, headers: &HeaderMap) -> bool {
    let given = headers.get(TOKEN_HEADER).map(|v| v.as_bytes()).unwrap_or_default();
    given.len() == ctx.token.len() && given.iter().zip(ctx.token.as_bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// The token embedded in the page, for tests.
pub fn token_from_page(html: &str) -> Option<&str> {
    let start = html.find("const TOKEN = \"")? + "const TOKEN = \"".len();
    let len = html[start..].find('"')?;
    Some(&html[start..start + len])
}
