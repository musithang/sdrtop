// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

// The command line. Kept in its own file because two things read it: `main.rs`
// parses with it, and `build.rs` `include!`s this file to render the man page
// from the same `Parser` derive.
//
// So this file stays self-contained - no `crate::` paths, no imports beyond what
// a build script can also satisfy, and no `//!` module docs, which are invalid
// through an `include!`. The module's documentation lives on `mod cli;` in
// `main.rs` instead.

// ASCII ranges (`0-40`, not an en dash) on purpose: these strings become the
// man page, and a bare `groff -man` pipeline without preconv renders a
// non-ASCII dash as mojibake. `man(1)` itself gets it right; not everything
// that reads a man page is `man(1)`.
use clap::Parser;
use std::path::PathBuf;

// `version` is not decoration: it is the first thing anyone pastes into a bug
// report, and a packaged binary that cannot say which build it is makes every
// report ambiguous. The crate version alone stopped being enough once
// install.sh could install from `main` as readily as from a release, so
// `build.rs` folds the commit in and this prints `0.4.1 (2ec9491)`.
//
// `option_env!` rather than `env!`, and that is forced: `build.rs` `include!`s
// this file, and a build script is compiled before it runs, so before any
// `cargo:rustc-env` it emits exists. `env!` would refuse to compile there.
// Inside the build script this is therefore always the fallback arm, which is
// correct: the man page carries the plain version and no commit.
pub(crate) const VERSION: &str = match option_env!("SDRTOP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(
    name = "sdrtop",
    version = VERSION,
    about = "SDR terminal monitor: HackRF One, RTL-SDR, and SoapySDR devices"
)]
pub struct Cli {
    /// Path to config file (default: ~/.config/sdrtop/config.toml)
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Pick the backend, optionally with SoapySDR device args (soapy=driver=airspy)
    #[arg(long, value_name = "hackrf|rtlsdr|soapy[=args]")]
    pub device: Option<String>,

    /// Center frequency in Hz, e.g. 433920000 (overrides config)
    #[arg(long, value_name = "HZ")]
    pub frequency: Option<u64>,

    /// Primary front-end gain in dB - HackRF LNA / RTL-SDR tuner (overrides config)
    #[arg(long, value_name = "DB")]
    pub gain: Option<u32>,

    /// HackRF LNA gain in dB, 0-40 step 8 (overrides config)
    #[arg(long)]
    pub lna: Option<u32>,

    /// HackRF VGA gain in dB, 0-62 step 2 (overrides config)
    #[arg(long)]
    pub vga: Option<u32>,

    /// Color theme: a built-in (sdr, nord, dracula, gruvbox, catppuccin,
    /// solarized) or the name of a file in ~/.config/sdrtop/themes/
    #[arg(long, value_name = "THEME")]
    pub theme: Option<String>,
}
