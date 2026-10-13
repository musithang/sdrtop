// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Wi-Fi's receiver, as the worker last left it: where it runs, or why not.

/// The latest frame of this channel the receiver synchronised to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WifiFrame {
    /// Its offset from our tuning, with its uncertainty; our own
    /// oscillator's error is in it until a reference takes it out.
    pub offset_hz: crate::signal::dsp::uncertainty::Uncertain,
    /// The per-sample SNR its long training field gave, in dB.
    pub snr_db: f64,
    /// The channel centre it was heard on, which the offset is a fraction
    /// of.
    pub carrier_hz: f64,
    /// What the symbols after its long training field said: its L-SIG and
    /// its format.
    pub heard: crate::signal::wifi::format::Heard,
    pub when: std::time::Instant,
}

/// Where Wi-Fi's receiver runs, or why it does not, published by the
/// worker's Wi-Fi part (`signal::net::worker`'s `wifi`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WifiState {
    /// The Wi-Fi channel the receiver runs on now, when it runs.
    pub channel: Option<u8>,
    /// Why it does not run, when a Wi-Fi view is open and it does not: the
    /// rate, or a tuning off every channel centre. `None` off the Wi-Fi
    /// views, where not running is not a refusal.
    pub refused: Option<String>,
    /// Whether it has run at all this session. Decode health names Wi-Fi
    /// only then, as it does LE Coded: a line for a receiver this session
    /// never started would offer one the view on screen does not start.
    pub ran: bool,
    /// The latest frame of this channel, kept across a stand-down so the
    /// panel can still say what it last saw, dated.
    pub last_frame: Option<WifiFrame>,
}
