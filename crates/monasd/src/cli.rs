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
pub(crate) const VERSION: &str = match option_env!("MONASD_VERSION") {
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
/// hold each one to its source in both directions: the modes to the engine's
/// table, the routes to the server's router, the kinds of radio to the
/// parser of `--device`, the port to the default, and the config block to
/// what the config reads.
pub const SECTIONS: &[(&str, &str)] = &[
    (
        "Modes",
        "\
Each mode is named after the sdrtop view that shows its work, so a script
and the person at the terminal mean the same thing by net_survey:

  Spectrum           spectrum
  NET, 2.4 GHz       net, net_survey
  Bluetooth LE       net_ble, net_census, net_connection
  LE Coded PHY       net_coded
  Classic Bluetooth  net_bt, net_piconet, net_bench
  Wi-Fi              wifi_survey, wifi_frames, wifi_airtime
  Pagers             radio_pocsag
  FT8                radio_ft8
  FT8 Lab            lab_ft8_signal, lab_ft8_band, lab_ft8_receiver

spectrum is the default. The Lab benches and the compact views read nothing
beyond the spectrum, so they need no mode of their own.",
    ),
    (
        "API",
        "\
Everything is under http://ADDR/v1, JSON in and out:

  GET  /v1/health    the radio, streaming or not, stale or not, samples lost
  GET  /v1/radios    the radios found, and which one is open
  GET  /v1/config    the engine's settings, as they would be saved now
  POST /v1/command   one command, answered with what was done or why not
  GET  /v1/events    a WebSocket: commands applied, log lines, gaps, stalls

On loopback anyone on this machine may use it. Anywhere else every request
carries the token from the config, and monasd will not start without one:

  Authorization: Bearer TOKEN

There is no TLS: across a network you do not trust, use an SSH tunnel.
The commands, their replies and the events are described in
https://github.com/musithang/sdrtop/blob/main/user_docs/monasd.md",
    ),
    (
        "Config",
        "\
The radio's settings are shared with sdrtop, from the same file. monasd's
own are one block, and a flag wins over the line it matches:

  [monasd]
  listen = \"0.0.0.0:7373\"
  token  = \"a long random string, not this one\"
  mode   = \"net_survey\"

On exit monasd writes back the radio's settings and nothing else.",
    ),
    (
        "Examples",
        "  monasd
      the one radio plugged in, the spectrum, on 127.0.0.1:7373
  monasd --device rtlsdr --frequency 433920000 --gain 30
  monasd --mode net_survey --listen 0.0.0.0:7373
  curl -s http://127.0.0.1:7373/v1/health
  curl -s -H 'Content-Type: application/json' \\
       -d '{\"command\":\"tune\",\"hz\":433920000}' http://127.0.0.1:7373/v1/command
  websocat ws://127.0.0.1:7373/v1/events",
    ),
    (
        "Exit status",
        "  0  stopped by SIGTERM or Ctrl-C, and the radio's settings saved
  1  could not start, or could not save: the reason is the last line on
     stderr (no radio, several and no --device, a mode it does not know,
     an address it cannot use, a token missing)
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
    name = "monasd",
    version = VERSION,
    about = "Monas with no screen: one radio, its receivers, and an HTTP API",
    long_about = "\
monasd runs Monas, the engine under sdrtop, with no screen. It opens one
radio, computes what one of sdrtop's views would compute, and serves the
radio's state over HTTP and a WebSocket until SIGTERM or Ctrl-C. Then it
stops the radio and saves the radio's settings.

It reads the same config file as sdrtop, so a radio set up in the terminal
is set up here too. The log goes to stderr, a line per line, for a terminal,
a pipe or the journal.",
    after_help = "The modes, the API, the config block and examples: monasd --help, or man monasd.",
    after_long_help = after_long_help(),
)]
pub struct Cli {
    /// The config file, shared with sdrtop (default ~/.config/sdrtop/config.toml)
    ///
    /// monasd reads the radio's settings and its own [monasd] block from it,
    /// and on exit writes back the radio's settings only. A file that does not
    /// exist yet is fine: the defaults are used, and it is written on exit.
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Which kind of radio: hackrf, rtlsdr, airspy, airspyhf, sdrplay, plutosky, tinysa, soapy
    ///
    /// Without it monasd looks for every kind it can open. With one radio
    /// found it opens that one; with several it refuses and names them, since
    /// there is nobody at a terminal to ask. tinysa takes a serial port and an
    /// input (tinysa=/dev/ttyACM0?input=low), soapy takes SoapySDR's device
    /// arguments (soapy=driver=lime).
    #[arg(long, value_name = "KIND[=SELECTOR]")]
    pub device: Option<String>,

    /// Center frequency in Hz, e.g. 433920000; overrides the config
    #[arg(long, value_name = "HZ")]
    pub frequency: Option<u64>,

    /// Gain: a total in dB, or stages by name like LNA=28,VGA=12; overrides the config
    ///
    /// A total is spread over the stages the way sdrtop's gain knob spreads
    /// it. The stage names are the radio's own, as sdrtop's Gain panel shows
    /// them.
    #[arg(long, value_name = "DB")]
    pub gain: Option<String>,

    /// The first gain stage in dB, whatever the radio calls it (HackRF LNA, RTL-SDR tuner)
    #[arg(long, value_name = "DB")]
    pub lna: Option<u32>,

    /// The second gain stage in dB, on a radio that has one
    #[arg(long, value_name = "DB")]
    pub vga: Option<u32>,

    /// Where to serve the API, host:port (default 127.0.0.1:7373)
    ///
    /// Anywhere but loopback needs a token in the config's [monasd] block:
    /// the API retunes the radio, and monasd will not offer that to anyone
    /// who can reach the port. The block's listen line sets it too.
    #[arg(long, value_name = "ADDR")]
    pub listen: Option<String>,

    /// What to compute, by the name of the sdrtop view that shows it (default spectrum)
    ///
    /// The names are listed under Modes below; a name monasd does not know is
    /// refused with the list. The [monasd] block's mode line sets it too.
    #[arg(long, value_name = "MODE")]
    pub mode: Option<String>,
}
