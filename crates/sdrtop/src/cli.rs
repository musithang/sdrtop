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

/// The sections `--help` adds after the flags, and the man page after its
/// OPTIONS: a heading and its text, the same words in both.
///
/// Laid out for an 80-column terminal by hand, because the tables are tables:
/// the body's lines are kept as written, indented by two in `--help`, set
/// without filling in the man page. ASCII only, for the reason above.
///
/// Every list here is the code's list written out, and the tests in `main.rs`
/// hold each one to its source: the keys to the key handler, the files to the
/// paths the program uses, the themes and the kinds of radio to theirs.
pub const SECTIONS: &[(&str, &str)] = &[
    (
        "First keys",
        "  Esc        the menu: every view, the radio, the Lab, the options, the keys
  Space      start or stop receiving
  f          type a frequency, in MHz
  s          type a sample rate
  Up, Down   more or less gain
  a          the front end's boost (the HackRF's amplifier, the RTL-SDR's AGC)
  1 to 9     the views of the group you are in
  Tab        show or hide the footer, which lists the keys of the view
  q          quit, saving the settings

Every other key is in the menu's Keys tile (K), and in
https://github.com/musithang/sdrtop/blob/main/user_docs/keys.md",
    ),
    (
        "Files",
        "  ~/.config/sdrtop/config.toml   the settings, written back on quit
  ~/.config/sdrtop/themes/       your own themes
  ~/.config/sdrtop/presets/      your own layouts
  ~/.config/sdrtop/sdrtop.log    what the radio libraries said while it ran
  ~/.local/share/sdrtop/         recordings and exports

Recordings go to $XDG_DATA_HOME/sdrtop/ instead when that is set, and with
--config, themes/ and presets/ are looked for beside that file.",
    ),
    (
        "Examples",
        "  sdrtop
      the radio plugged in; with several, a list to pick from
  sdrtop --device rtlsdr --frequency 100000000
  sdrtop --gain LNA=24,VGA=30 --theme nord
  sdrtop --device soapy=driver=lime
  sdrtop --device tinysa=/dev/ttyACM0?input=low
  sdrtop --config ~/bench.toml
      a second set of settings, kept apart from the everyday ones",
    ),
    (
        "Exit status",
        "  0  quit, or no radio picked from the list
  1  could not start: the reason is the last line printed (no radio found,
     the radio would not open, a --device it does not know)
  2  a flag it does not understand",
    ),
];

/// [`SECTIONS`] as `--help` prints them after the flags: each heading as
/// clap prints its own, the text indented under it.
pub fn after_long_help() -> String {
    SECTIONS
        .iter()
        .map(|(heading, body)| {
            let body: Vec<String> = body
                .lines()
                .map(|l| {
                    if l.is_empty() {
                        String::new()
                    } else {
                        format!("  {l}")
                    }
                })
                .collect();
            format!("{heading}:\n{}", body.join("\n"))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[derive(Parser)]
#[command(
    name = "sdrtop",
    version = VERSION,
    about = "An SDR monitor for the terminal: spectrum, waterfall, decoders and measurements",
    long_about = "\
sdrtop opens a radio and shows what it hears, in the terminal: the spectrum
and the waterfall, the 2.4 GHz band (Bluetooth and Wi-Fi), FT8, pagers,
an FM and AM demodulator's readings, and a Lab that measures the radio
itself. Every number on screen says where it came from, and a number it
cannot stand behind is not shown.

Radios: HackRF One, RTL-SDR, Airspy R2, Airspy HF+ Discovery, SDRplay RSP1B,
PlutoSky R2, tinySA, and anything SoapySDR opens.",
    after_help = "The first keys, the files and examples: sdrtop --help, or man sdrtop.",
    after_long_help = after_long_help(),
)]
pub struct Cli {
    /// The config file (default ~/.config/sdrtop/config.toml)
    ///
    /// Read at start and written back on quit. A file that does not exist yet
    /// is fine: the defaults are used, and it is written on quit. Themes and
    /// presets are looked for in the folder it is in.
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Which kind of radio: hackrf, rtlsdr, airspy, airspyhf, sdrplay, plutosky, tinysa, soapy
    ///
    /// Without it sdrtop looks for every kind it can open, and with several
    /// found offers a list. tinysa takes a serial port and an input
    /// (tinysa=/dev/ttyACM0?input=low), soapy takes SoapySDR's device
    /// arguments (soapy=driver=lime).
    #[arg(long, value_name = "KIND[=SELECTOR]")]
    pub device: Option<String>,

    /// Center frequency in Hz, e.g. 433920000; overrides the config
    #[arg(long, value_name = "HZ")]
    pub frequency: Option<u64>,

    /// Gain: a total in dB, or stages by name like LNA=28,VGA=12; overrides the config
    ///
    /// A total is spread over the stages the way the gain keys spread it. The
    /// stage names are the radio's own, as the Gain panel shows them.
    #[arg(long, value_name = "DB")]
    pub gain: Option<String>,

    /// The first gain stage in dB, whatever the radio calls it (HackRF LNA, RTL-SDR tuner)
    #[arg(long, value_name = "DB")]
    pub lna: Option<u32>,

    /// The second gain stage in dB, on a radio that has one
    #[arg(long, value_name = "DB")]
    pub vga: Option<u32>,

    /// Colour theme: sdr, nord, dracula, gruvbox, catppuccin, solarized, or one of your own
    ///
    /// Your own is a file in the themes folder beside the config file,
    /// named without its .toml; it is looked for before the built-ins.
    #[arg(long, value_name = "THEME")]
    pub theme: Option<String>,
}
