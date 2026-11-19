// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! monasd: Monas with no screen. The engine, a radio, the API, and nothing
//! drawing; the terminal's counterpart, a front end of the engine and not
//! part of it, so the engine never learns who its clients are.
//!
//! It reads the same config file as the terminal (the engine's blocks and its
//! own `[monasd]`), writes back only the engine's, and runs until SIGTERM or
//! Ctrl-C.

/// The command line.
///
/// In its own file so `build.rs` can `include!` it and generate the man page from
/// the same `Parser` derive this parses with.
mod cli;
mod config;
mod plan;
mod server;

/// The engine, under the names its parts have in Monas.
pub(crate) use monas::{engine, hardware};

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use clap::Parser;

use crate::config::{EngineConfig, MonasdConfig};
use crate::engine::command::{Command, Reply};
use crate::engine::events::Event;
use crate::engine::{start, Engine};
use cli::Cli;
use plan::HeadlessPlan;

/// The config file the terminal uses too: the radio's settings are shared.
fn default_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/sdrtop/config.toml"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let config_path = cli.config.or_else(default_config_path);
    let MonasdConfig { mut engine, monasd } = config_path
        .as_deref()
        .map(MonasdConfig::load_or_default)
        .unwrap_or_default();

    if let Some(f) = cli.frequency {
        engine.radio.frequency_hz = f;
    }
    // `--lna` / `--vga` are **positional**: the first and second stage,
    // whatever the device calls them, layered over the config's own gain.
    // `--gain` takes either form the config file takes.
    if let Some(l) = cli.lna {
        engine.radio.lna_gain = Some(l);
    }
    if let Some(v) = cli.vga {
        engine.radio.vga_gain = Some(v);
    }
    if let Some(g) = cli.gain {
        engine.radio.gain = Some(g);
    }

    // Decided before any radio is looked for: a run that could not run as
    // asked says so at once.
    let plan = match plan::plan(cli.listen.as_deref(), cli.mode.as_deref(), &monasd) {
        Ok(plan) => plan,
        Err(why) => {
            eprintln!("monasd: {why}");
            std::process::exit(1);
        }
    };

    let (want, soapy_filter) = match cli.device.as_deref() {
        None => (None, None),
        Some(spec) => match hardware::discovery::parse_device_arg(spec) {
            Ok((kind, filter)) => (Some(kind), filter),
            Err(e) => {
                eprintln!("monasd: {e}");
                std::process::exit(1);
            }
        },
    };
    if let Some(kind) = want {
        if let Err(err) = kind.check_available() {
            eprintln!("monasd: {err}");
            std::process::exit(1);
        }
    }
    // The libraries' own messages while looking go where everything else
    // monasd says goes: stderr.
    let devices = hardware::list_all_devices(want, soapy_filter.as_deref());
    if devices.is_empty() {
        eprintln!(
            "monasd: no radio found.{}",
            hardware::discovery::no_device_hint(want)
        );
        std::process::exit(1);
    }
    let labels: Vec<String> = devices.iter().map(|d| d.label.clone()).collect();
    let chosen = match plan::pick(&labels) {
        Ok(i) => i,
        Err(why) => {
            eprintln!("monasd: {why}");
            std::process::exit(1);
        }
    };
    run(engine, config_path, &devices, chosen, plan)
        .await
        .context("monasd")
}

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
        crate::engine::boot::Shown::default(),
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
        .name("monasd-log".into())
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
            "monasd: computing for {}; the API is on http://{}/v1",
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
    engine.with_state(|m| m.push_log("monasd: stopping"));
    // A stream already stopped (by a client, or by the radio) has nothing to
    // stop, and asking a driver to stop one it is not running is an error.
    if engine.device().is_streaming() {
        if let Err(e) = engine.device().stop_rx() {
            eprintln!("stopping the stream: {e}");
        }
    }
    if let Some(path) = config_path {
        cfg.radio = crate::engine::boot::radio_block(&engine.snapshot(), &cfg.radio);
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

#[cfg(test)]
mod cli_tests {
    use super::Cli;
    use clap::CommandFactory;

    /// `--version` has to exist and lead with the crate's own version.
    ///
    /// It is the first thing anyone pastes into a bug report, and a packaged
    /// binary that cannot say which build it is makes every report ambiguous.
    ///
    /// Starts with rather than equals: `build.rs` appends the commit when it can
    /// find one, so the string is `0.4.1 (2ec9491)` from a checkout or a
    /// published crate and a bare `0.4.1` from a source tree with neither. All
    /// three are legal, and the crate version leading is the part that must
    /// hold, because that is what the tag and crates.io agree on.
    #[test]
    fn the_binary_reports_its_own_version() {
        let cmd = Cli::command();
        let reported = cmd.get_version().expect("--version is not wired up at all");
        let crate_version = env!("CARGO_PKG_VERSION");

        assert!(
            reported.starts_with(crate_version),
            "--version reports {reported:?}, which does not lead with the crate version {crate_version:?}"
        );

        // Whatever follows is the commit, and it has to look like one: a
        // parenthesised short sha, optionally marked dirty. A malformed suffix
        // means build.rs put something else there, which a bug report would
        // then carry as if it were a commit.
        let suffix = &reported[crate_version.len()..];
        assert!(
            suffix.is_empty()
                || (suffix.starts_with(" (")
                    && suffix.ends_with(')')
                    && suffix[2..suffix.len() - 1]
                        .trim_end_matches("-dirty")
                        .chars()
                        .all(|c| c.is_ascii_hexdigit())),
            "--version suffix {suffix:?} is not a parenthesised short commit"
        );
    }

    /// The man page describes exactly the flags the binary has - no more, no less.
    ///
    /// It is generated by `build.rs` from `src/cli.rs`, the same definition this
    /// parses with, so the two cannot drift. This reads the generated file back
    /// and checks both directions, because "generated from the same source" is a
    /// claim about the build script that nothing else verifies.
    #[test]
    fn the_man_page_documents_every_flag_and_invents_none() {
        let page = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/man/monasd.1"
        ))
        .expect("build.rs should have generated target/man/monasd.1");

        let cmd = Cli::command();
        let flags: Vec<String> = cmd
            .get_arguments()
            .filter_map(|a| a.get_long())
            .map(|l| l.to_string())
            .collect();
        assert!(
            flags.len() >= 8,
            "expected the real flag set, got {flags:?}"
        );

        for flag in &flags {
            // roff escapes a leading hyphen as `\-`.
            let escaped = format!("\\-\\-{}", flag.replace('-', "\\-"));
            assert!(
                page.contains(&escaped),
                "--{flag} is a flag but the man page never mentions it"
            );
        }

        // And nothing in the manual that is not a flag.
        let documented: std::collections::HashSet<String> =
            regex_long_flags(&page).into_iter().collect();
        let known: std::collections::HashSet<String> = flags.into_iter().collect();
        let invented: Vec<&String> = documented.difference(&known).collect();
        assert!(
            invented.is_empty(),
            "the man page documents flags the binary does not have: {invented:?}"
        );
    }

    /// Long options as the man page spells them: `\fB\-\-name\fR`.
    fn regex_long_flags(page: &str) -> Vec<String> {
        let mut out = Vec::new();
        for part in page.split("\\fB\\-\\-").skip(1) {
            // Up to the closing font escape, then unescape the roff hyphens an
            // internal `-` in a flag name would carry.
            let Some(name) = part.split("\\fR").next() else {
                continue;
            };
            let name = name.replace('\\', "");
            if !name.is_empty() && name != "help" && name != "version" {
                out.push(name);
            }
        }
        out
    }

    /// The crate metadata is what the generated man page and `--help` are built
    /// from, and what a release page quotes. Asserted here so it cannot be
    /// broken by an edit to `Cargo.toml` that nothing else notices.
    #[test]
    fn the_package_metadata_the_man_page_needs_is_present() {
        assert!(
            !env!("CARGO_PKG_DESCRIPTION").is_empty(),
            "the man page's NAME section is the package description"
        );
        assert!(!env!("CARGO_PKG_REPOSITORY").is_empty());
        assert_eq!(env!("CARGO_PKG_LICENSE"), "GPL-3.0-or-later");
        // The command's own `about` and the package description are two different
        // strings on purpose: one is a one-line usage banner, the other is the
        // longer sentence the man page and the README open with. Both must exist.
        assert!(Cli::command().get_about().is_some());
    }

    /// `--listen` and `--mode` stand alone: there is no screen to switch off.
    #[test]
    fn the_run_flags_need_nothing_else() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["monasd", "--listen", "0.0.0.0:7373", "--mode", "net_survey"])
                .unwrap();
        assert_eq!(cli.listen.as_deref(), Some("0.0.0.0:7373"));
        assert_eq!(cli.mode.as_deref(), Some("net_survey"));
    }
}
