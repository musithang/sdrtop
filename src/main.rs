mod app;
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
use config::AppConfig;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "sdrtop", about = "HackRF One / RTL-SDR terminal monitor")]
struct Cli {
    /// Path to config file (default: ~/.config/sdrtop/config.toml)
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Pick the backend when more than one device type is connected
    #[arg(long, value_name = "hackrf|rtlsdr")]
    device: Option<String>,

    /// Center frequency in Hz, e.g. 433920000 (overrides config)
    #[arg(long, value_name = "HZ")]
    frequency: Option<u64>,

    /// Primary front-end gain in dB - HackRF LNA / RTL-SDR tuner (overrides config)
    #[arg(long, value_name = "DB")]
    gain: Option<u32>,

    /// HackRF LNA gain in dB, 0–40 step 8 (overrides config)
    #[arg(long)]
    lna: Option<u32>,

    /// HackRF VGA gain in dB, 0–62 step 2 (overrides config)
    #[arg(long)]
    vga: Option<u32>,

    /// Color theme: a built-in (sdr, nord, dracula, gruvbox, catppuccin,
    /// solarized) or the name of a file in ~/.config/sdrtop/themes/
    #[arg(long, value_name = "THEME")]
    theme: Option<String>,
}

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

    let mut devices = hardware::list_all_devices();
    if let Some(kind) = &cli.device {
        let want = match kind.to_ascii_lowercase().as_str() {
            "hackrf" => hardware::DeviceKind::HackRf,
            "rtlsdr" | "rtl-sdr" | "rtl" => hardware::DeviceKind::RtlSdr,
            other => {
                eprintln!("Unknown --device '{}' (use 'hackrf' or 'rtlsdr')", other);
                std::process::exit(1);
            }
        };
        devices.retain(|d| d.kind == want);
    }
    if devices.is_empty() {
        eprintln!("No SDR device found. Connect a HackRF or RTL-SDR and try again.");
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
