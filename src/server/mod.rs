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

use crate::engine::command::{Command, Reply};
use crate::engine::events::Waited;
use crate::engine::testimony::Staleness;
use crate::engine::Engine;

/// The engine, and who may ask it.
#[derive(Clone)]
pub struct Access {
    engine: Engine,
    /// Required on every request when set.
    token: Option<Arc<str>>,
}

impl Access {
    /// Served on loopback: no token asked.
    pub fn loopback(engine: Engine) -> Self {
        Self {
            engine,
            token: None,
        }
    }

    /// Served beyond this machine: `token` on every request.
    pub fn with_token(engine: Engine, token: String) -> Self {
        Self {
            engine,
            token: Some(Arc::from(token)),
        }
    }
}

/// Every route, behind the token check.
pub fn router(access: Access) -> Router {
    Router::new()
        .route("/v1/health", get(health))
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

/// Whether the radio is delivering, and which radio, where, doing what.
async fn health(State(access): State<Access>) -> Json<serde_json::Value> {
    Json(access.engine.with_state(|m| {
        serde_json::json!({
            "radio": &*m.system.board_name,
            "streaming": m.radio.hw_streaming,
            "stale": Staleness::NotStreaming.resolve(m),
            "frequency_hz": m.radio.frequency,
            "sample_rate_hz": m.radio.config_sample_rate,
            "view": m.mode.view,
        })
    }))
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
}
