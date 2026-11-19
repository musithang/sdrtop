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
mod engine;
mod event;
mod export;
mod geo;
mod hardware;
mod headless;
mod host;
mod palette;
mod server;
mod signal;
mod state;
mod tasks;
mod ui;

use anyhow::{Context, Result};
use app::App;
use clap::Parser;
use cli::Cli;
use config::AppConfig;
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
    let mut app_cfg = config_path
        .as_deref()
        .map(AppConfig::load_or_default)
        .unwrap_or_default();

    if let Some(f) = cli.frequency {
        app_cfg.radio.frequency_hz = f;
    }
    // `--lna` / `--vga` are **positional**: the first and second stage, whatever
    // the device calls them. That is what they already meant on the two native
    // radios, and it is the only reading that works on a device whose stages are
    // named `IFGR` and `RFGR`. `resolve_gains` layers them over the config's own
    // gain, so one flag does not silently reset the other stages.
    if let Some(l) = cli.lna {
        app_cfg.radio.lna_gain = Some(l);
    }
    if let Some(v) = cli.vga {
        app_cfg.radio.vga_gain = Some(v);
    }
    // `--gain` takes either form the config file takes: a bare number, which is
    // the whole chain and gets distributed the way the knob distributes it, or
    // `NAME=value` pairs. One parser serves both, so the flag can never express
    // something the file cannot.
    if let Some(g) = cli.gain {
        app_cfg.radio.gain = Some(g);
    }
    if let Some(t) = cli.theme {
        app_cfg.theme.base = t;
    }

    // Decided before any radio is looked for: a headless run that could not
    // run as asked says so at once.
    let headless = if cli.headless {
        let mut layouts = config::LayoutConfig::default_config();
        layouts.presets.extend(app_cfg.presets.clone());
        match engine::headless::plan(
            cli.listen.as_deref(),
            cli.mode.as_deref(),
            &app_cfg.headless,
            &layouts,
        ) {
            Ok(plan) => Some(plan),
            Err(why) => {
                eprintln!("sdrtop: {why}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    let themes_dir = config_path
        .as_deref()
        .and_then(config::AppConfig::themes_dir);
    let theme = app_cfg.build_theme(themes_dir.as_deref());

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

    if let Some(plan) = headless {
        let labels: Vec<String> = devices.iter().map(|d| d.label.clone()).collect();
        let chosen = match engine::headless::pick(&labels) {
            Ok(i) => i,
            Err(why) => {
                eprintln!("sdrtop: {why}");
                std::process::exit(1);
            }
        };
        return headless::run(app_cfg, config_path, &devices, chosen, plan)
            .await
            .context("Headless error");
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

    /// `--listen` and `--mode` say how a headless run runs, so they mean
    /// nothing without `--headless`, and saying one alone is an error rather
    /// than a flag quietly ignored.
    #[test]
    fn the_headless_flags_need_headless() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["sdrtop", "--listen", "127.0.0.1:7373"]).is_err());
        assert!(Cli::try_parse_from(["sdrtop", "--mode", "net_survey"]).is_err());
        let cli = Cli::try_parse_from([
            "sdrtop",
            "--headless",
            "--listen",
            "0.0.0.0:7373",
            "--mode",
            "net_survey",
        ])
        .unwrap();
        assert!(cli.headless);
        assert_eq!(cli.listen.as_deref(), Some("0.0.0.0:7373"));
        assert_eq!(cli.mode.as_deref(), Some("net_survey"));
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

    /// **Nothing in a comment points into the plans.** The plans are
    /// gitignored: a contributor reading the source cannot open what such a
    /// comment cites, so each comment says its own reason instead. The
    /// source was cleared of them once; this keeps it so.
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
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        walk(&root.join("src"), &mut files);
        assert!(files.len() > 100, "the source tree was not found");
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

    /// What the engine may not reach for: the drawing libraries and the
    /// modules that draw or take keys.
    const SCREEN_PATHS: [&str; 6] = [
        "ratatui",
        "crossterm",
        "crate::ui",
        "crate::app",
        "crate::theme",
        "crate::palette",
    ];

    /// The lines of the code (not of its comments) under `dirs` that name a
    /// screen path, as `file:line: text`.
    fn screen_references(dirs: &[&str]) -> Vec<String> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            if dir.is_file() {
                out.push(dir.to_path_buf());
                return;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in dirs {
            walk(&root.join(dir), &mut files);
        }
        let mut found = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if SCREEN_PATHS.iter().any(|path| code.contains(path)) {
                    let at = file.strip_prefix(root).unwrap_or(&file).display();
                    found.push(format!("{at}:{}: {}", n + 1, line.trim()));
                }
            }
        }
        found
    }

    /// **The engine never sees the screen.** Everything that measures runs
    /// with no terminal behind it, so a headless instance and any client
    /// other than the TUI get exactly what the TUI gets. One crate keeps
    /// the screen in reach of every module; this is what keeps it out of
    /// the engine's.
    #[test]
    fn the_engine_never_imports_the_ui() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(
            root.join("src/engine/mod.rs").is_file(),
            "the engine module was not found"
        );
        let found = screen_references(&["src/engine", "src/server", "src/headless.rs"]);
        assert!(
            found.is_empty(),
            "the engine reaches for the screen:\n{}",
            found.join("\n")
        );
    }

    /// Where the Monas side still names the screen in its production code:
    /// each a file and the screen path it names. **This list only shrinks.**
    /// An entry goes when its coupling is fixed, and the test fails until it
    /// does; a coupling not on it fails the test at once. When it is empty
    /// the crate boundary holds the line instead.
    const MONAS_CODE_NAMES_THE_SCREEN: &[(&str, &str)] = &[];

    /// The same for test code on the Monas side, which may draw a panel to
    /// check what a state looks like. These move to the terminal's side with
    /// the panels when the source becomes crates.
    const MONAS_TESTS_NAME_THE_SCREEN: &[(&str, &str)] = &[
        ("src/export/census.rs", "crate::ui"),
        ("src/export/ft8.rs", "crate::ui"),
        ("src/export/occupancy.rs", "crate::ui"),
        ("src/state/fixture.rs", "crate::Theme"),
        ("src/state/fixture.rs", "crate::ui"),
        ("src/state/fixture.rs", "ratatui"),
    ];

    /// The same for doc links, which would point across the boundary.
    const MONAS_DOCS_LINK_THE_SCREEN: &[(&str, &str)] = &[];

    /// **The Monas side names the screen only where it is known to.** The
    /// engine's own modules are held strictly by
    /// [`the_engine_never_imports_the_ui`]; the rest of what measures (the
    /// signal chain, the radio, the tasks, the state, the exports, the map)
    /// is held to the three lists above, which only shrink.
    #[test]
    fn the_monas_side_names_the_screen_only_where_listed() {
        use std::collections::BTreeSet;
        const SCREEN: [&str; 8] = [
            "ratatui",
            "crossterm",
            "crate::ui",
            "crate::app",
            "crate::theme",
            "crate::palette",
            "crate::event",
            "crate::Theme",
        ];
        // What a doc link may start with to point at the screen, with or
        // without `crate::`.
        const SCREEN_MODULES: [&str; 5] = ["ui", "app", "theme", "palette", "event"];
        fn walk(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            if path.is_file() {
                out.push(path.to_path_buf());
                return;
            }
            for entry in std::fs::read_dir(path).unwrap() {
                let p = entry.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|e| e == "rs") {
                    out.push(p);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in [
            "src/signal",
            "src/hardware",
            "src/tasks",
            "src/state",
            "src/export",
            "src/geo",
            "src/host.rs",
        ] {
            walk(&root.join(dir), &mut files);
        }
        assert!(files.len() > 100, "the source tree was not found");
        // A file a `mod.rs` declares under `#[cfg(test)]` is test code from
        // its first line.
        let mut test_files = BTreeSet::new();
        for file in &files {
            let text = std::fs::read_to_string(file).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for (n, line) in lines.iter().enumerate() {
                if line.trim() != "#[cfg(test)]" {
                    continue;
                }
                let Some(decl) = lines.get(n + 1).map(|l| l.trim()) else {
                    continue;
                };
                let Some(name) = decl
                    .strip_suffix(';')
                    .and_then(|d| d.rsplit_once("mod "))
                    .map(|(_, name)| name)
                else {
                    continue;
                };
                let dir = file.parent().unwrap();
                let here = if file.file_name().is_some_and(|f| f == "mod.rs") {
                    dir.to_path_buf()
                } else {
                    dir.join(file.file_stem().unwrap())
                };
                test_files.insert(here.join(format!("{name}.rs")));
                test_files.insert(here.join(name).join("mod.rs"));
            }
        }
        let (mut code, mut tests, mut docs) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        for file in &files {
            let at = file.strip_prefix(root).unwrap().display().to_string();
            let text = std::fs::read_to_string(file).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            let mut in_tests = test_files.contains(file);
            for (n, line) in lines.iter().enumerate() {
                if line.trim() == "#[cfg(test)]"
                    && lines
                        .get(n + 1)
                        .is_some_and(|next| next.contains("mod ") && next.ends_with('{'))
                {
                    in_tests = true;
                }
                let trimmed = line.trim_start();
                if trimmed.starts_with("///") || trimmed.starts_with("//!") {
                    for module in SCREEN_MODULES {
                        for form in [
                            format!("[`{module}::"),
                            format!("[`crate::{module}::"),
                            format!("](crate::{module}::"),
                            format!("]({module}::"),
                        ] {
                            if line.contains(&form) {
                                docs.insert((at.clone(), module.to_string()));
                            }
                        }
                    }
                    continue;
                }
                let code_part = line.split("//").next().unwrap_or("");
                for path in SCREEN {
                    if code_part.contains(path) {
                        let into = if in_tests { &mut tests } else { &mut code };
                        into.insert((at.clone(), path.to_string()));
                    }
                }
            }
        }
        let mut problems = Vec::new();
        for (what, found, listed) in [
            ("code", &code, MONAS_CODE_NAMES_THE_SCREEN),
            ("tests", &tests, MONAS_TESTS_NAME_THE_SCREEN),
            ("doc links", &docs, MONAS_DOCS_LINK_THE_SCREEN),
        ] {
            let listed: BTreeSet<(String, String)> = listed
                .iter()
                .map(|(f, p)| (f.to_string(), p.to_string()))
                .collect();
            for (file, path) in found.difference(&listed) {
                problems.push(format!("{what}: {file} names {path}, and is not listed"));
            }
            for (file, path) in listed.difference(found) {
                problems.push(format!(
                    "{what}: {file} no longer names {path}: take it off the list"
                ));
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
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
        ] {
            assert!(plan_reference(cited).is_some(), "missed: {cited}");
        }
        for ordinary in [
            " the band-plan label row",
            " the sweep stops at the edge",
            " Stop the stream before retuning",
            " code table G0 and Eb/N0",
        ] {
            assert_eq!(plan_reference(ordinary), None, "flagged: {ordinary}");
        }
    }
}
