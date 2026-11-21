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
mod palette;
mod ui;

/// The engine, under the names the terminal has always used for its parts:
/// `crate::state::…` is Monas's state, `crate::signal::…` its signal chain.
pub(crate) use monas::{engine, export, geo, hardware, signal, state, tasks};

use anyhow::{Context, Result};
use app::App;
use clap::Parser;
use cli::Cli;
use config::SdrtopConfig;
use ratatui::crossterm::{
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
/// "Allocating zero-copy buffers", "Found … tuner", "\[R82XX\] PLL not locked!",
/// some from its own read thread - which would scribble over the alternate
/// screen. Sending it to a file keeps the TUI clean while preserving the output
/// for debugging. Best-effort: on error stderr is left alone and the reason is
/// returned, for the app's own log to say.
fn redirect_stderr_to_log() -> Result<i32, String> {
    use std::os::unix::io::AsRawFd;
    let file = open_log(&log_path())?;
    unsafe {
        let saved = libc::dup(libc::STDERR_FILENO);
        if saved < 0 {
            return Err("the log could not be attached to stderr".to_string());
        }
        if libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) < 0 {
            libc::close(saved);
            return Err("the log could not be attached to stderr".to_string());
        }
        // `file` drops here, closing its own fd; fd 2 keeps the open description.
        Ok(saved)
    }
}

/// The session log at `path`, opened to append, its directory made if
/// missing; why not, naming the file, when it cannot be.
fn open_log(path: &std::path::Path) -> Result<std::fs::File, String> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| {
            format!(
                "The log {} could not be opened ({e}): library messages may show on screen.",
                path.display()
            )
        })
}

/// What is said when no radio was found: the backends' hints, and where the
/// libraries' own messages went while sdrtop looked, when they went to the
/// log rather than the terminal.
fn no_device_message(hint: &str, log: Option<&std::path::Path>) -> String {
    let mut message = format!(
        "No device found. Connect a HackRF, RTL-SDR, Airspy R2 or HF+, SDRplay RSP1B, PlutoSky R2 or tinySA and try again.{hint}"
    );
    if let Some(log) = log {
        message.push_str(&format!(
            "\nWhat the radio libraries said while looking is in {}.",
            log.display()
        ));
    }
    message
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

fn application_result(result: io::Result<()>) -> Result<()> {
    result
        .map_err(anyhow::Error::from)
        .context("Application error")
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config_path = cli.config.or_else(default_config_path);
    // One read of the file: the terminal's blocks and the engine's.
    let mut app_cfg = config_path
        .as_deref()
        .map(SdrtopConfig::load_or_default)
        .unwrap_or_default();

    if let Some(f) = cli.frequency {
        app_cfg.engine.radio.frequency_hz = f;
    }
    // `--lna` / `--vga` are **positional**: the first and second stage, whatever
    // the device calls them. That is what they already meant on the two native
    // radios, and it is the only reading that works on a device whose stages are
    // named `IFGR` and `RFGR`. `resolve_gains` layers them over the config's own
    // gain, so one flag does not silently reset the other stages.
    if let Some(l) = cli.lna {
        app_cfg.engine.radio.lna_gain = Some(l);
    }
    if let Some(v) = cli.vga {
        app_cfg.engine.radio.vga_gain = Some(v);
    }
    // `--gain` takes either form the config file takes: a bare number, which is
    // the whole chain and gets distributed the way the knob distributes it, or
    // `NAME=value` pairs. One parser serves both, so the flag can never express
    // something the file cannot.
    if let Some(g) = cli.gain {
        app_cfg.engine.radio.gain = Some(g);
    }
    if let Some(t) = cli.theme {
        app_cfg.tui.theme.base = t;
    }

    let themes_dir = config_path
        .as_deref()
        .and_then(config::TuiConfig::themes_dir);
    let theme = app_cfg.tui.build_theme(themes_dir.as_deref());

    let (want, soapy_filter) = match cli.device.as_deref() {
        None => (None, None),
        Some(spec) => match hardware::discovery::parse_device_arg(spec) {
            Ok((kind, filter)) => (Some(kind), filter),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
    };

    if let Some(kind) = want {
        if let Err(err) = kind.check_available() {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
    // Looking for radios asks every library, and some talk on stderr while
    // they do: SoapyRTLSDR opens each dongle to read its tuner, and librtlsdr
    // says why it could not when another app holds one. That goes to the log
    // with the rest of their messages, not onto the terminal, where it would
    // sit behind the TUI and reappear at exit.
    let looking = redirect_stderr_to_log();
    let devices = hardware::list_all_devices(want, soapy_filter.as_deref());
    let logged = looking.ok().map(|saved| {
        restore_stderr(Some(saved));
        log_path()
    });
    if devices.is_empty() {
        eprintln!(
            "{}",
            no_device_message(
                &hardware::discovery::no_device_hint(want),
                logged.as_deref()
            )
        );
        std::process::exit(1);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // From here on the alternate screen is live - keep backend-library chatter
    // off it by routing stderr to the log file until we tear the TUI down.
    let (saved_stderr, log_problem) = match redirect_stderr_to_log() {
        Ok(fd) => (Some(fd), None),
        Err(why) => (None, Some(why)),
    };

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
                execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                terminal.show_cursor()?;
                return Ok(());
            }
            Err(e) => {
                restore_stderr(saved_stderr);
                disable_raw_mode()?;
                execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
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
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            terminal.show_cursor()?;
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    if let Some(why) = log_problem {
        app.note(why);
    }
    let result = app.run(&mut terminal);

    restore_stderr(saved_stderr);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Some(warning) = app.unsaved() {
        eprintln!("sdrtop: {warning}");
    }
    application_result(result)
}

#[cfg(test)]
mod cli_tests {
    use super::{application_result, Cli};
    use clap::CommandFactory;
    use std::io;

    /// **With no radio found, the message says where the libraries'
    /// messages went**: they were sent to the log while sdrtop looked, and a
    /// module that would not load is often the reason nothing was found.
    /// Where they could not be sent there, they were on the terminal already.
    #[test]
    fn no_radio_found_points_at_what_the_libraries_said() {
        let log = std::path::Path::new("/home/me/.config/sdrtop/sdrtop.log");
        let said = super::no_device_message("\nSoapySDR is installed.", Some(log));
        assert!(said.starts_with("No device found."), "{said}");
        assert!(said.contains("SoapySDR is installed."), "{said}");
        assert!(
            said.ends_with(&format!("is in {}.", log.display())),
            "{said}"
        );
        let unlogged = super::no_device_message("", None);
        assert!(!unlogged.contains("sdrtop.log"), "{unlogged}");
    }

    /// **A log that cannot be opened says which file and why**, so the
    /// reason backend messages land on the screen is not a mystery.
    #[test]
    fn a_log_that_cannot_be_opened_says_which_file_and_why() {
        let blocker =
            std::env::temp_dir().join(format!("sdrtop-log-blocked-{}", std::process::id()));
        std::fs::write(&blocker, "").unwrap();
        let path = blocker.join("sdrtop.log");
        let why = super::open_log(&path).unwrap_err();
        assert!(why.contains(&path.display().to_string()), "{why}");
        assert!(why.contains("could not be opened"), "{why}");
        let _ = std::fs::remove_file(&blocker);
    }

    #[test]
    fn application_errors_keep_the_full_chain_for_process_failure() {
        let error = anyhow::anyhow!("config file could not be replaced")
            .context("failed to save session settings");
        let rendered = format!(
            "{:?}",
            application_result(Err(io::Error::other(error))).unwrap_err()
        );

        assert_eq!(rendered.matches("Application error").count(), 1);
        assert_eq!(
            rendered.matches("failed to save session settings").count(),
            1
        );
        assert_eq!(
            rendered
                .matches("config file could not be replaced")
                .count(),
            1
        );
        assert!(!rendered.contains("Custom {"));
    }

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
            "/../../target/man/sdrtop.1"
        ))
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

    /// The body of one of the sections `--help` prints after the flags.
    fn section(heading: &str) -> &'static str {
        crate::cli::SECTIONS
            .iter()
            .find(|(h, _)| *h == heading)
            .map(|(_, body)| *body)
            .unwrap_or_else(|| panic!("--help has no {heading} section"))
    }

    /// The first line of a flag's help: what `-h` shows.
    fn flag_summary(name: &str) -> String {
        let cmd = Cli::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_long() == Some(name))
            .unwrap_or_else(|| panic!("there is no --{name}"));
        arg.get_help().map(ToString::to_string).unwrap_or_default()
    }

    /// Every key the help offers a newcomer is one the key handler answers.
    /// The full list is the menu's, which has its own test; these are the
    /// few worth knowing before opening it.
    #[test]
    fn the_first_keys_in_the_help_are_keys_sdrtop_answers() {
        let handler = include_str!("app/input/global/mod.rs");
        let mut checked = 0;
        for line in section("First keys")
            .lines()
            .filter(|l| l.starts_with("  "))
        {
            let (keys, _) = line
                .trim()
                .split_once("  ")
                .expect("a key, then what it does");
            let arms: Vec<String> = match keys.trim() {
                "Esc" => vec!["KeyCode::Esc".into()],
                "Space" => vec!["KeyCode::Char(' ')".into()],
                "Tab" => vec!["KeyCode::Tab".into()],
                "Up, Down" => vec!["KeyCode::Up".into(), "KeyCode::Down".into()],
                "1 to 9" => vec!["'1'..='9'".into()],
                k if k.chars().count() == 1 => vec![format!("KeyCode::Char('{k}')")],
                other => panic!("the help names a key this test cannot read: {other:?}"),
            };
            for arm in arms {
                assert!(
                    handler.contains(&arm),
                    "the help offers {keys}, but no {arm} arm answers it"
                );
            }
            checked += 1;
        }
        assert!(checked >= 8, "the key table was not found");
    }

    /// The files the help names are the paths sdrtop uses, each of them.
    #[test]
    fn the_help_names_the_files_sdrtop_uses() {
        let home = std::env::var("HOME").expect("HOME");
        let listed: Vec<std::path::PathBuf> = section("Files")
            .lines()
            .filter_map(|l| {
                l.trim_start()
                    .strip_prefix("~/")
                    .filter(|_| l.starts_with("  "))
            })
            .map(|rest| {
                let path = rest.split_whitespace().next().unwrap_or_default();
                std::path::Path::new(&home).join(path.trim_end_matches('/'))
            })
            .collect();
        let config = super::default_config_path().expect("a default config path");
        let mut used = vec![
            config.clone(),
            crate::config::TuiConfig::themes_dir(&config).unwrap(),
            crate::config::LayoutConfig::presets_dir(&config).unwrap(),
            super::log_path(),
        ];
        // The data folder as it is when $XDG_DATA_HOME is not set, which is
        // what the help's path names; the help says the other case in words.
        if std::env::var_os("XDG_DATA_HOME").is_none() {
            used.push(monas::export::destination::default_dir());
        } else {
            used.push(std::path::Path::new(&home).join(".local/share/sdrtop"));
        }
        let mut listed_sorted = listed.clone();
        listed_sorted.sort();
        used.sort();
        assert_eq!(
            listed_sorted, used,
            "the help's files are not the ones sdrtop uses"
        );
    }

    /// The built-in themes the help names are the built-ins, all of them.
    #[test]
    fn the_help_names_every_built_in_theme() {
        let summary = flag_summary("theme");
        let (_, list) = summary.split_once(": ").expect("the themes follow a colon");
        let mut named: Vec<&str> = list
            .split(", ")
            .map(str::trim)
            .filter(|n| !n.starts_with("or "))
            .collect();
        let mut builtin: Vec<&str> = crate::theme::Theme::builtin_names().collect();
        named.sort_unstable();
        builtin.sort_unstable();
        assert_eq!(named, builtin, "the help's themes are not the built-ins");
    }

    /// The kinds of radio `--device` names are the ones its parser takes,
    /// and the parser takes no kind the help leaves out.
    #[test]
    fn the_help_names_every_kind_of_radio_and_only_those() {
        let summary = flag_summary("device");
        let (_, list) = summary.split_once(": ").expect("the kinds follow a colon");
        let mut named: Vec<&str> = list.split(", ").map(str::trim).collect();
        for kind in &named {
            assert!(
                monas::hardware::discovery::parse_device_arg(kind).is_ok(),
                "--device {kind} is in the help but refused"
            );
        }
        // The parser's refusal lists the names it takes, each in quotes.
        let refusal = monas::hardware::discovery::parse_device_arg("nonsense")
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

    /// The man page carries each section `--help` adds, under its own heading.
    #[test]
    fn the_man_page_has_the_sections_the_help_has() {
        let page = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/man/sdrtop.1"
        ))
        .expect("build.rs should have generated target/man/sdrtop.1");
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

#[cfg(test)]
mod source_tests {
    /// The comment of a line, if it has one: what follows its first `//`.
    fn comment(line: &str) -> Option<&str> {
        line.find("//").map(|at| &line[at + 2..])
    }

    /// Why `text` points at a plan or design, if it does: the forms a
    /// comment used to cite them by.
    fn plan_reference(text: &str) -> Option<&'static str> {
        let lower = text.to_lowercase();
        if lower.contains("dev_docs") {
            return Some("a dev_docs path");
        }
        if lower.contains("-plan.md") || lower.contains("-design.md") {
            return Some("a plan or design file");
        }
        if lower.contains("design section") {
            return Some("a design section");
        }
        // A design note by its number: the three letters, a dash, a year.
        if text
            .match_indices("DSN-")
            .any(|(at, _)| text[at + 4..].starts_with(|c: char| c.is_ascii_digit()))
        {
            return Some("a design note's number");
        }
        if lower.contains("case study")
            || lower.contains("case-study")
            || lower.contains("case studies")
        {
            return Some("a case study");
        }
        if text
            .match_indices("Stop ")
            .any(|(at, _)| text[at + 5..].starts_with(|c: char| c.is_ascii_digit()))
        {
            return Some("a plan's stop");
        }
        // A plan named by its file stem, two words or more before the
        // suffix (a band plan is not one).
        let named = lower
            .split(|c: char| !(c.is_ascii_lowercase() || c == '-'))
            .any(|word| {
                (word.ends_with("-plan") || word.ends_with("-design"))
                    && word.matches('-').count() >= 2
            });
        named.then_some("a plan's name")
    }

    /// **The installer's source build is the release profile, less the
    /// link-time optimisation.** A published crate carries no profile, so
    /// packaging/install.sh asks `cargo install` for the release profile's
    /// `strip` and `panic` itself; this holds the two to the root manifest,
    /// so a change to the profile cannot leave the installer building a
    /// different binary from the tarball's without a word.
    #[test]
    fn the_installer_builds_with_the_release_profiles_strip_and_panic() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let manifest: toml::Table = std::fs::read_to_string(format!("{root}/Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let release = &manifest["profile"]["release"];
        let installer = std::fs::read_to_string(format!("{root}/packaging/install.sh")).unwrap();
        let strip = release["strip"].as_bool().unwrap();
        let panic = release["panic"].as_str().unwrap();
        assert!(
            installer.contains(&format!("CARGO_PROFILE_RELEASE_STRIP={strip}")),
            "install.sh does not ask for strip = {strip}"
        );
        assert!(
            installer.contains(&format!("CARGO_PROFILE_RELEASE_PANIC={panic}")),
            "install.sh does not ask for panic = {panic}"
        );
    }

    /// **Every internal crate is pinned exactly at the workspace's version.**
    /// A version of one crate works with the same version of the others and
    /// no other, so each is named in `[workspace.dependencies]` with its
    /// `path` and `version = "=X.Y.Z"`, X.Y.Z being `[workspace.package]`'s.
    /// A bump that misses a pin fails here, in CI, before any tag.
    #[test]
    fn the_internal_pins_are_the_workspace_version() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
                .expect("the workspace manifest");
        let manifest: toml::Table = text.parse().expect("the workspace manifest is TOML");
        let workspace = manifest["workspace"].as_table().unwrap();
        let version = workspace["package"]["version"].as_str().unwrap();
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
        let internal: Vec<(&String, &toml::Value)> = workspace["dependencies"]
            .as_table()
            .unwrap()
            .iter()
            .filter(|(_, dep)| dep.get("path").is_some())
            .collect();
        assert!(!internal.is_empty(), "no internal crate found");
        for (name, dep) in internal {
            assert_eq!(
                dep.get("version").and_then(|v| v.as_str()),
                Some(format!("={version}").as_str()),
                "{name} is not pinned at ={version}"
            );
        }
    }

    /// **Nothing in a comment points into the plans**, in any crate of the
    /// workspace. The plans are gitignored: a contributor reading the source
    /// cannot open what such a comment cites, so each comment says its own
    /// reason instead. The source was cleared of them once; this keeps it so.
    #[test]
    fn no_comment_points_at_a_plan() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let mut files = Vec::new();
        walk(&root.join("crates"), &mut files);
        assert!(files.len() > 400, "the workspace's source was not found");
        let mut found = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for (n, line) in text.lines().enumerate() {
                if let Some(why) = comment(line).and_then(plan_reference) {
                    let at = file.strip_prefix(root).unwrap_or(&file).display();
                    found.push(format!("{at}:{}: {why}: {}", n + 1, line.trim()));
                }
            }
        }
        assert!(
            found.is_empty(),
            "comments pointing at plans:\n{}",
            found.join("\n")
        );
    }

    /// The forms it looks for, and the ordinary words it must leave alone.
    #[test]
    fn plan_references_are_told_from_ordinary_words() {
        for cited in [
            " see dev_docs/foo.md",
            " as the ble-coded-plan.md says",
            " design section 6",
            " (case study)",
            " Stop 11 found this",
            " per net-ux-polish-plan",
            " redesign (DSN-2026-07)",
        ] {
            assert!(plan_reference(cited).is_some(), "missed: {cited}");
        }
        for ordinary in [
            " the band-plan label row",
            " the sweep stops at the edge",
            " Stop the stream before retuning",
            " code table G0 and Eb/N0",
            " the DSN-like spelling of a word",
        ] {
            assert_eq!(plan_reference(ordinary), None, "flagged: {ordinary}");
        }
    }
}
