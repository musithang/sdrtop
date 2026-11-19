// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! sdrtop with no terminal: the engine, a radio, the API, and nothing
//! drawing. The terminal's counterpart: a front end of the engine, not part
//! of it, so the engine never learns who its clients are.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;

use crate::config::EngineConfig;
use crate::engine::command::{Command, Reply};
use crate::engine::events::Event;
use crate::engine::headless::HeadlessPlan;
use crate::engine::{self, start, Engine};
use crate::hardware;
use crate::server;

/// Run headless on the radio `listing` names, as `plan` says, until SIGTERM
/// or Ctrl-C; then stop the stream and save the engine's blocks.
///
/// The log goes to stderr, a line per line, where a terminal, a pipe or the
/// journal takes it.
pub async fn run(
    mut cfg: EngineConfig,
    config_path: Option<PathBuf>,
    found: &[hardware::DeviceListing],
    chosen: usize,
    plan: HeadlessPlan,
) -> anyhow::Result<()> {
    let listing = &found[chosen];
    let device = hardware::open_device(listing, &cfg)?;
    let observable = listing.kind.observer_profile().is_some();
    let geometry = device.capabilities().sample_geometry;
    // No screen: the state starts from how a screen with nothing chosen
    // would show it.
    let state = Arc::new(Mutex::new(start::open(
        &cfg,
        engine::boot::Shown::default(),
        &device,
        observable,
    )?));
    let engine = Engine::new(Arc::clone(&state), Arc::clone(&device));

    // What was said while the radio opened, then everything said after.
    let log = engine.subscribe(1024);
    for entry in engine.snapshot().log {
        eprintln!("{}", entry.text);
    }
    std::thread::Builder::new()
        .name("headless-log".into())
        .spawn(move || {
            while let Some(delivered) = log.next_blocking() {
                if delivered.lost_before > 0 {
                    eprintln!("({} log lines lost)", delivered.lost_before);
                }
                if let Event::Log { text, .. } = delivered.event {
                    eprintln!("{text}");
                }
            }
        })?;

    let reply = engine.apply(Command::SetMode {
        section: plan.mode.section.clone(),
        view: plan.mode.view.clone(),
    });
    if let Reply::Refused { reason } = reply {
        anyhow::bail!("{reason}");
    }
    // What the terminal sets from the panels on screen, set here from the
    // plan: nothing reads the demodulator or the sweep headless yet.
    engine.with_state(|m| {
        m.spectrum.wanted = plan.spectrum;
        m.demod.enabled = false;
        m.sweep.active = false;
    });
    engine.apply(Command::Stream { on: true });
    // Bound before anything streams: an address already taken is said
    // before the radio is put to work.
    let listener = tokio::net::TcpListener::bind(plan.listen)
        .await
        .with_context(|| format!("cannot listen on {}", plan.listen))?;
    let access = match (&plan.token, plan.listen.ip().is_loopback()) {
        (Some(token), false) => server::Access::with_token(engine.clone(), token.clone()),
        _ => server::Access::loopback(engine.clone()),
    };
    let radios = found
        .iter()
        .enumerate()
        .map(|(i, d)| server::Radio::new(&d.label, d.kind, d.serial.clone(), i == chosen))
        .collect();
    let access = access.with_radios(radios).with_config(cfg.clone());

    let (rx_ctx, feeds) = start::feeds(&state, geometry);
    start::run(&state, &device, &rx_ctx, feeds, cfg.net.bt_channels);
    engine.with_state(|m| {
        m.push_log(format!(
            "Headless: computing for {}; the API is on http://{}/v1",
            plan.mode.view, plan.listen
        ))
    });

    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(server::serve(listener, server::router(access), async {
        let _ = stopped.await;
    }));
    until_asked_to_stop().await;
    let _ = stop.send(());
    if let Ok(Err(e)) = serving.await {
        eprintln!("the API stopped: {e}");
    }

    engine.hand_back_the_tuner();
    engine.apply(Command::Stream { on: false });
    engine.with_state(|m| m.push_log("Headless: stopping"));
    // A stream already stopped (by a client, or by the radio) has nothing to
    // stop, and asking a driver to stop one it is not running is an error.
    if engine.device().is_streaming() {
        if let Err(e) = engine.device().stop_rx() {
            eprintln!("stopping the stream: {e}");
        }
    }
    if let Some(path) = config_path {
        cfg.radio = engine::boot::radio_block(&engine.snapshot(), &cfg.radio);
        cfg.save(&path)?;
    }
    Ok(())
}

/// Until SIGTERM (a service manager stopping it) or Ctrl-C.
async fn until_asked_to_stop() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
