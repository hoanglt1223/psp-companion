//! Local HTTP endpoint (127.0.0.1 only) that feeds the notifications and Claude Code widgets.
//!
//! - `POST /notify` with `{"title": "...", "body": "...", "app": "..."}` adds a notification.
//! - `DELETE /notify` clears them.
//! - `POST /claude-code` takes a Claude Code hook's JSON input (pipe it with curl from a hook command).
//!
//! JSON content type is required, so a web page cannot post here without a CORS preflight, which is refused.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use tokio::net::TcpListener;
use tracing::{info, warn};

use super::WidgetHandle;
use super::model::{ClaudeSession, ClaudeState, MAX_CLAUDE_SESSIONS, MAX_NOTIFICATIONS, Notification, WidgetValue};

const SESSION_EXPIRY: Duration = Duration::from_secs(12 * 3600);

#[derive(Deserialize)]
struct NotifyBody {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    app: String,
}

/// The fields of Claude Code hook input this widget uses; the rest is ignored.
#[derive(Deserialize)]
pub struct HookInput {
    pub session_id: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

struct Tracked {
    session: ClaudeSession,
    updated: Instant,
}

#[derive(Default)]
struct Store {
    notifications: Vec<Notification>,
    claude: HashMap<String, Tracked>,
}

impl Store {
    fn claude_value(&mut self, now: Instant) -> WidgetValue {
        self.claude.retain(|_, t| now.duration_since(t.updated) < SESSION_EXPIRY);
        let mut list: Vec<&Tracked> = self.claude.values().collect();
        list.sort_by_key(|t| std::cmp::Reverse(t.updated));
        WidgetValue::ClaudeCode(list.into_iter().take(MAX_CLAUDE_SESSIONS).map(|t| t.session.clone()).collect())
    }
}

/// Maps one hook event to the session's new state; None removes the session.
pub fn apply_hook(prev: Option<&ClaudeSession>, h: &HookInput) -> Option<Option<ClaudeSession>> {
    let project = Path::new(&h.cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .or_else(|| prev.map(|p| p.project.clone()))
        .unwrap_or_else(|| "session".into());
    let (state, detail) = match h.hook_event_name.as_str() {
        "SessionStart" => (ClaudeState::Idle, "ready".to_string()),
        "UserPromptSubmit" => (ClaudeState::Working, "thinking".to_string()),
        "PreToolUse" => (ClaudeState::Working, h.tool_name.clone().unwrap_or_else(|| "tool".into())),
        "PostToolUse" | "SubagentStop" | "PreCompact" => {
            (ClaudeState::Working, prev.map(|p| p.detail.clone()).unwrap_or_else(|| "working".into()))
        }
        "Notification" => (ClaudeState::Waiting, h.message.clone().unwrap_or_else(|| "needs your input".into())),
        "Stop" => (ClaudeState::Done, "finished".to_string()),
        "SessionEnd" => return Some(None),
        _ => return None,
    };
    Some(Some(ClaudeSession { project, state, detail }))
}

#[derive(Clone)]
struct Ctx {
    handle: WidgetHandle,
    store: Arc<Mutex<Store>>,
    token: Arc<String>,
}

fn authorized(ctx: &Ctx, headers: &HeaderMap) -> bool {
    ctx.token.is_empty()
        || headers.get("x-companion-token").and_then(|v| v.to_str().ok()).is_some_and(|v| v == ctx.token.as_str())
}

async fn notify(State(ctx): State<Ctx>, headers: HeaderMap, Json(body): Json<NotifyBody>) -> StatusCode {
    if !authorized(&ctx, &headers) {
        return StatusCode::UNAUTHORIZED;
    }
    if body.title.trim().is_empty() {
        return StatusCode::UNPROCESSABLE_ENTITY;
    }
    let value = {
        let mut s = ctx.store.lock().unwrap_or_else(|e| e.into_inner());
        s.notifications.insert(
            0,
            Notification {
                app: body.app,
                title: body.title,
                body: body.body,
                time: chrono::Local::now().format("%H:%M").to_string(),
            },
        );
        s.notifications.truncate(MAX_NOTIFICATIONS);
        WidgetValue::Notifications(s.notifications.clone())
    };
    ctx.handle.update(value).await;
    StatusCode::NO_CONTENT
}

async fn clear(State(ctx): State<Ctx>, headers: HeaderMap) -> StatusCode {
    if !authorized(&ctx, &headers) {
        return StatusCode::UNAUTHORIZED;
    }
    ctx.store.lock().unwrap_or_else(|e| e.into_inner()).notifications.clear();
    ctx.handle.update(WidgetValue::Notifications(Vec::new())).await;
    StatusCode::NO_CONTENT
}

async fn claude_code(State(ctx): State<Ctx>, headers: HeaderMap, Json(hook): Json<HookInput>) -> StatusCode {
    if !authorized(&ctx, &headers) {
        return StatusCode::UNAUTHORIZED;
    }
    let value = {
        let mut s = ctx.store.lock().unwrap_or_else(|e| e.into_inner());
        let prev = s.claude.get(&hook.session_id).map(|t| &t.session);
        match apply_hook(prev, &hook) {
            None => return StatusCode::NO_CONTENT,
            Some(None) => {
                s.claude.remove(&hook.session_id);
            }
            Some(Some(session)) => {
                s.claude.insert(hook.session_id.clone(), Tracked { session, updated: Instant::now() });
            }
        }
        s.claude_value(Instant::now())
    };
    ctx.handle.update(value).await;
    StatusCode::NO_CONTENT
}

pub fn router(handle: WidgetHandle, token: String) -> Router {
    let ctx = Ctx { handle, store: Arc::default(), token: Arc::new(token) };
    Router::new()
        .route("/notify", post(notify).delete(clear))
        .route("/claude-code", post(claude_code))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(ctx)
}

pub async fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await
}

pub fn spawn(listener: TcpListener, handle: WidgetHandle, token: String) {
    if let Ok(addr) = listener.local_addr() {
        info!(%addr, "widget ingest endpoint listening");
    }
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router(handle, token)).await {
            warn!("widget ingest endpoint stopped: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(event: &str, tool: Option<&str>) -> HookInput {
        HookInput {
            session_id: "s1".into(),
            hook_event_name: event.into(),
            cwd: "D:\\Projects\\psp-companion".into(),
            tool_name: tool.map(String::from),
            message: None,
        }
    }

    #[test]
    fn hook_events_map_to_states() {
        let s = apply_hook(None, &hook("UserPromptSubmit", None)).unwrap().unwrap();
        assert_eq!((s.project.as_str(), s.state), ("psp-companion", ClaudeState::Working));
        let s = apply_hook(Some(&s), &hook("PreToolUse", Some("Bash"))).unwrap().unwrap();
        assert_eq!(s.detail, "Bash");
        let s2 = apply_hook(Some(&s), &hook("PostToolUse", None)).unwrap().unwrap();
        assert_eq!(s2.detail, "Bash");
        assert_eq!(apply_hook(Some(&s), &hook("Notification", None)).unwrap().unwrap().state, ClaudeState::Waiting);
        assert_eq!(apply_hook(Some(&s), &hook("Stop", None)).unwrap().unwrap().state, ClaudeState::Done);
        assert_eq!(apply_hook(Some(&s), &hook("SessionEnd", None)), Some(None));
        assert_eq!(apply_hook(Some(&s), &hook("SomethingNew", None)), None);
    }
}
