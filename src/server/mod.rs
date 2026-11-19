// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The engine over the network: HTTP and JSON for commands and readings, a
//! WebSocket for events. A client of the engine like the terminal is, and
//! held by the same test to never reach for the screen.
//!
//! **Who may ask.** On loopback anyone on this machine may, as anyone on it
//! may already run sdrtop. Anywhere else a token is required on every
//! request (`Authorization: Bearer <token>`): a command retunes a radio
//! somebody may be relying on, which is not an anonymous operation.
//!
//! Everything under `/v1`, so a later change of shape is a new prefix, not a
//! surprise to every script written against this one.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use serde::Serialize;

use crate::config::EngineConfig;
use crate::engine::command::{Command, Reply};
use crate::engine::events::Waited;
use crate::engine::testimony::Staleness;
use crate::engine::Engine;
use crate::hardware::DeviceKind;

/// The engine, and who may ask it.
#[derive(Clone)]
pub struct Access {
    engine: Engine,
    /// Required on every request when set.
    token: Option<Arc<str>>,
    /// The radios found when the engine started. Not looked for again while
    /// one streams: some libraries open every radio to describe it.
    radios: Arc<Vec<Radio>>,
    /// The config as loaded, for the blocks the state does not hold.
    config: Arc<EngineConfig>,
}

/// One radio found, as the API lists it.
#[derive(Clone, Debug, Serialize)]
pub struct Radio {
    pub label: String,
    /// The backend, spelled as `--device` takes it.
    pub backend: String,
    pub serial: Option<String>,
    /// Written with this radio on the bench, rather than reasoned from a
    /// library's headers (POLICY rule 1).
    pub verified: bool,
    /// The one this engine runs.
    pub open: bool,
}

impl Radio {
    pub fn new(label: &str, kind: DeviceKind, serial: Option<String>, open: bool) -> Self {
        Self {
            label: label.to_string(),
            backend: format!("{kind:?}").to_lowercase(),
            serial,
            verified: kind.verified(),
            open,
        }
    }
}

impl Access {
    /// Served on loopback: no token asked.
    pub fn loopback(engine: Engine) -> Self {
        Self {
            engine,
            token: None,
            radios: Arc::default(),
            config: Arc::default(),
        }
    }

    /// Served beyond this machine: `token` on every request.
    pub fn with_token(engine: Engine, token: String) -> Self {
        Self {
            token: Some(Arc::from(token)),
            ..Self::loopback(engine)
        }
    }

    /// The radios found at start.
    pub fn with_radios(self, radios: Vec<Radio>) -> Self {
        Self {
            radios: Arc::new(radios),
            ..self
        }
    }

    /// The config as loaded.
    pub fn with_config(self, config: EngineConfig) -> Self {
        Self {
            config: Arc::new(config),
            ..self
        }
    }
}

/// Every route, behind the token check.
pub fn router(access: Access) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/radios", get(radios))
        .route("/v1/config", get(config))
        .route("/v1/command", post(command))
        .route("/v1/events", get(events))
        .layer(middleware::from_fn_with_state(
            access.clone(),
            require_token,
        ))
        .with_state(access)
}

/// Serve `router` on `listener` until `stop` completes.
pub async fn serve(
    listener: tokio::net::TcpListener,
    router: Router,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(stop)
        .await
}

async fn require_token(State(access): State<Access>, req: Request, next: Next) -> Response {
    if let Some(token) = &access.token {
        let presented = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if presented != Some(&**token) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    next.run(req).await
}

/// Whether the radio is delivering, and which radio, where, doing what;
/// the samples lost this session, split as the timing bench splits them;
/// each sample feed's own account; and what the process costs the machine.
async fn health(State(access): State<Access>) -> Json<serde_json::Value> {
    Json(access.engine.with_state(|m| {
        let feed = |refused, session| serde_json::json!({ "refused": refused, "refused_session": session });
        serde_json::json!({
            "radio": &*m.system.board_name,
            "streaming": m.radio.hw_streaming,
            "stale": Staleness::NotStreaming.resolve(m),
            "frequency_hz": m.radio.frequency,
            "sample_rate_hz": m.radio.config_sample_rate,
            "view": m.mode.view,
            "lost_samples": {
                "all": m.signal.total_drops_session,
                "driver": m.signal.driver_drops_session,
                "sdrtop": m.signal.total_drops_session.saturating_sub(m.signal.driver_drops_session),
            },
            "feeds": {
                "spectrum": feed(m.iq.fft_drops, m.iq.fft_drops_session),
                "net": feed(m.net.health.refused, m.net.health.refused_session),
                "pager": feed(m.pager.feed.refused, m.pager.feed.refused_session),
                "ft8": feed(m.ft8.feed.refused, m.ft8.feed.refused_session),
            },
            "process": {
                "cpu_pct": m.system.process_cpu_pct,
                "rss_mb": m.system.process_rss_mb,
            },
        })
    }))
}

/// The radios found at start, verified or reasoned, and which one runs.
async fn radios(State(access): State<Access>) -> Json<Vec<Radio>> {
    Json(access.radios.as_ref().clone())
}

/// The engine's blocks of the config as they would be saved now. Only the
/// engine's: the server's own block, with its token, is not among them, so
/// there is nothing to withhold.
async fn config(State(access): State<Access>) -> Response {
    let mut cfg = access.config.as_ref().clone();
    cfg.radio = access
        .engine
        .with_state(|m| crate::engine::boot::radio_block(m, &cfg.radio));
    let Ok(all) = toml::Table::try_from(&cfg) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let blocks: serde_json::Map<String, serde_json::Value> = all
        .into_iter()
        .filter_map(|(block, value)| Some((block, serde_json::to_value(value).ok()?)))
        .collect();
    Json(blocks).into_response()
}

/// One command, answered: 200 when applied, 422 with the reason when not.
async fn command(State(access): State<Access>, Json(command): Json<Command>) -> Response {
    // The command makes a device call, which blocks on USB.
    let engine = access.engine.clone();
    let reply = match tokio::task::spawn_blocking(move || engine.apply(command)).await {
        Ok(reply) => reply,
        Err(e) => Reply::Refused {
            reason: format!("the command did not finish: {e}"),
        },
    };
    let status = match reply {
        Reply::Applied { .. } => StatusCode::OK,
        Reply::Refused { .. } => StatusCode::UNPROCESSABLE_ENTITY,
    };
    (status, Json(reply)).into_response()
}

/// The events, as they happen, one JSON message each.
async fn events(State(access): State<Access>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| forward(socket, access.engine))
}

/// How many events a client may fall behind by before it starts losing them.
const CLIENT_QUEUE: usize = 256;

async fn forward(mut socket: WebSocket, engine: Engine) {
    let subscription = engine.subscribe(CLIENT_QUEUE);
    let (tx, mut rx) = tokio::sync::mpsc::channel(CLIENT_QUEUE);
    // The bus is read where waiting is allowed to block; the socket is
    // written here. When the socket goes, the reader notices within a second
    // and drops the subscription.
    tokio::task::spawn_blocking(move || loop {
        if tx.is_closed() {
            break;
        }
        match subscription.wait(Duration::from_secs(1)) {
            Waited::Event(delivered) => {
                if tx.blocking_send(delivered).is_err() {
                    break;
                }
            }
            Waited::Quiet => {}
            Waited::Gone => break,
        }
    });
    loop {
        tokio::select! {
            delivered = rx.recv() => {
                let Some(delivered) = delivered else { break };
                let Ok(text) = serde_json::to_string(&delivered) else { continue };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testing::{engine_on, Bench};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn ask(app: axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    fn get(path: &str) -> Request<Body> {
        Request::get(path).body(Body::empty()).unwrap()
    }

    fn post(path: &str, json: &str) -> Request<Body> {
        Request::post(path)
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .unwrap()
    }

    /// Health answers whatever the radio is doing: a radio that is not
    /// streaming is a state to report, not an error.
    #[tokio::test]
    async fn health_answers_without_a_radio_streaming() {
        let (engine, _) = engine_on(Bench::new());
        let (status, json) = ask(router(Access::loopback(engine)), get("/v1/health")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["streaming"], false);
        assert_eq!(json["stale"], true);
    }

    #[tokio::test]
    async fn a_command_comes_back_with_its_reply() {
        let (engine, _) = engine_on(Bench::new());
        let app = router(Access::loopback(engine));
        let (status, json) = ask(
            app.clone(),
            post("/v1/command", r#"{"command":"tune","hz":433920000}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["reply"], "applied");

        let (status, json) = ask(app, post("/v1/command", r#"{"command":"tune","hz":1}"#)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["reply"], "refused");
        assert!(json["reason"].as_str().unwrap().contains("not a frequency"));
    }

    /// Off loopback, every route wants the token; the wrong one is as good
    /// as none.
    #[tokio::test]
    async fn off_loopback_a_missing_token_is_401() {
        let (engine, _) = engine_on(Bench::new());
        let app = router(Access::with_token(engine, "s3cret".into()));
        let (status, _) = ask(app.clone(), get("/v1/health")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let wrong = Request::get("/v1/health")
            .header("authorization", "Bearer nope")
            .body(Body::empty())
            .unwrap();
        assert_eq!(ask(app.clone(), wrong).await.0, StatusCode::UNAUTHORIZED);
        let right = Request::get("/v1/health")
            .header("authorization", "Bearer s3cret")
            .body(Body::empty())
            .unwrap();
        assert_eq!(ask(app, right).await.0, StatusCode::OK);
    }

    /// The engine the API commands is the one the terminal draws: a script's
    /// retune is in the next frame.
    #[tokio::test]
    async fn a_command_from_the_api_is_in_the_next_tui_snapshot() {
        let (engine, device) = engine_on(Bench::new());
        let app = router(Access::loopback(engine.clone()));
        ask(
            app,
            post("/v1/command", r#"{"command":"tune","hz":145500000}"#),
        )
        .await;
        assert_eq!(engine.snapshot().radio.frequency, 145_500_000);
        assert_eq!(*device.tuned.lock().unwrap(), vec![145_500_000]);
    }

    /// Something that is not a command is refused before the engine sees it.
    #[tokio::test]
    async fn a_body_that_is_not_a_command_is_400() {
        let (engine, device) = engine_on(Bench::new());
        let app = router(Access::loopback(engine));
        let (status, _) = ask(app, post("/v1/command", r#"{"command":"explode"}"#)).await;
        assert!(status.is_client_error(), "{status}");
        assert!(device.tuned.lock().unwrap().is_empty());
    }

    /// Rule 1 over the API: a radio reached through SoapySDR is said to be
    /// reasoned from its headers, every other one verified on the bench, and
    /// the one this engine runs is marked.
    #[tokio::test]
    async fn radios_lists_each_with_verified_or_reasoned() {
        let (engine, _) = engine_on(Bench::new());
        let radios = vec![
            Radio::new(
                "HackRF One",
                crate::hardware::DeviceKind::HackRf,
                Some("ab12".into()),
                true,
            ),
            Radio::new(
                "LimeSDR Mini",
                crate::hardware::DeviceKind::Soapy,
                None,
                false,
            ),
        ];
        let app = router(Access::loopback(engine).with_radios(radios));
        let (status, json) = ask(app, get("/v1/radios")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json[0]["label"], "HackRF One");
        assert_eq!(json[0]["backend"], "hackrf");
        assert_eq!(json[0]["verified"], true);
        assert_eq!(json[0]["open"], true);
        assert_eq!(json[1]["verified"], false);
        assert_eq!(json[1]["open"], false);
    }

    /// The loss the timing bench splits, split the same way, and each feed's
    /// own account beside it.
    #[tokio::test]
    async fn health_splits_loss_by_driver_and_sdrtop() {
        let (engine, _) = engine_on(Bench::new());
        engine.with_state(|m| {
            m.signal.total_drops_session = 1000;
            m.signal.driver_drops_session = 400;
            m.net.health.refused_session = 3;
        });
        let (_, json) = ask(router(Access::loopback(engine)), get("/v1/health")).await;
        assert_eq!(json["lost_samples"]["all"], 1000);
        assert_eq!(json["lost_samples"]["driver"], 400);
        assert_eq!(json["lost_samples"]["sdrtop"], 600);
        assert_eq!(json["feeds"]["net"]["refused_session"], 3);
        assert!(json["process"]["rss_mb"].is_number());
    }

    /// The engine's blocks, as they would be saved now, and no other.
    #[tokio::test]
    async fn config_is_the_engines_blocks() {
        let (engine, _) = engine_on(Bench::new());
        engine.with_state(|m| m.radio.frequency = 433_920_000);
        let mut cfg = crate::config::EngineConfig::default();
        cfg.net.bt_channels = 5;
        let app = router(Access::loopback(engine).with_config(cfg));
        let (status, json) = ask(app, get("/v1/config")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["radio"]["frequency_hz"], 433_920_000);
        assert_eq!(json["net"]["bt_channels"], 5);
        let mut blocks: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        blocks.retain(|b| !crate::config::ENGINE_BLOCKS.contains(b));
        assert!(blocks.is_empty(), "not the engine's: {blocks:?}");
    }
}
