// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

mod app;
/// The command line.
///
/// In its own file so `build.rs` can `include!` it and generate the man page from
/// the same `Parser` derive this parses with: a manual written by hand, or from a
/// copy of the flags, drifts away from the binary. One definition, two consumers.
mod cli;
mod config;
mod theme;
pub use theme::Theme;
mod event;
mod hardware;
mod palette;
mod signal;
mod state;
mod tasks;
mod ui;

use anyhow::Result;
use app::App;
use clap::Parser;
use cli::Cli;
use config::AppConfig;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::PathBuf;

fn default_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/sdrtop/config.toml"))
}

fn log_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config/sdrtop/sdrtop.log"))
        .unwrap_or_else(|| PathBuf::from("/tmp/sdrtop.log"))
}

/// Redirect stderr (fd 2) to a log file for the TUI session and return the saved
/// original fd. Backend libraries are chatty on stderr - librtlsdr prints
/// "Allocating zero-copy buffers", "Found … tuner", "[R82XX] PLL not locked!",
/// some from its own read thread - which would scribble over the alternate
/// screen. Sending it to a file keeps the TUI clean while preserving the output
/// for debugging. Best-effort: returns `None` (and leaves stderr alone) on error.
fn redirect_stderr_to_log() -> Option<i32> {
    use std::os::unix::io::AsRawFd;
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    unsafe {
        let saved = libc::dup(libc::STDERR_FILENO);
        if saved < 0 {
            return None;
        }
        if libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) < 0 {
            libc::close(saved);
            return None;
        }
        // `file` drops here, closing its own fd; fd 2 keeps the open description.
        Some(saved)
    }
}

/// Restore the real stderr saved by [`redirect_stderr_to_log`].
fn restore_stderr(saved: Option<i32>) {
    if let Some(s) = saved {
        unsafe {
            libc::dup2(s, libc::STDERR_FILENO);
            libc::close(s);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config_path = cli.config.or_else(default_config_path);
    let mut app_cfg = config_path
        .as_deref()
        .map(AppConfig::load_or_default)
        .unwrap_or_default();

    if let Some(f) = cli.frequency {
        app_cfg.radio.frequency_hz = f;
    }
    if let Some(l) = cli.lna {
        app_cfg.radio.lna_gain = l.min(40);
    }
    if let Some(v) = cli.vga {
        app_cfg.radio.vga_gain = v.min(62);
    }
    // --gain is the device-agnostic primary gain (applied after --lna so it wins);
    // the device clamps/snaps it at program time (HackRF LNA range, RTL nearest step).
    if let Some(g) = cli.gain {
        app_cfg.radio.lna_gain = g;
    }
    if let Some(t) = cli.theme {
        app_cfg.theme.base = t;
    }

    let themes_dir = config_path
        .as_deref()
        .and_then(config::AppConfig::themes_dir);
    let theme = app_cfg.build_theme(themes_dir.as_deref());

    // `--device` names a backend, and for SoapySDR it may carry the device
    // arguments too: `--device soapy=driver=airspy`. Those arguments are both a
    // filter and the way to ask for a driver sdrtop skips by default.
    let (want, soapy_filter) = match cli.device.as_deref() {
        None => (None, None),
        Some(spec) => {
            let (name, filter) = match spec.split_once('=') {
                Some((n, f)) => (n, Some(f)),
                None => (spec, None),
            };
            let kind = match name.to_ascii_lowercase().as_str() {
                "hackrf" => hardware::DeviceKind::HackRf,
                "rtlsdr" | "rtl-sdr" | "rtl" => hardware::DeviceKind::RtlSdr,
                "soapy" | "soapysdr" => hardware::DeviceKind::Soapy,
                other => {
                    eprintln!(
                        "Unknown --device '{other}' (use 'hackrf', 'rtlsdr', or \
                         'soapy', optionally as 'soapy=driver=airspy')"
                    );
                    std::process::exit(1);
                }
            };
            (Some(kind), filter)
        }
    };

    let devices = hardware::list_all_devices(want, soapy_filter);
    if devices.is_empty() {
        // Mentioning SoapySDR only where it could actually help. Telling someone
        // to install a driver for a library they do not have is a wild goose
        // chase, and they have enough to check already.
        let soapy_hint = if hardware::soapy::api::api().is_some() {
            " SoapySDR is installed: `SoapySDRUtil --find` lists what it can see."
        } else {
            ""
        };
        eprintln!("No SDR device found. Connect a HackRF or RTL-SDR and try again.{soapy_hint}");
        std::process::exit(1);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // From here on the alternate screen is live - keep backend-library chatter
    // off it by routing stderr to the log file until we tear the TUI down.
    let saved_stderr = redirect_stderr_to_log();

    let selected = if devices.len() > 1 {
        let items: Vec<(usize, String)> = devices
            .iter()
            .enumerate()
            .map(|(i, d)| (i, d.label.clone()))
            .collect();
        match ui::device_selector::run(items, &theme, &mut terminal) {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                restore_stderr(saved_stderr);
                disable_raw_mode()?;
                execute!(
                    terminal.backend_mut(),
                    LeaveAlternateScreen,
                    DisableMouseCapture
                )?;
                terminal.show_cursor()?;
                return Ok(());
            }
            Err(e) => {
                restore_stderr(saved_stderr);
                disable_raw_mode()?;
                execute!(
                    terminal.backend_mut(),
                    LeaveAlternateScreen,
                    DisableMouseCapture
                )?;
                terminal.show_cursor()?;
                eprintln!("Device selection error: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        0
    };

    let mut app = match App::new(app_cfg, config_path, &devices[selected]) {
        Ok(a) => a,
        Err(e) => {
            restore_stderr(saved_stderr);
            disable_raw_mode()?;
            execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture
            )?;
            terminal.show_cursor()?;
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    let result = app.run(&mut terminal);

    restore_stderr(saved_stderr);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = result {
        eprintln!("Application error: {:?}", err);
    }

    Ok(())
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
        let page =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/target/man/sdrtop.1"))
                .expect("build.rs should have generated target/man/sdrtop.1");

        let cmd = Cli::command();
        let flags: Vec<String> = cmd
            .get_arguments()
            .filter_map(|a| a.get_long())
            .map(|l| l.to_string())
            .collect();
        assert!(
            flags.len() >= 7,
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
}
