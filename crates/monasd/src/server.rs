// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The engine over the network: HTTP and JSON for commands and readings, a
//! WebSocket for events. A client of the engine like the terminal is, in a
//! crate that cannot reach for the screen: monasd does not depend on sdrtop.
//!
//! **Who may ask.** On loopback anyone on this machine may, as anyone on it
//! may already run sdrtop or monasd. Anywhere else a token is required on every
//! request (`Authorization: Bearer <token>`): a command retunes a radio
//! somebody may be relying on, which is not an anonymous operation.
//!
//! Everything under `/v1`, so a later change of shape is a new prefix, not a
//! surprise to every script written against this one.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use serde::{Deserialize, Serialize};

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
    /// The recorder and its folder, once the stream's tap exists.
    recordings: Option<Arc<crate::record::Recordings>>,
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
            recordings: None,
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

    /// The recorder, writing into its folder.
    pub fn with_recordings(self, recordings: Arc<crate::record::Recordings>) -> Self {
        Self {
            recordings: Some(recordings),
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
        .route("/v1/snapshot", get(snapshot))
        .route("/v1/provenance", get(provenance))
        .route("/v1/spectrum", get(spectrum))
        .route("/v1/pager", get(pager))
        .route("/v1/ft8", get(ft8))
        .route("/v1/net/band", get(net_band))
        .route("/v1/net/census", get(net_census))
        .route("/v1/net/ble", get(net_ble))
        .route("/v1/net/coded", get(net_coded))
        .route("/v1/net/bt", get(net_bt))
        .route("/v1/net/fer", get(net_fer))
        .route("/v1/net/wifi/survey", get(wifi_survey))
        .route("/v1/net/wifi/frames", get(wifi_frames))
        .route("/v1/net/wifi/airtime", get(wifi_airtime))
        .route("/v1/recording", post(recording_start))
        .route("/v1/recording/stop", post(recording_stop))
        .route("/v1/recordings", get(recordings))
        .route("/v1/recordings/{name}", get(recording_meta))
        .route("/v1/export", post(export))
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

/// How the radio and the process are doing.
#[derive(Clone, Debug, Serialize)]
pub struct Health {
    /// The radio's name.
    pub radio: String,
    pub streaming: bool,
    /// Whether its readings have stopped being live.
    pub stale: bool,
    pub frequency_hz: u64,
    pub sample_rate_hz: f64,
    /// The view whose work the engine does.
    pub view: String,
    /// Samples lost this session, and whose loss each was.
    pub lost_samples: LostSamples,
    /// Each sample feed's refusals: blocks it had no room for.
    pub feeds: Feeds,
    pub process: Process,
}

/// Samples lost this session: by the driver before the engine saw them, or
/// by the engine's own intake.
#[derive(Clone, Debug, Serialize)]
pub struct LostSamples {
    pub all: u64,
    pub driver: u64,
    pub monas: u64,
}

/// The sample feeds, each one's refusals.
#[derive(Clone, Debug, Serialize)]
pub struct Feeds {
    pub spectrum: Feed,
    pub net: Feed,
    pub pager: Feed,
    pub ft8: Feed,
}

/// One feed's refusals, lately and over the session.
#[derive(Clone, Debug, Serialize)]
pub struct Feed {
    pub refused: u64,
    pub refused_session: u64,
}

/// What the process costs the machine.
#[derive(Clone, Debug, Serialize)]
pub struct Process {
    pub cpu_pct: f32,
    pub rss_mb: u64,
}

/// Whether the radio is delivering, and which radio, where, doing what; the
/// samples lost this session, split as the timing bench splits them; each
/// sample feed's own account; and what the process costs the machine.
async fn health(State(access): State<Access>) -> Json<Health> {
    Json(access.engine.with_state(|m| {
        let feed = |refused, refused_session| Feed {
            refused,
            refused_session,
        };
        Health {
            radio: m.system.board_name.to_string(),
            streaming: m.radio.hw_streaming,
            stale: Staleness::NotStreaming.resolve(m),
            frequency_hz: m.radio.frequency,
            sample_rate_hz: m.radio.config_sample_rate,
            view: m.mode.view.clone(),
            lost_samples: LostSamples {
                all: m.signal.total_drops_session,
                driver: m.signal.driver_drops_session,
                monas: m
                    .signal
                    .total_drops_session
                    .saturating_sub(m.signal.driver_drops_session),
            },
            feeds: Feeds {
                spectrum: feed(m.iq.fft_drops, m.iq.fft_drops_session),
                net: feed(m.net.health.refused, m.net.health.refused_session),
                pager: feed(m.pager.feed.refused, m.pager.feed.refused_session),
                ft8: feed(m.ft8.feed.refused, m.ft8.feed.refused_session),
            },
            process: Process {
                cpu_pct: m.system.process_cpu_pct,
                rss_mb: m.system.process_rss_mb,
            },
        }
    }))
}

/// Everything the engine reads: each reading a value or the reason it has
/// none, each group with its testimony. Built from a copy of the state, so
/// the engine's lock is held only for the copy.
async fn snapshot(State(access): State<Access>) -> Json<crate::engine::view::Snapshot> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::snapshot(
        &state,
        crate::engine::view::Clock::now(),
    ))
}

/// The latest spectrum frame with its testimony, or why there is none.
/// Its own route, so a client that wants the figures is not sent thousands
/// of bins with every snapshot.
async fn spectrum(
    State(access): State<Access>,
) -> Json<
    crate::engine::view::Group<crate::engine::view::Reading<crate::engine::view::spectrum::Frame>>,
> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::spectrum::spectrum(
        &state,
        crate::engine::view::Clock::now(),
    ))
}

/// `?since=N`: only what was numbered after `N`, so a client polling a list
/// is sent each entry once.
#[derive(Deserialize)]
struct Since {
    since: Option<u64>,
}

/// The pages the pager shows, as it shows them: masked, only the kind and
/// length; answering one address, only that address's.
async fn pager(
    State(access): State<Access>,
    Query(q): Query<Since>,
) -> Json<crate::engine::view::Group<crate::engine::view::pager::Pages>> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::pager::pages(
        &state,
        crate::engine::view::Clock::now(),
        q.since,
    ))
}

/// The FT8 decodes held, each with what it measured beside what it said:
/// the CSV export's account, as JSON.
async fn ft8(
    State(access): State<Access>,
    Query(q): Query<Since>,
) -> Json<crate::engine::view::Group<crate::engine::view::ft8::Decodes>> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::ft8::decodes(&state, q.since))
}

/// The 2.4 GHz band's occupancy, one megahertz to a cell: the occupancy
/// export's account, unobserved cells unobserved rather than zero.
async fn net_band(
    State(access): State<Access>,
) -> Json<crate::engine::view::Group<crate::engine::view::Reading<crate::engine::view::net::Band>>>
{
    let state = access.engine.snapshot();
    Json(crate::engine::view::net::band(
        &state,
        crate::engine::view::Clock::now(),
    ))
}

/// The census of transmitters, in the panel's order, addresses shown as
/// the panel shows them: the census export's account.
async fn net_census(
    State(access): State<Access>,
) -> Json<crate::engine::view::Group<crate::engine::view::net::Census>> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::net::census(
        &state,
        crate::engine::view::Clock::now(),
    ))
}

/// The LE 1M and 2M packet list as it is showing, newest first, each with
/// what was measured of it: the BLE export's account. `?since=N` sends only
/// packets numbered after `N`.
async fn net_ble(
    State(access): State<Access>,
    Query(q): Query<Since>,
) -> Json<crate::engine::view::Group<crate::engine::view::ble::Packets>> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::ble::ble(
        &state,
        crate::engine::view::Clock::now(),
        q.since,
    ))
}

/// The LE Coded packet list, the same way.
async fn net_coded(
    State(access): State<Access>,
    Query(q): Query<Since>,
) -> Json<crate::engine::view::Group<crate::engine::view::ble::Packets>> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::ble::coded(
        &state,
        crate::engine::view::Clock::now(),
        q.since,
    ))
}

/// The classic Bluetooth hits kept, oldest first: the BR/EDR export's
/// account.
async fn net_bt(
    State(access): State<Access>,
) -> Json<crate::engine::view::Group<crate::engine::view::bt::Hits>> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::bt::hits(
        &state,
        crate::engine::view::Clock::now(),
    ))
}

/// The frame error rate against SNR, all traffic and each device: the FER
/// export's account.
async fn net_fer(
    State(access): State<Access>,
) -> Json<crate::engine::view::Group<crate::engine::view::bt::Fer>> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::bt::fer(
        &state,
        crate::engine::view::Clock::now(),
    ))
}

/// The Wi-Fi survey: each channel's busy share over its visits, its
/// airtime by family, and which channel is least busy or why none can be
/// named yet.
async fn wifi_survey(
    State(access): State<Access>,
) -> Json<crate::engine::view::Group<crate::engine::view::wifi::Survey>> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::wifi::survey(
        &state,
        crate::engine::view::Clock::now(),
    ))
}

/// The Wi-Fi frames heard on the radio's centre, newest first, worded as
/// the Frames list words them. `?since=N` sends only frames after `N`.
async fn wifi_frames(
    State(access): State<Access>,
    Query(q): Query<Since>,
) -> Json<crate::engine::view::Group<crate::engine::view::wifi::Frames>> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::wifi::frames(
        &state,
        crate::engine::view::Clock::now(),
        q.since,
    ))
}

/// The locked channel's airtime: its share, who spent it, and the rhythm
/// of the gaps.
async fn wifi_airtime(
    State(access): State<Access>,
) -> Json<crate::engine::view::Group<crate::engine::view::wifi::Airtime>> {
    let state = access.engine.snapshot();
    Json(crate::engine::view::wifi::airtime(
        &state,
        crate::engine::view::Clock::now(),
    ))
}

/// The reply when there is no recorder to use.
fn no_recorder() -> Response {
    answered(Reply::Refused {
        reason: "nothing to record from: the stream has no recorder".to_string(),
    })
}

/// A reply as a command's is answered: 200 when done, 422 with the reason
/// when not.
fn answered(reply: Reply) -> Response {
    let status = match reply {
        Reply::Applied { .. } => StatusCode::OK,
        Reply::Refused { .. } => StatusCode::UNPROCESSABLE_ENTITY,
    };
    (status, Json(reply)).into_response()
}

/// Start a SigMF recording into the recordings folder, named by the time,
/// with `{"seconds": .., "gigabytes": ..}` in place of the config's limits
/// where given: answered as a command is.
async fn recording_start(
    State(access): State<Access>,
    ask: Option<Json<crate::record::Ask>>,
) -> Response {
    let Some(r) = access.recordings.clone() else {
        return no_recorder();
    };
    let ask = ask.map(|Json(a)| a).unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    // The recorder checks the disk and creates files.
    match tokio::task::spawn_blocking(move || r.start(ask, now)).await {
        Ok(reply) => answered(reply),
        Err(e) => answered(Reply::Refused {
            reason: format!("the recorder did not start: {e}"),
        }),
    }
}

/// Stop the recording running, keeping every block already captured:
/// answered as a command is.
async fn recording_stop(State(access): State<Access>) -> Response {
    match &access.recordings {
        Some(r) => answered(r.stop()),
        None => no_recorder(),
    }
}

/// The recordings in the recordings folder, oldest first. Their data stays
/// on this machine; their metadata is one route further.
async fn recordings(State(access): State<Access>) -> Json<crate::record::Listing> {
    Json(match &access.recordings {
        Some(r) => r.list(),
        None => crate::record::Listing { recordings: vec![] },
    })
}

/// One recording's `.sigmf-meta`, by the name the listing gives it: what it
/// holds, where its samples are missing and why, and how it ended.
async fn recording_meta(
    State(access): State<Access>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    let Some(r) = access.recordings.clone() else {
        return no_recorder();
    };
    match r.meta(&name) {
        Ok(meta) => Json(meta).into_response(),
        Err(why) => (StatusCode::NOT_FOUND, Json(Reply::Refused { reason: why })).into_response(),
    }
}

/// Write the CSV export of what the engine computes, as the terminal's
/// export key does: NET's six files in a NET mode, the FT8 decodes in an FT8
/// mode, into the folder the recordings go to. Answered as a command is,
/// each file named with its rows.
async fn export(State(access): State<Access>) -> Response {
    let Some(r) = access.recordings.clone() else {
        return no_recorder();
    };
    let state = access.engine.snapshot();
    let written = tokio::task::spawn_blocking(move || exported(&state, &r)).await;
    answered(written.unwrap_or_else(|e| Reply::Refused {
        reason: format!("the export did not finish: {e}"),
    }))
}

/// The export of `state` into `r`'s folder, as a reply.
fn exported(state: &crate::state::SdrMetrics, r: &crate::record::Recordings) -> Reply {
    let feeds = state.mode.feeds;
    if !feeds.net && !feeds.ft8 {
        return Reply::Refused {
            reason: format!(
                "nothing to export in the {} mode: the exports are the 2.4 GHz views' and FT8's",
                state.mode.view
            ),
        };
    }
    if let Err(e) = std::fs::create_dir_all(r.folder()) {
        return Reply::Refused {
            reason: format!("cannot use the data folder: {e}"),
        };
    }
    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let results = if feeds.ft8 {
        vec![crate::export::ft8_section(
            state,
            r.folder(),
            unix_secs,
            r.program(),
        )]
    } else {
        crate::export::net_section(state, r.folder(), unix_secs, r.program())
    };
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for result in results {
        match result {
            Ok(w) => {
                let name = w
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                done.push(match (w.rows, &w.note) {
                    (0, Some(note)) => format!("{name} (nothing to write: {note})"),
                    (n, _) => format!("{name} ({n} rows)"),
                });
            }
            Err(why) => failed.push(why),
        }
    }
    if done.is_empty() {
        return Reply::Refused {
            reason: failed.join("; "),
        };
    }
    let mut detail = format!("wrote {}", done.join(", "));
    if !failed.is_empty() {
        detail.push_str(&format!("; not written: {}", failed.join("; ")));
    }
    Reply::Applied { detail }
}

/// Where the measurements come from: the same fields an export's header
/// carries, by the same names.
async fn provenance(
    State(access): State<Access>,
) -> Json<serde_json::Map<String, serde_json::Value>> {
    let state = access.engine.snapshot();
    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let mut fields = serde_json::Map::new();
    fields.insert("monas".into(), env!("CARGO_PKG_VERSION").into());
    for (name, value) in crate::export::provenance::fields(&state, unix_secs) {
        fields.insert(name.into(), value.into());
    }
    Json(fields)
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

    /// The snapshot says what it cannot: a radio not streaming, with no
    /// frame yet, gives its groups stale and its figures absent with a
    /// reason, never a zero.
    #[tokio::test]
    async fn the_snapshot_says_what_it_cannot_read() {
        let (engine, _) = engine_on(Bench::new());
        let (status, json) = ask(router(Access::loopback(engine)), get("/v1/snapshot")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["radio"]["testimony"]["stale"], true);
        assert_eq!(json["signal"]["testimony"]["stale"], true);
        assert!(
            json["signal"]["peak_to_nf_db"]["absent"].is_string(),
            "{json}"
        );
        assert!(json["radio"]["frequency_hz"].is_u64(), "{json}");
        assert!(json["at_unix_ms"].as_u64().unwrap() > 1_700_000_000_000);
        for group in [
            "radio", "signal", "iq", "vitals", "record", "pager", "ft8", "demod",
        ] {
            assert!(json[group]["testimony"].is_object(), "no {group}: {json}");
        }
    }

    /// With no frame yet, the spectrum route says so rather than sending an
    /// empty array a client could read as a silent band.
    #[tokio::test]
    async fn the_spectrum_says_when_there_is_no_frame() {
        let (engine, _) = engine_on(Bench::new());
        let (status, json) = ask(router(Access::loopback(engine)), get("/v1/spectrum")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["absent"], "no spectrum frame yet", "{json}");
        assert_eq!(json["testimony"]["stale"], true);
    }

    /// The lists answer with nothing held as an empty list and the reason,
    /// and take `since`.
    #[tokio::test]
    async fn the_decode_lists_answer_and_take_since() {
        let (engine, _) = engine_on(Bench::new());
        let app = router(Access::loopback(engine));
        let (status, json) = ask(app.clone(), get("/v1/pager?since=3")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            json["pages"].as_array().is_some_and(Vec::is_empty),
            "{json}"
        );
        let (status, json) = ask(app, get("/v1/ft8")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["note"], "nothing has been decoded this session",
            "{json}"
        );
    }

    /// Outside the survey the band says where it is measured; the census
    /// answers with the transmitters it holds, none yet.
    #[tokio::test]
    async fn the_band_and_the_census_answer() {
        let (engine, _) = engine_on(Bench::new());
        let app = router(Access::loopback(engine));
        let (status, json) = ask(app.clone(), get("/v1/net/band")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            json["absent"]
                .as_str()
                .is_some_and(|w| w.contains("Survey")),
            "{json}"
        );
        let (status, json) = ask(app, get("/v1/net/census")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            json["devices"].as_array().is_some_and(Vec::is_empty),
            "{json}"
        );
        assert!(json["testimony"].is_object(), "{json}");
    }

    /// The packet lists, the hits and the error curve answer with nothing
    /// heard as empty lists and their notes.
    #[tokio::test]
    async fn the_packet_lists_answer_empty_with_their_notes() {
        let (engine, _) = engine_on(Bench::new());
        let app = router(Access::loopback(engine));
        for (route, list) in [
            ("/v1/net/ble?since=0", "packets"),
            ("/v1/net/coded", "packets"),
            ("/v1/net/bt", "hits"),
            ("/v1/net/fer", "bins"),
            ("/v1/net/wifi/survey", "channels"),
            ("/v1/net/wifi/frames?since=0", "frames"),
        ] {
            let (status, json) = ask(app.clone(), get(route)).await;
            assert_eq!(status, StatusCode::OK, "{route}");
            // The survey always lists its thirteen channels, each saying it
            // was not visited; every other list is empty.
            let empty =
                route.ends_with("survey") || json[list].as_array().is_some_and(Vec::is_empty);
            assert!(json[list].is_array() && empty, "{route}: {json}");
            assert!(json["testimony"].is_object(), "{route}: {json}");
        }
        let (_, air) = ask(app.clone(), get("/v1/net/wifi/airtime")).await;
        assert_eq!(
            air["share"]["absent"], "not locked on a Wi-Fi channel",
            "{air}"
        );
        let (_, coded) = ask(app, get("/v1/net/coded")).await;
        assert!(
            coded["note"]
                .as_str()
                .is_some_and(|n| n.contains("LE Coded")),
            "{coded}"
        );
    }

    /// A recording over the API: started, listed, its metadata served by
    /// its name, stopped; a name the folder does not hold is a 404.
    #[tokio::test]
    async fn a_recording_runs_through_the_api() {
        let dir = std::env::temp_dir().join(format!("monasd-api-rec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = crate::state::SdrMetrics::fixture().streaming();
        m.radio.config_sample_rate = 1e6;
        let state = Arc::new(std::sync::Mutex::new(m));
        let engine = Engine::new(Arc::clone(&state), Arc::new(Bench::new()));
        let rec = Arc::new(crate::record::Recordings::new(
            Arc::clone(&state),
            Arc::new(crate::hardware::RecordTap::default()),
            dir.clone(),
            crate::config::RecordSettings::default(),
            "monasd test".into(),
        ));
        let app = router(Access::loopback(engine).with_recordings(Arc::clone(&rec)));
        let (status, json) = ask(app.clone(), post("/v1/recording", r#"{"seconds": 2}"#)).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        let (_, list) = ask(app.clone(), get("/v1/recordings")).await;
        let name = list["recordings"][0]["name"].as_str().unwrap().to_string();
        assert_eq!(list["recordings"][0]["running"], true, "{list}");
        let (status, meta) = ask(app.clone(), get(&format!("/v1/recordings/{name}"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(meta["global"]["core:recorder"], "monasd test");
        let (status, _) = ask(app.clone(), get("/v1/recordings/..%2Fsecret")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = ask(app.clone(), post("/v1/recording/stop", "{}")).await;
        assert_eq!(status, StatusCode::OK);
        rec.finish();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An export writes the mode's files into the data folder, each named
    /// with its rows and headed by monasd's name; a mode with nothing to
    /// export is refused with why.
    #[tokio::test]
    async fn an_export_writes_the_modes_files() {
        let dir = std::env::temp_dir().join(format!("monasd-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = Arc::new(std::sync::Mutex::new(crate::state::SdrMetrics::fixture()));
        let engine = Engine::new(Arc::clone(&state), Arc::new(Bench::new()));
        let rec = Arc::new(crate::record::Recordings::new(
            Arc::clone(&state),
            Arc::new(crate::hardware::RecordTap::default()),
            dir.clone(),
            crate::config::RecordSettings::default(),
            "monasd test".into(),
        ));
        let app = router(Access::loopback(engine.clone()).with_recordings(rec));
        let (status, json) = ask(app.clone(), post("/v1/export", "{}")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{json}");
        assert!(
            json["reason"]
                .as_str()
                .unwrap()
                .contains("nothing to export"),
            "{json}"
        );

        let net = crate::engine::mode::Mode::named("net_survey").unwrap();
        engine.apply(crate::engine::command::Command::SetMode {
            section: net.section,
            view: net.view,
        });
        let (status, json) = ask(app, post("/v1/export", "{}")).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        let detail = json["detail"].as_str().unwrap();
        assert!(
            detail.contains("net-band") && detail.contains("net-census"),
            "{detail}"
        );
        let band = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().starts_with("net-band"))
            .expect("the band's file");
        let text = std::fs::read_to_string(band.path()).unwrap();
        assert!(text.starts_with("# monasd test\n"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The provenance is the export header's fields, as JSON.
    #[tokio::test]
    async fn the_provenance_is_the_export_headers_fields() {
        let (engine, _) = engine_on(Bench::new());
        let (status, json) = ask(router(Access::loopback(engine)), get("/v1/provenance")).await;
        assert_eq!(status, StatusCode::OK);
        for field in [
            "monas",
            "exported",
            "device",
            "tuning",
            "mode",
            "reference",
            "session",
        ] {
            assert!(json[field].is_string(), "no {field}: {json}");
        }
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
        assert_eq!(json["lost_samples"]["monas"], 600);
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
