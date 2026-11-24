// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! monasd: Monas with no screen. The engine, a radio, the API, and nothing
//! drawing; the terminal's counterpart, a front end of the engine and not
//! part of it, so the engine never learns who its clients are.
//!
//! It reads the same config file as the terminal (the engine's blocks and its
//! own `[monasd]`), writes back only the engine's, and runs until SIGTERM or
//! Ctrl-C.

#[cfg(test)]
mod api_doc;
/// The command line.
///
/// In its own file so `build.rs` can `include!` it and generate the man page from
/// the same `Parser` derive this parses with.
///
/// Its doc comments are the terminal's `--help`, not rustdoc's pages, so a
/// config block written as `[monasd]` there is text and not a link.
#[allow(rustdoc::broken_intra_doc_links)]
mod cli;
mod config;
mod plan;
mod record;
mod revive;
mod server;

/// The engine, under the names its parts have in Monas.
pub(crate) use monas::{engine, export, hardware, state};

/// What `core:recorder` names for the recordings this program writes.
fn recorder_name() -> String {
    format!("monasd {}", cli::VERSION)
}

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
    // The mark, for a person at a terminal; a pipe or the journal gets the
    // log alone.
    if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        eprintln!("{}\n", cli::logo(std::env::var_os("NO_COLOR").is_none()));
    }

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
    // plan: the demodulator for a mode whose work includes it (as long as it
    // has not been switched off), and no sweep, which nothing reads headless.
    engine.with_state(|m| {
        m.spectrum.wanted = plan.spectrum;
        m.demod.enabled = plan.mode.feeds.demod && m.demod.user_on;
        m.sweep.active = false;
    });
    // The display switches, before anything is served: a server's choice of
    // how much to show is set before the first client can ask.
    for command in &plan.display {
        if let Reply::Refused { reason } = engine.apply(command.clone()) {
            anyhow::bail!("{reason}");
        }
    }
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
    // The stream's feeds first: the recorder takes its blocks from them.
    let (rx_ctx, feeds) = start::feeds(&state, geometry);
    let recordings = Arc::new(record::Recordings::new(
        Arc::clone(&state),
        Arc::clone(&rx_ctx.record),
        plan.recordings_dir.clone(),
        cfg.record.clone(),
        recorder_name(),
    ));
    let access = access
        .with_radios(radios)
        .with_config(cfg.clone())
        .with_recordings(Arc::clone(&recordings));
    let running = start::run(&state, &device, &rx_ctx, feeds, cfg.net.bt_channels);
    // The radio, should it go: the same kind and serial, looked for again.
    let wanted = listing.clone();
    let open_cfg = cfg.clone();
    let reopen: Arc<revive::Reopen> = Arc::new(move || {
        let found = hardware::list_all_devices(Some(wanted.kind), None);
        let same = found.iter().find(|d| {
            d.kind == wanted.kind
                && match (&wanted.serial, &d.serial) {
                    (Some(a), Some(b)) => a == b,
                    _ => d.label == wanted.label,
                }
        });
        let Some(listing) = same else {
            return Err(format!("no {} found", wanted.label));
        };
        hardware::open_device(listing, &open_cfg).map_err(|e| format!("{e:#}"))
    });
    let (stop_watch, watch_stopped) = tokio::sync::watch::channel(false);
    let watcher = tokio::spawn(revive::watch(
        engine.clone(),
        Arc::clone(&state),
        Arc::clone(&recordings),
        revive::Run {
            running,
            rx_ctx,
            bt_channels: cfg.net.bt_channels,
        },
        reopen,
        revive::backoff,
        watch_stopped,
    ));
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
    // No radio is brought back while the program is on its way out.
    let _ = stop_watch.send(true);
    let _ = watcher.await;
    let _ = stop.send(());
    if let Ok(Err(e)) = serving.await {
        eprintln!("the API stopped: {e}");
    }
    // A recording still running is closed, its metadata saying why.
    recordings.finish();

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

    /// The body of one of the sections `--help` prints after the flags.
    fn section(heading: &str) -> &'static str {
        crate::cli::SECTIONS
            .iter()
            .find(|(h, _)| *h == heading)
            .map(|(_, body)| *body)
            .unwrap_or_else(|| panic!("--help has no {heading} section"))
    }

    /// The help a flag shows under `--help`, or its one line where it has no more.
    fn flag_help(name: &str) -> String {
        let cmd = Cli::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_long() == Some(name))
            .unwrap_or_else(|| panic!("there is no --{name}"));
        arg.get_long_help()
            .or(arg.get_help())
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    /// The Modes table names every mode the engine knows, each once, and
    /// nothing else: a table written out by hand is where a new mode goes
    /// missing.
    #[test]
    fn the_help_lists_every_mode_and_only_modes() {
        let mut listed: Vec<&str> = section("Modes")
            .lines()
            .filter(|l| l.starts_with("  "))
            .filter_map(|l| l.trim().split_once("  ").map(|(_, names)| names))
            .flat_map(|names| names.trim().split(", "))
            .collect();
        let count = listed.len();
        listed.sort_unstable();
        listed.dedup();
        assert_eq!(listed.len(), count, "a mode is listed twice");
        let mut modes: Vec<&str> = crate::engine::mode::MODES.iter().map(|(n, _)| *n).collect();
        modes.sort_unstable();
        assert_eq!(listed, modes, "the help's modes are not the engine's");
    }

    /// The API section lists the routes the server answers on, with their
    /// methods, in both directions.
    #[test]
    fn the_help_lists_every_route_and_only_routes() {
        let mut listed: Vec<(String, String)> = section("API")
            .lines()
            .filter_map(|l| {
                let mut words = l.split_whitespace();
                let method = words.next()?;
                let path = words.next()?;
                (["GET", "POST"].contains(&method) && path.starts_with("/v1/"))
                    .then(|| (method.to_lowercase(), path.to_string()))
            })
            .collect();
        // What the router is built with, read from its source: a route is
        // `.route("/v1/x", get(handler))`.
        let mut served: Vec<(String, String)> = include_str!("server.rs")
            .lines()
            .filter_map(|l| {
                let rest = l.trim().strip_prefix(".route(\"")?;
                let (path, rest) = rest.split_once("\", ")?;
                let (method, _) = rest.split_once('(')?;
                Some((method.to_string(), path.to_string()))
            })
            .collect();
        listed.sort();
        served.sort();
        assert!(
            served.len() >= 5,
            "the router's routes were not found: {served:?}"
        );
        assert_eq!(listed, served, "the help's routes are not the server's");
    }

    /// The kinds of radio `--device` names are the ones its parser takes,
    /// and the parser takes no kind the help leaves out.
    #[test]
    fn the_help_names_every_kind_of_radio_and_only_those() {
        let help = flag_help("device");
        let first = help.lines().next().unwrap_or_default();
        let (_, list) = first.split_once(": ").expect("the kinds follow a colon");
        let mut named: Vec<&str> = list.split(", ").map(str::trim).collect();
        for kind in &named {
            assert!(
                crate::hardware::discovery::parse_device_arg(kind).is_ok(),
                "--device {kind} is in the help but refused"
            );
        }
        // The parser's refusal lists the names it takes, each in quotes.
        let refusal = crate::hardware::discovery::parse_device_arg("nonsense")
            .unwrap_err()
            .to_string();
        let mut taken: Vec<&str> = refusal
            .split('\'')
            .skip(3)
            .step_by(2)
            .filter(|w| w.chars().all(|c| c.is_ascii_lowercase()))
            .collect();
        named.sort_unstable();
        taken.sort_unstable();
        assert_eq!(named, taken, "the help's kinds are not the parser's");
    }

    /// The address the help gives as the default is the one used.
    #[test]
    fn the_help_gives_the_default_address_that_is_used() {
        let default = format!("127.0.0.1:{}", crate::plan::DEFAULT_PORT);
        assert!(flag_help("listen").contains(&default));
        assert!(section("Examples").contains(&default));
        let plan = crate::plan::plan(None, None, &Default::default()).unwrap();
        assert_eq!(plan.listen.to_string(), default);
        assert_eq!(plan.mode.view, "spectrum");
        assert!(flag_help("mode").contains("(default spectrum)"));
    }

    /// The config block the help shows is one the config reads, and one
    /// monasd would start with: a token beside an address off loopback.
    #[test]
    fn the_help_shows_a_config_block_that_works() {
        let block: String = section("Config")
            .lines()
            .filter(|l| l.starts_with("  "))
            .map(|l| format!("{}\n", l.trim()))
            .collect();
        let read: crate::config::MonasdConfig = toml::from_str(&block).unwrap();
        let settings = &read.monasd;
        assert!(settings.listen.is_some() && settings.token.is_some() && settings.mode.is_some());
        let plan = crate::plan::plan(None, None, settings).unwrap();
        assert!(!plan.listen.ip().is_loopback());
    }

    /// The long help fits an 80-column terminal as written, and is ASCII,
    /// for the man page.
    #[test]
    fn the_help_fits_eighty_columns_and_is_ascii() {
        let cmd = Cli::command();
        let long_about = cmd
            .get_long_about()
            .map(ToString::to_string)
            .unwrap_or_default();
        let after = crate::cli::after_long_help();
        for line in long_about.lines().chain(after.lines()) {
            assert!(line.len() <= 80, "wider than 80 columns: {line:?}");
            assert!(line.is_ascii(), "not ASCII: {line:?}");
        }
        for arg in cmd.get_arguments() {
            for help in [arg.get_help(), arg.get_long_help()].into_iter().flatten() {
                assert!(
                    help.to_string().is_ascii(),
                    "--{:?} help is not ASCII",
                    arg.get_long()
                );
            }
        }
    }

    /// The mark opens `--help`, its colours are the only thing the coloured
    /// one adds, and it stays out of the man page, which is ASCII.
    #[test]
    fn the_logo_opens_the_help_and_stays_out_of_the_man_page() {
        let plain = crate::cli::logo(false);
        assert_eq!(plain.lines().collect::<Vec<_>>(), crate::cli::LOGO);
        // The coloured one, its escapes taken out, is the plain one.
        let coloured = crate::cli::logo(true);
        let mut stripped = String::new();
        let mut rest = coloured.as_str();
        while let Some(at) = rest.find('\x1b') {
            stripped.push_str(&rest[..at]);
            rest = &rest[at..];
            rest = &rest[rest.find('m').expect("an escape ends in m") + 1..];
        }
        stripped.push_str(rest);
        assert_eq!(stripped, plain);
        let before = Cli::command().get_before_help().map(|h| h.to_string());
        assert!(before.is_some_and(|h| h.contains("monas")));
        let page = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/man/monasd.1"
        ))
        .expect("build.rs should have generated target/man/monasd.1");
        assert!(page.is_ascii(), "the man page is not ASCII");
    }

    /// The man page carries each section `--help` adds, under its own heading.
    #[test]
    fn the_man_page_has_the_sections_the_help_has() {
        let page = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/man/monasd.1"
        ))
        .expect("build.rs should have generated target/man/monasd.1");
        for (heading, _) in crate::cli::SECTIONS {
            assert!(
                page.contains(&format!(".SH \"{}\"", heading.to_uppercase()))
                    || page.contains(&format!(".SH {}\n", heading.to_uppercase())),
                "the man page has no {heading} section"
            );
        }
        assert!(
            !page.contains(".SH EXTRA"),
            "the sections ran together under EXTRA"
        );
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
