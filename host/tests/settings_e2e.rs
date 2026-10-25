//! The local settings page over HTTP: loopback Host check, per-run token, validation, and what it may change.

mod common;

use common::*;
use companion_host::actions::{Action, MediaKey};
use companion_host::settings::{self, TOKEN_HEADER};
use companion_host::voice::Secret;
use companion_host::widgets;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn raw_get(addr: SocketAddr, host: &str, path: &str) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    timeout(T, s.read_to_string(&mut out)).await.unwrap().unwrap();
    out
}

#[tokio::test]
async fn settings_page_edits_only_its_sections_and_guards_writes() {
    let host = start_host("settings").await;
    let device: Id16 = random_array();
    host.state.update_config(|c| c.voice.api_key = Secret("sk-test-secret".into())).unwrap();

    let listener = widgets::ingest::bind(0).await.unwrap();
    let addr = listener.local_addr().unwrap();
    widgets::ingest::serve(listener, settings::router(host.state.clone(), addr.port()));

    // DNS rebinding: a page on another name that resolves to 127.0.0.1 is refused.
    let refused = raw_get(addr, "evil.example:80", "/settings").await;
    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");

    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", addr.port());
    let page = http.get(format!("{base}/settings")).send().await.unwrap().text().await.unwrap();
    let token = settings::token_from_page(&page).expect("token in page").to_string();
    assert_eq!(token.len(), 32);

    let no_token = http.get(format!("{base}/settings/config")).send().await.unwrap();
    assert_eq!(no_token.status(), 403);

    // Pairing from the page (a host without a tray): the PIN it shows pairs a PSP.
    let refused = http.post(format!("{base}/settings/pair")).send().await.unwrap();
    assert_eq!(refused.status(), 403);
    let opened: Value = http
        .post(format!("{base}/settings/pair"))
        .header(TOKEN_HEADER, &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pin = opened["pin"].as_str().expect("pin").to_string();
    assert_eq!(pin.len(), 6);
    pair(&host, &pin, device).await.unwrap();

    let mut cfg: Value = http
        .get(format!("{base}/settings/config"))
        .header(TOKEN_HEADER, &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg["voice"]["api_key"], "", "the API key is never sent to the page");
    assert_eq!(cfg["api_key_set"], true);
    cfg.as_object_mut().unwrap().remove("api_key_set");

    // Forged write without the token.
    let forged = http.post(format!("{base}/settings/config")).json(&cfg).send().await.unwrap();
    assert_eq!(forged.status(), 403);

    // An invalid value is refused and nothing is written.
    let mut bad = cfg.clone();
    bad["streamdeck"]["bitmap_size"] = json!(5);
    let r = http.post(format!("{base}/settings/config")).header(TOKEN_HEADER, &token).json(&bad).send().await.unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(host.state.config().streamdeck.bitmap_size, 76);

    cfg["voice"]["model"] = json!("claude-haiku-5-5");
    cfg["voice"]["tts"]["engine"] = json!("none");
    cfg["macros"] = json!([{ "slot": 3, "label": "Skip", "action": { "type": "media", "key": "next" } }]);
    let r = http.post(format!("{base}/settings/config")).header(TOKEN_HEADER, &token).json(&cfg).send().await.unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());

    let saved = Config::load_or_create(&host.dir.join("config.toml")).unwrap();
    assert_eq!(saved.voice.model, "claude-haiku-5-5");
    assert_eq!(saved.voice.tts.engine, companion_host::voice::TtsEngine::None);
    assert_eq!(saved.macros.len(), 1);
    assert_eq!(saved.macros[0].action, Action::Media { key: MediaKey::Next });
    assert_eq!(saved.voice.api_key, Secret("sk-test-secret".into()), "the stored key survives a save");
    assert!(saved.device(&device).is_some(), "paired devices are untouched");
    assert_eq!(saved, host.state.config());
}
