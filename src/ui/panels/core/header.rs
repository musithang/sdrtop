// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::chrome::frame;
use crate::ui::panel::{Panel, PanelChrome};
use crate::ui::widgets::band_plan::band_at;
use crate::ui::widgets::charts::gain_bar_colored;

pub struct HeaderPanel;

/// A "breathing" RX status dot that cycles small→large→small on a ~0.9 s loop.
/// Pure glyph animation - the badge colours never change. All four glyphs are a
/// single terminal column, so the badge width (and the header gap math) is fixed.
/// Only animates while frames are flowing (RX), which is exactly when the UI
/// is already redrawing, so it costs no extra wakeups.
fn rx_pulse_glyph() -> &'static str {
    use std::time::{SystemTime, UNIX_EPOCH};
    const FRAMES: [&str; 4] = ["\u{2219}", "\u{2022}", "\u{25CF}", "\u{2022}"]; // ∙ • ● •
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    FRAMES[((ms / 220) % FRAMES.len() as u128) as usize]
}

/// A dot-leader span that fills `gap` columns: ` ······ ` (one space at each
/// end, dim dots between), connecting a left field to a right one like an
/// engraved instrument readout. Falls back to plain spaces when too short.
fn leader(gap: usize, color: ratatui::style::Color) -> Span<'static> {
    if gap >= 4 {
        Span::styled(
            format!(" {} ", "·".repeat(gap - 2)),
            Style::default().fg(color),
        )
    } else {
        Span::raw(" ".repeat(gap))
    }
}

/// Returns (filled_str, empty_str). Each string is exactly `n` terminal columns.
/// Uses continuous ⅛-block glyphs for smooth sub-cell resolution.
/// Shared with the command rail so the gain bars read identically there.
pub(crate) fn gain_bar(gain: u32, max_gain: u32, n: usize) -> (String, String) {
    crate::ui::widgets::charts::eighth_block_bar(gain, max_gain, n)
}

/// Power-of-ten exponent (in Hz) of the digit the current tuning step acts on:
/// 1 kHz→3, 10 kHz→4, 100 kHz→5, 1 MHz→6, 10 MHz→7. Coarse-but-non-decade steps
/// (5 kHz, 25 kHz, 500 kHz, 5 MHz) collapse onto their leading digit's place,
/// which is the digit a user reads as "the one I'm moving".
fn step_place_exp(step_hz: u64) -> u32 {
    let mut e = 0u32;
    let mut s = step_hz.max(1);
    while s >= 10 {
        s /= 10;
        e += 1;
    }
    e
}

/// Segmented VFO frequency readout: the MHz value rendered digit-by-digit with a
/// thin gap between every character, and the single digit the current tuning step
/// moves underlined + brightened - so you can see at a glance which place `← →`
/// will change. The decimal point is dimmed. Returns the spans; width varies with
/// The MHz readout string the VFO renders, e.g. `"145.500"`. Shared so the big
/// freq-hero formats the frequency identically and the active-digit index lines
/// up with the same characters.
pub(crate) fn vfo_string(freq_hz: u64) -> String {
    format!("{:.3}", freq_hz as f64 / 1_000_000.0)
}

/// Char index (in [`vfo_string`]) of the digit the current tuning step moves, if
/// that digit is on screen. Shared by the small VFO and the big freq-hero so they
/// underline / colour the *same* digit.
pub(crate) fn active_digit_idx(freq_hz: u64, step_hz: u64) -> Option<usize> {
    let s = vfo_string(freq_hz);
    let dot_pos = s.find('.').unwrap_or(s.len());
    let exp = step_place_exp(step_hz);
    if exp >= 6 {
        let from_right = (exp - 6) as usize; // 0 = ones-MHz digit (just left of '.')
        (from_right < dot_pos).then(|| dot_pos - 1 - from_right)
    } else if (3..=5).contains(&exp) {
        Some(dot_pos + 1 + (5 - exp) as usize) // 5→.1xx, 4→..1x, 3→...1
    } else {
        None
    }
}

/// the number of MHz digits (the caller measures it for layout). Shared with the
/// command rail's freq-hero so the segmented readout + active-digit cue match.
pub(crate) fn vfo_spans(
    freq_hz: u64,
    step_hz: u64,
    digit: ratatui::style::Color,
    dot: ratatui::style::Color,
    active: ratatui::style::Color,
) -> Vec<Span<'static>> {
    let s = vfo_string(freq_hz); // e.g. "145.500"
    let active_idx = active_digit_idx(freq_hz, step_hz);

    let chars: Vec<char> = s.chars().collect();
    let mut spans = Vec::with_capacity(chars.len() * 2);
    for (i, c) in chars.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if Some(i) == active_idx {
            Style::default()
                .fg(active)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else if *c == '.' {
            Style::default().fg(dot)
        } else {
            Style::default().fg(digit).add_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(c.to_string(), style));
    }
    spans
}

/// Returns the number of space characters needed between the fw-version field
/// and the right-aligned "AMP … USB …" section in the top band.
/// All length arguments are in terminal columns (chars, not bytes).
fn top_band_gap(
    board_name_len: usize,
    badge_len: usize,
    fw_value_len: usize,
    amp_val_len: Option<usize>,
    usb_val_len: usize,
    inner_width: u16,
) -> usize {
    // left side: " " + " DeviceName " + "  " + " BADGE " + "  " + "hackrf fw " + fw_val
    let left = 1 + (2 + board_name_len) + 2 + badge_len + 2 + 10 + fw_value_len;
    // right side: "AMP "(4) + amp_val + "  ·  "(5) + "USB "(4) + usb_val + "  "(2)
    //
    // `None` is a device with no front end boost, which draws neither the label
    // nor the value. Passing 0 instead would leave a four column hole at the
    // right edge, because the label is part of the field.
    let boost = amp_val_len.map_or(0, |n| 4 + n);
    let right = boost + 5 + 4 + usb_val_len + 2;
    (inner_width as usize).saturating_sub(left + right)
}

fn top_band_line(state: &SdrMetrics, theme: &crate::Theme, inner_width: u16) -> Line<'static> {
    use ratatui::style::Color;

    // --- Status badge ---
    // RX uses a breathing dot; IDLE/OBSERVER are steady. Every variant is 6
    // columns so `top_band_gap` stays valid.
    let (badge_text, badge_bg, badge_fg): (String, Color, Color) = if state.observer.active {
        (
            " ◈ OBSERVER ".to_string(),
            theme.observer,
            Color::Rgb(4, 6, 15),
        )
    } else if state.radio.hw_streaming {
        (
            format!(" {} RX ", rx_pulse_glyph()),
            theme.status_ok,
            Color::Rgb(3, 15, 6),
        )
    } else {
        (
            " ○ IDLE ".to_string(),
            theme.status_warn,
            Color::Rgb(10, 7, 0),
        )
    };
    let badge_len = badge_text.chars().count();

    // --- Firmware version + label ---
    // Mayhem nightly: "n_XXXXXX"; Mayhem release: "vX.Y.Z" → label as "mayhem fw "
    // Standard HackRF firmware ("2024.02.1", "git-...") → label as "hackrf fw "
    // Both labels are exactly 10 chars so top_band_gap stays valid.
    // Firmware field. A device with no firmware of its own names its software
    // stack instead, and **the backend says which** rather than the header
    // working it out: this used to ask whether the gain model was single-knob,
    // which meant a SoapySDR device with one gain control introduced itself as
    // an RTL-SDR the moment there was a third backend. All labels are exactly 10
    // columns so top_band_gap stays valid.
    let (fw_val, fw_label): (std::sync::Arc<str>, &str) = if let Some(stack) = &state.system.stack {
        let v: std::sync::Arc<str> = if state.observer.active {
            std::sync::Arc::from("—")
        } else {
            std::sync::Arc::clone(&stack.value)
        };
        (v, stack.label)
    } else if state.observer.active {
        (std::sync::Arc::from("—"), "hackrf fw ")
    } else {
        let is_mayhem = state.system.fw_version.starts_with("n_")
            || (state.system.fw_version.starts_with('v')
                && state
                    .system
                    .fw_version
                    .chars()
                    .nth(1)
                    .is_some_and(|c| c.is_ascii_digit()));
        let label = if is_mayhem {
            "mayhem fw "
        } else {
            "hackrf fw "
        };
        (state.system.fw_version.clone(), label)
    };
    let fw_color = if state.observer.active {
        theme.label
    } else {
        theme.value
    };
    let fw_len = fw_val.chars().count();

    // --- AMP value (always 3 terminal columns) ---
    let (amp_val, amp_color) = if state.observer.active {
        ("—  ".to_string(), theme.label)
    } else if state.radio.amp_enabled {
        ("ON ".to_string(), theme.value_hi)
    } else {
        ("OFF".to_string(), theme.label)
    };

    // --- USB value (always 9 terminal columns) ---
    let (usb_val, usb_color) = if state.radio.hw_streaming && state.radio.current_throughput_bps > 0
    {
        let mb = state.radio.current_throughput_bps as f64 / 1_000_000.0;
        (format!("{:4.1} MB/s", mb), theme.value)
    } else {
        ("—        ".to_string(), theme.label) // 1 + 8 spaces = 9 chars
    };

    // --- Gap ---
    let board_len = state.system.board_name.chars().count();
    let gap = top_band_gap(
        board_len,
        badge_len,
        fw_len,
        state.caps.gain.has_boost().then(|| amp_val.chars().count()),
        usb_val.chars().count(),
        inner_width,
    );

    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!(" {} ", state.system.board_name),
            Style::default()
                .fg(theme.value_hi)
                .bg(Color::Rgb(20, 25, 38))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            badge_text,
            Style::default()
                .fg(badge_fg)
                .bg(badge_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(fw_label, Style::default().fg(theme.label)),
        Span::styled(fw_val.to_string(), Style::default().fg(fw_color)),
        leader(gap, theme.border_dim),
        // HackRF's RF amp or RTL-SDR's tuner AGC - both 3-char labels, so the
        // "{label} " field stays 4 columns and top_band_gap remains valid.
        //
        // A device with neither gets neither, the same as the rail and the
        // micro gain view: an `AGC OFF` for a stage that is not in the radio
        // sends the reader looking for the key that turns it on.
        Span::styled(
            if state.caps.gain.has_boost() {
                format!("{} ", state.caps.gain.boost_label())
            } else {
                String::new()
            },
            Style::default().fg(theme.label),
        ),
        Span::styled(
            if state.caps.gain.has_boost() {
                amp_val
            } else {
                String::new()
            },
            Style::default().fg(amp_color),
        ),
        Span::raw("  ·  "),
        Span::styled("USB ", Style::default().fg(theme.label)),
        Span::styled(usb_val, Style::default().fg(usb_color)),
        Span::raw("  "),
    ])
}

/// Compact frequency label for the tuning-range end-caps: "1M", "145M", "1.8G",
/// "6G", "24M", "300k". Whole GHz values drop the decimal ("6G", not "6.0G").
fn fmt_freq_compact(hz: u64) -> String {
    if hz >= 1_000_000_000 {
        let g = hz as f64 / 1e9;
        if (g - g.round()).abs() < 0.05 {
            format!("{:.0}G", g)
        } else {
            format!("{:.1}G", g)
        }
    } else if hz >= 1_000_000 {
        format!("{:.0}M", hz as f64 / 1e6)
    } else if hz >= 1_000 {
        format!("{}k", hz / 1_000)
    } else {
        format!("{hz}")
    }
}

/// Logarithmic position (0..1) of `freq` within the tunable range `[min,max]`.
/// Log, because a receiver's span is enormous (MHz…GHz) - a linear bar would
/// Perceptual exponent for the tuning-dial position. A pure-log axis pushes the
/// low end too far right (1 MHz…120 MHz already eats ~55 % of the bar, then the
/// whole GHz range crawls in the remaining 45 %). A pure-linear axis does the
/// opposite - it crushes everything below ~1 GHz into the first columns. `0.4`
/// is the middle ground: it spreads VHF/UHF readably while still moving the
/// needle at a steady pace up into the GHz range.
const DIAL_GAMMA: f64 = 0.4;

/// Position (0..1) of `freq` within the tunable range `[min,max]` on a `γ`-power
/// axis (see [`DIAL_GAMMA`]). Clamped to the range.
fn range_frac(freq: u64, min: u64, max: u64) -> f64 {
    let lo = (min.max(1) as f64).powf(DIAL_GAMMA);
    let hi = (max.max(1) as f64).powf(DIAL_GAMMA);
    if hi <= lo {
        return 0.0;
    }
    let f = (freq as f64)
        .clamp(min as f64, max as f64)
        .max(1.0)
        .powf(DIAL_GAMMA);
    ((f - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// A plain ruled `├───────┤` rule - fallback for terminals too narrow to fit the
/// live tuning strip.
fn plain_separator(theme: &crate::Theme, outer_width: u16) -> Line<'static> {
    let fill = (outer_width as usize).saturating_sub(2);
    Line::from(vec![
        Span::styled("├", Style::default().fg(theme.border_dim)),
        Span::styled("─".repeat(fill), Style::default().fg(theme.border_default)),
        Span::styled("┤", Style::default().fg(theme.border_dim)),
    ])
}

/// The header's central rule, repurposed from a static "FREQUENCY" label into a
/// live tuning dial: a `γ`-power position bar across the device's whole tunable
/// range, end-capped by the range limits, with a lit `━` rail behind a `◆` needle
/// at the current frequency - and the **band name riding the needle** (e.g.
/// `◆╴2m╶`), so the band you're in sits exactly where the eye lands. `outer_width`
/// is the FULL panel width; rendered at the outer Rect so `├`/`┤` overwrite `│`.
fn band_strip_line(state: &SdrMetrics, theme: &crate::Theme, outer_width: u16) -> Line<'static> {
    if state.ui.is_net_section() {
        return net_band_strip(state, theme, outer_width);
    }
    // Inside NET the rail spans the band being worked, not the radio's whole
    // tuning range. A HackRF reaches 6 GHz, so on its own rail every channel in
    // the 2.4 GHz band lands in the same column and the marker never moves. The
    // rail is there to show where you are; over the wrong range it shows nothing.
    // The compact formatter speaks in decades and cannot say 2483.5: it renders
    // the two edges as "2.4G" and "2.5G", which look like a hundred megahertz
    // apart and put the top of the band above where it ends. Inside NET the
    // labels are megahertz.
    let (fmin, fmax, lo, hi) = if state.ui.is_net_section() {
        use crate::signal::net::band::{HIGH_HZ, LOW_HZ};
        (
            LOW_HZ,
            HIGH_HZ,
            format!("{}", LOW_HZ / 1_000_000),
            format!("{}", HIGH_HZ / 1_000_000),
        )
    } else {
        (
            state.caps.freq_min_hz,
            state.caps.freq_max_hz,
            fmt_freq_compact(state.caps.freq_min_hz),
            fmt_freq_compact(state.caps.freq_max_hz),
        )
    };
    compose_band_strip(
        state.radio.frequency,
        fmin,
        fmax,
        lo,
        hi,
        theme,
        outer_width,
    )
}

/// The NET section's band strip: the 2.4 GHz band, with what the radio sees
/// of it now and which of that the decoders are listening to.
///
/// **What is seen, not only where the needle is.** Elsewhere the strip is a
/// dial, lit up to the tuning. In NET the question is what the receiver is
/// covering, so the window it sees (the tuning, give or take half the span)
/// is the lit stretch; the classic channels being watched are drawn over it
/// in their own ink and the BLE decoder's channel is a dot in its own, the
/// same two inks and the same `●` the coexistence history marks their hits
/// with. In SURVEY the window walks the band as the survey does.
fn net_band_strip(state: &SdrMetrics, theme: &crate::Theme, outer_width: u16) -> Line<'static> {
    use crate::signal::net::band::{HIGH_HZ, LOW_HZ};
    let lo_lbl = format!("{}", LOW_HZ / 1_000_000);
    let hi_lbl = format!("{}", HIGH_HZ / 1_000_000);
    let left_w = 1 + 1 + 1 + lo_lbl.chars().count() + 1;
    let right_w = 1 + hi_lbl.chars().count() + 1 + 1 + 1;
    let track_w = (outer_width as usize).saturating_sub(left_w + right_w);
    if track_w < 8 {
        return plain_separator(theme, outer_width);
    }
    let col = |hz: f64| -> usize {
        ((range_frac(hz.max(0.0) as u64, LOW_HZ, HIGH_HZ) * (track_w - 1) as f64).round() as usize)
            .min(track_w - 1)
    };
    let tuned = state.radio.frequency as f64;
    let span = if state.radio.bb_filter_hz > 0 {
        (state.radio.bb_filter_hz as f64).min(state.radio.config_sample_rate)
    } else {
        state.radio.config_sample_rate
    };
    let (a, b) = (col(tuned - span / 2.0), col(tuned + span / 2.0));
    let watched = &state.net.bt_channels_watched;
    let bt_cols = match (watched.iter().min(), watched.iter().max()) {
        (Some(&lo), Some(&hi)) => {
            let edge = |ch: u8| crate::signal::bt::channel::centre_hz(ch).map(|hz| hz as f64);
            match (edge(lo), edge(hi)) {
                (Some(l), Some(h)) => Some((col(l - 400e3), col(h + 400e3))),
                _ => None,
            }
        }
        _ => None,
    };
    let ble_col = state
        .net
        .ble_channel
        .and_then(crate::signal::ble::channel::centre_hz)
        .map(|hz| col(hz as f64));

    let bold = |c| {
        Style::default()
            .fg(c)
            .add_modifier(ratatui::style::Modifier::BOLD)
    };
    let mut spans = vec![
        Span::styled("├", Style::default().fg(theme.border_dim)),
        Span::styled("─", Style::default().fg(theme.border_default)),
        Span::raw(" "),
        Span::styled(lo_lbl, Style::default().fg(theme.label)),
        Span::raw(" "),
    ];
    for c in 0..track_w {
        let in_view = (a..=b).contains(&c);
        spans.push(if Some(c) == ble_col {
            Span::styled("\u{25cf}", bold(theme.net_ble))
        } else if in_view && bt_cols.is_some_and(|(l, h)| (l..=h).contains(&c)) {
            Span::styled("\u{2501}", bold(theme.net_bt))
        } else if in_view {
            Span::styled("\u{2501}", bold(theme.border_accent))
        } else {
            Span::styled("\u{2508}", Style::default().fg(theme.border_dim))
        });
    }
    spans.extend([
        Span::raw(" "),
        Span::styled(hi_lbl, Style::default().fg(theme.label)),
        Span::raw(" "),
        Span::styled("─", Style::default().fg(theme.border_default)),
        Span::styled("┤", Style::default().fg(theme.border_dim)),
    ]);
    Line::from(spans)
}

/// The NET section's bottom band: what the receiver is doing, in place of the
/// gain staging and tuning the normal header shows.
///
/// Design section 9.2. In this section the tuning and the gain are the least
/// interesting things on the screen, and they have a panel of their own; what
/// the user needs continuously is the mode, because `SURVEY` and `LOCK` mean
/// different things about every number below them - then which decoder is
/// running and on what channel, whether the feed has been interrupted, and how
/// much of the worker's time the decoding costs.
fn net_band_line(state: &SdrMetrics, theme: &crate::Theme, inner_width: u16) -> Line<'static> {
    let value = Style::default().fg(theme.value);
    let health = &state.net.health;
    let mut fields = channel_fields(state, theme);
    fields.push(BandField::new(
        format!("{:.3} MHz", state.radio.frequency as f64 / 1e6),
        value,
        2,
    ));
    fields.push(BandField::new(
        format!("{:.3} Msps", state.radio.config_sample_rate / 1e6),
        value,
        1,
    ));
    if let Some(load) = health.decode_load {
        // Past one the worker is falling behind the stream and the feed will
        // start refusing blocks: that is a fault, not a figure to note.
        let style = if load > 1.0 {
            Style::default().fg(theme.status_crit)
        } else {
            value
        };
        fields.push(BandField::new(
            format!("decode {}", load_text(load)),
            style,
            3,
        ));
    }
    // Before the first block there is no feed to have been interrupted, and
    // "gaps 0" would claim a look that has not happened.
    if health.last_block.is_some() {
        let style = if health.gaps > 0 {
            Style::default().fg(theme.status_warn)
        } else {
            value
        };
        fields.push(BandField::new(format!("gaps {}", health.gaps), style, 4));
    }
    compose_net_band(state.net.mode, &fields, theme, inner_width)
}

/// A decode load as the band shows it: a percentage while it is one a reader
/// can take in, a multiple of real time once it is not.
///
/// The first live run read `decode 180742%` - true, and useless at a glance.
/// Past ten times real time the digits of a percentage are all a reader sees;
/// `1807×` says the same thing in the size a header field has room for, and
/// keeps the magnitude that tells a steady overload from a stall.
fn load_text(load: f64) -> String {
    if load < 10.0 {
        format!("{:.0}%", load * 100.0)
    } else {
        format!("{load:.0}\u{d7}")
    }
}

/// The channels, in the numbering of whatever is being received on them.
///
/// A radio parked on 2402 MHz for BLE is on BLE channel 37; calling it by the
/// nearest Wi-Fi number, as the band line once did, names a channel nobody is
/// listening to. So a running decoder names its own channel, each as its own
/// field: BLE's single one with whether it is an advertising or a data channel
/// (which is why a list can be quiet), and the span of classic channels being
/// watched. Each wears the mark and ink the coexistence history gives that
/// decoder's hits, `●` and `■`, as the band strip above does. Only with no
/// decoder running does the band fall back to Wi-Fi's numbering, the one
/// everyone reads 2.4 GHz in. Between channels of every scheme there is no
/// channel, and nothing is said rather than the nearest one rounded to.
fn channel_fields(state: &SdrMetrics, theme: &crate::Theme) -> Vec<BandField> {
    let value = Style::default().fg(theme.value);
    let label = Style::default().fg(theme.label);
    let mut out = Vec::new();
    if let Some(ch) = state.net.ble_channel {
        let kind = if crate::signal::ble::channel::advertising_channel_index(ch).is_some() {
            "adv"
        } else {
            "data"
        };
        out.push(BandField::spans(
            vec![
                Span::styled("\u{25cf}", Style::default().fg(theme.net_ble)),
                Span::styled(format!(" BLE {ch} "), value),
                Span::styled(kind, label),
            ],
            5,
        ));
    }
    let watched = &state.net.bt_channels_watched;
    if let (Some(lo), Some(hi)) = (watched.iter().min(), watched.iter().max()) {
        let span = if lo == hi {
            format!(" BT {lo}")
        } else {
            format!(" BT {lo}\u{2013}{hi}")
        };
        out.push(BandField::spans(
            vec![
                Span::styled("\u{25a0}", Style::default().fg(theme.net_bt)),
                Span::styled(span, value),
            ],
            5,
        ));
    }
    if out.is_empty() {
        if let Some(ch) = crate::signal::net::band::wifi_channel(state.radio.frequency) {
            out.push(BandField::new(format!("Wi-Fi ch {ch}"), value, 5));
        }
    }
    out
}

/// One field of the NET band: its text, its style, and how long it holds its
/// place as the line narrows - higher holds longer.
struct BandField {
    spans: Vec<Span<'static>>,
    keep: u8,
}

impl BandField {
    fn new(text: String, style: Style, keep: u8) -> Self {
        Self::spans(vec![Span::styled(text, style)], keep)
    }

    /// A field in more than one ink, kept or dropped whole.
    fn spans(spans: Vec<Span<'static>>, keep: u8) -> Self {
        Self { spans, keep }
    }

    fn width(&self) -> usize {
        self.spans.iter().map(|s| s.content.chars().count()).sum()
    }

    #[cfg(test)]
    fn text(&self) -> String {
        self.spans.iter().map(|s| s.content.as_ref()).collect()
    }
}

/// Pure core of [`net_band_line`], taking the fields already built so the
/// widths can be tested without a `SdrMetrics`.
///
/// **Fields give way least-important first, not simply from the right.** They
/// are drawn in reading order, but when the line is short the ones that
/// matter least go first: the sample rate, then the frequency (both are on
/// the radio's own panel), then the decode load, then the gap count. The
/// channel goes last, and `NET` and the mode never go, because a header that
/// has stopped saying which mode is running is worse than no header.
fn compose_net_band(
    mode: crate::state::NetMode,
    fields: &[BandField],
    theme: &crate::Theme,
    inner_width: u16,
) -> Line<'static> {
    use ratatui::style::Modifier;

    const SEP: &str = " \u{b7} ";
    let sep_w = SEP.chars().count();
    let mut width = 1 + 3 + sep_w + mode.label().len(); // " NET" + sep + mode

    let mut by_importance: Vec<usize> = (0..fields.len()).collect();
    by_importance.sort_by_key(|&i| std::cmp::Reverse(fields[i].keep));
    let mut kept = vec![false; fields.len()];
    for i in by_importance {
        let next = width + sep_w + fields[i].width();
        if next <= inner_width as usize {
            width = next;
            kept[i] = true;
        }
    }

    let mut spans = vec![
        Span::styled(" NET", Style::default().fg(theme.border_accent)),
        Span::styled(SEP, Style::default().fg(theme.label)),
        Span::styled(
            mode.label().to_string(),
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    for (field, _) in fields.iter().zip(&kept).filter(|(_, k)| **k) {
        spans.push(Span::styled(SEP, Style::default().fg(theme.label)));
        spans.extend(field.spans.iter().cloned());
    }
    Line::from(spans)
}

/// Pure core of [`band_strip_line`] - takes the tuned frequency and tunable range
/// directly so it can be unit-tested without a full `SdrMetrics`.
fn compose_band_strip(
    freq: u64,
    fmin: u64,
    fmax: u64,
    lo_lbl: String,
    hi_lbl: String,
    theme: &crate::Theme,
    outer_width: u16,
) -> Line<'static> {
    let frac = range_frac(freq, fmin, fmax);

    // Fixed chrome around the track:  ├ ─ ␠ LO ␠ <track> ␠ HI ␠ ─ ┤
    let left_w = 1 + 1 + 1 + lo_lbl.chars().count() + 1;
    let right_w = 1 + hi_lbl.chars().count() + 1 + 1 + 1;
    let track_w = (outer_width as usize).saturating_sub(left_w + right_w);

    if track_w < 8 {
        return plain_separator(theme, outer_width);
    }

    let marker_col = ((frac * (track_w - 1) as f64).round() as usize).min(track_w - 1);

    let dim = theme.border_dim;
    let track = theme.border_default;

    let mut spans = vec![
        Span::styled("├", Style::default().fg(dim)),
        Span::styled("─", Style::default().fg(track)),
        Span::raw(" "),
        Span::styled(lo_lbl, Style::default().fg(theme.label)),
        Span::raw(" "),
    ];
    spans.extend(rail_spans(track_w, marker_col, band_at(freq), theme));
    spans.extend([
        Span::raw(" "),
        Span::styled(hi_lbl, Style::default().fg(theme.label)),
        Span::raw(" "),
        Span::styled("─", Style::default().fg(track)),
        Span::styled("┤", Style::default().fg(dim)),
    ]);
    Line::from(spans)
}

/// The lit-rail dial itself, exactly `track_w` columns: a bright heavy `━` rule up
/// to the `◆` needle, then a faint dashed `┈` rule, with the band-name callout
/// `╴NAME╶` placed against the needle (to its right if it fits, else its left).
/// Position is double-encoded - brightness *and* line weight - so it reads at a
/// glance, and the band label sits right at the needle.
fn rail_spans(
    track_w: usize,
    marker_col: usize,
    band: Option<&'static str>,
    theme: &crate::Theme,
) -> Vec<Span<'static>> {
    let heavy = Style::default().fg(theme.border_accent);
    let faint = Style::default().fg(theme.border_dim);
    let mark = Style::default()
        .fg(theme.value_hi)
        .add_modifier(Modifier::BOLD);
    let cap = Style::default().fg(theme.border_accent);
    let name = Style::default()
        .fg(theme.value_hi)
        .add_modifier(Modifier::BOLD);

    let callout: Vec<Span<'static>> = match band {
        Some(b) => vec![
            Span::styled("╴", cap),
            Span::styled(b, name),
            Span::styled("╶", cap),
        ],
        None => Vec::new(),
    };
    let cw: usize = callout.iter().map(|s| s.width()).sum();

    let heavy_run = |n: usize| Span::styled("━".repeat(n), heavy);
    let faint_run = |n: usize| Span::styled("┈".repeat(n), faint);
    let needle = || Span::styled("◆", mark);

    let mut spans = Vec::with_capacity(6);
    if cw > 0 && marker_col + 1 + cw <= track_w {
        // Callout to the RIGHT of the needle.
        spans.push(heavy_run(marker_col));
        spans.push(needle());
        spans.extend(callout);
        spans.push(faint_run(track_w - marker_col - 1 - cw));
    } else if cw > 0 && marker_col >= cw {
        // No room on the right - tuck the callout to the LEFT of the needle.
        spans.push(heavy_run(marker_col - cw));
        spans.extend(callout);
        spans.push(needle());
        spans.push(faint_run(track_w - marker_col - 1));
    } else {
        // Between bands (or no room): just the lit rail + needle.
        spans.push(heavy_run(marker_col));
        spans.push(needle());
        spans.push(faint_run(track_w - marker_col - 1));
    }
    spans
}

/// Frequency · sample-rate on the left, gain bars right-aligned. Left block
/// (freq + SR): 31 chars. Right block: 42 chars - either HackRF's LNA + VGA, or a
/// single-tuner stage (RTL-SDR) with the second-stage region blanked to the same
/// width so the gap math and right-alignment hold for both.
fn bottom_band_line(state: &SdrMetrics, theme: &crate::Theme, inner_width: u16) -> Line<'static> {
    let active = state.radio.hw_streaming && !state.observer.active;
    let gm = &state.caps.gain;

    let sr_str = format!("{:4.1}", state.radio.config_sample_rate / 1_000_000.0);
    let (bandwidth_label, bandwidth_unit) = if state.caps.sample_rate_is_span {
        ("SPAN ", " MHz")
    } else {
        ("SR ", " Msps")
    };

    let freq_color = if state.observer.active {
        theme.label
    } else {
        theme.border_accent
    };
    let val_color = if active { theme.value } else { theme.label };
    let dim = theme.border_dim;

    // ⅛-block gain bar that matches the command rail: a meaning gradient while
    // streaming (LNA green→yellow, VGA cyan→orange), flat dim when idle.
    let gain_bar_spans = |gain: u32, max: u32, lo: Color, hi: Color| -> Vec<Span<'static>> {
        if active {
            gain_bar_colored(gain, max, 8, lo, hi, dim)
        } else {
            let (f, e) = gain_bar(gain, max, 8);
            vec![
                Span::styled(f, Style::default().fg(theme.label)),
                Span::styled(e, Style::default().fg(dim)),
            ]
        }
    };

    // Left block: segmented VFO readout + unit + sample-rate. Its width varies
    // with the number of MHz digits and the active-digit underline, so it is
    // measured (below) rather than assumed, and the trailing gap fills the rest.
    let mut left_spans = vec![Span::raw("  ")];
    left_spans.extend(vfo_spans(
        state.radio.frequency,
        state.spectrum.step_hz,
        freq_color,
        theme.label,
        theme.value_hi,
    ));
    left_spans.extend([
        Span::raw(" "),
        Span::styled("MHz", Style::default().fg(theme.label)),
        Span::raw("    "),
        Span::styled(bandwidth_label, Style::default().fg(theme.label)),
        Span::styled(sr_str, Style::default().fg(val_color)),
        Span::styled(bandwidth_unit, Style::default().fg(theme.label)),
    ]);
    let left_w: usize = left_spans.iter().map(|s| s.width()).sum();

    if state.caps.sample_rate_is_span {
        let gap = (inner_width as usize).saturating_sub(left_w);
        left_spans.push(leader(gap, theme.border_dim));
        return Line::from(left_spans);
    }

    // right: primary "LNA/TUN "(4) + bar(8) + " "(1) + val(2) + " dB"(3) + "    "(4)  = 22
    //      + second stage "VGA "(4) + bar(8) + " "(1) + val(2) + " dB"(3) + "  "(2)   = 20  (blank on RTL)
    let right = 22 + 20;
    let gap = (inner_width as usize).saturating_sub(left_w + right);

    // Primary stage: HackRF LNA / RTL-SDR tuner - green → yellow gradient.
    let p_str = format!("{:2}", state.shown_gain());
    let p_label = format!("{:<4}", gm.primary_label_short());

    let mut spans = left_spans;
    spans.push(leader(gap, theme.border_dim));
    spans.push(Span::styled(p_label, Style::default().fg(theme.label)));
    spans.extend(gain_bar_spans(
        state.shown_gain(),
        gm.primary_max_db(),
        theme.status_ok,
        theme.value_hi,
    ));
    spans.extend([
        Span::raw(" "),
        Span::styled(p_str, Style::default().fg(val_color)),
        Span::styled(" dB", Style::default().fg(theme.label)),
        Span::raw("    "),
    ]);

    if let (true, Some(second)) = (gm.has_second_stage(), gm.stages().get(1)) {
        // The second stage, by the name and the range the device gave it,
        // in the same four columns whatever it is called.
        let vga_str = format!("{:2}", state.radio.secondary_gain());
        let name: String = second.name.chars().take(3).collect();
        spans.push(Span::styled(
            format!("{name:<3} "),
            Style::default().fg(theme.label),
        ));
        spans.extend(gain_bar_spans(
            state.radio.secondary_gain(),
            second.max_db.max(1.0).round() as u32,
            theme.border_accent,
            theme.status_warn,
        ));
        spans.extend([
            Span::raw(" "),
            Span::styled(vga_str, Style::default().fg(val_color)),
            Span::styled(" dB", Style::default().fg(theme.label)),
            Span::raw("  "),
        ]);
    } else {
        // Single-tuner device: blank the 20-col second-stage region to keep width.
        spans.push(Span::raw(" ".repeat(20)));
    }

    Line::from(spans)
}

impl Panel for HeaderPanel {
    fn name(&self) -> &'static str {
        "header"
    }
    fn supports_acquisition(&self, _acquisition: crate::hardware::AcquisitionKind) -> bool {
        true
    }
    fn min_size(&self) -> (u16, u16) {
        (60, 5)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::deck("Radio")
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        // inner.height == 3 when the panel is 5 rows tall. Row positions:
        //   inner.y     → top band
        //   inner.y + 1 → the band strip, drawn at the *outer* width so its ├ and ┤
        //                 end caps land on the side borders and tie into the frame
        //   inner.y + 2 → bottom band
        if inner.height < 3 {
            return;
        }
        let outer = frame::outer_of(inner);
        let top_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        let sep_area = Rect {
            x: outer.x,
            y: inner.y + 1,
            width: outer.width,
            height: 1,
        };
        let bot_area = Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: 1,
        };

        f.render_widget(
            Paragraph::new(top_band_line(state, theme, inner.width)),
            top_area,
        );
        f.render_widget(
            Paragraph::new(band_strip_line(state, theme, outer.width)),
            sep_area,
        );
        let bottom = if state.ui.is_net_section() {
            net_band_line(state, theme, inner.width)
        } else {
            bottom_band_line(state, theme, inner.width)
        };
        f.render_widget(Paragraph::new(bottom), bot_area);
    }
}

/// A two-row header for the Command Rail layout: just the device-status band and
/// the γ-power tuning dial - the frequency readout and gain bars move into the
/// rail, so the header stays out of the way ("where am I in the range" context
/// only). Reuses the full header's `top_band_line` + `band_strip_line`, so the
/// two stay visually identical. Height 4 (2 inner rows).
pub struct SlimHeaderPanel;

impl Panel for SlimHeaderPanel {
    fn name(&self) -> &'static str {
        "header_slim"
    }
    fn supports_acquisition(&self, _acquisition: crate::hardware::AcquisitionKind) -> bool {
        true
    }
    fn min_size(&self) -> (u16, u16) {
        (60, 4)
    }
    fn preferred_height(&self, _w: u16, _s: &SdrMetrics) -> u16 {
        4
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::deck("Radio")
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        // inner.height == 2 when the panel is 4 rows tall:
        //   inner.y     → device-status band
        //   inner.y + 1 → tuning dial, at outer width so its ├/┤ overwrite the │
        if inner.height < 2 {
            return;
        }
        let outer = frame::outer_of(inner);
        let top_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        let dial_area = Rect {
            x: outer.x,
            y: inner.y + 1,
            width: outer.width,
            height: 1,
        };

        f.render_widget(
            Paragraph::new(top_band_line(state, theme, inner.width)),
            top_area,
        );
        f.render_widget(
            Paragraph::new(band_strip_line(state, theme, outer.width)),
            dial_area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn gain_bar_zero_gain_all_empty() {
        let (filled, empty) = gain_bar(0, 40, 8);
        assert_eq!(filled, "");
        assert_eq!(empty, " ".repeat(8));
    }

    #[test]
    fn gain_bar_full_gain_all_filled() {
        let (filled, empty) = gain_bar(40, 40, 8);
        assert_eq!(filled, "█".repeat(8));
        assert_eq!(empty, "");
    }

    #[test]
    fn step_place_exp_maps_steps_to_digit_place() {
        // decade steps land exactly on their digit
        assert_eq!(step_place_exp(1_000), 3); // 1 kHz
        assert_eq!(step_place_exp(10_000), 4); // 10 kHz
        assert_eq!(step_place_exp(100_000), 5); // 100 kHz
        assert_eq!(step_place_exp(1_000_000), 6); // 1 MHz
        assert_eq!(step_place_exp(10_000_000), 7); // 10 MHz
                                                   // non-decade steps collapse onto their leading digit's place
        assert_eq!(step_place_exp(5_000), 3);
        assert_eq!(step_place_exp(25_000), 4);
        assert_eq!(step_place_exp(500_000), 5);
        assert_eq!(step_place_exp(5_000_000), 6);
    }

    #[test]
    fn vfo_underlines_the_active_digit() {
        let t = Theme::sdr();
        // 145.500 MHz, 10 kHz step → the 10-kHz digit is the first decimal-2 ('0'
        // in ".50"). Exactly one span carries UNDERLINED.
        let spans = vfo_spans(145_500_000, 10_000, t.border_accent, t.label, t.value_hi);
        let underlined: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(underlined.len(), 1, "exactly one active digit");
        // "145.500": frac index 1 (5→exp5,4→exp4) → the '0' after the '5'
        assert_eq!(underlined[0], "0");
        // active digit is brightened, not the plain accent
        let act = spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
            .unwrap();
        assert_eq!(act.style.fg, Some(t.value_hi));
    }

    #[test]
    fn vfo_step_above_screen_underlines_nothing() {
        let t = Theme::sdr();
        // 5 MHz, 10 MHz step → tens-of-MHz digit, which doesn't exist → no underline
        let spans = vfo_spans(5_000_000, 10_000_000, t.border_accent, t.label, t.value_hi);
        let any = spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(!any, "active digit off-screen → nothing underlined");
    }

    #[test]
    fn gain_bar_half_gain() {
        let (filled, empty) = gain_bar(20, 40, 8);
        assert_eq!(filled.chars().count(), 4);
        assert_eq!(empty.chars().count(), 4);
    }

    #[test]
    fn gain_bar_total_always_equals_width() {
        for gain in [0u32, 1, 16, 20, 40] {
            let (f, e) = gain_bar(gain, 40, 8);
            assert_eq!(
                f.chars().count() + e.chars().count(),
                8,
                "gain={gain}: filled({}) + empty({}) != 8",
                f.chars().count(),
                e.chars().count()
            );
        }
    }

    #[test]
    fn top_band_gap_rx_state() {
        // HackRF One (len=10), badge " ● RX " (len=6), fw "2024.02.1" (len=9), inner=78
        // amp_val "ON " (3), usb_val "10.0 MB/s" (9)
        assert_eq!(top_band_gap(10, 6, 9, Some(3), 9, 78), 9);
    }

    #[test]
    fn top_band_gap_idle_state() {
        // badge " ○ IDLE " is 2 chars wider than RX → gap shrinks by 2
        assert_eq!(top_band_gap(10, 8, 9, Some(3), 9, 78), 7);
    }

    #[test]
    fn top_band_gap_observer_state() {
        // badge " ◈ OBSERVER " (len=12), fw "—" (len=1)
        assert_eq!(top_band_gap(10, 12, 1, Some(3), 9, 78), 11);
    }

    /// A device with no boost draws neither the label nor the value, so the gap
    /// has to grow by the whole four column field. Passing zero instead leaves a
    /// hole at the right edge, which is what the live run showed.
    #[test]
    fn the_gap_absorbs_a_boost_field_that_is_not_drawn() {
        let with = top_band_gap(10, 6, 9, Some(3), 9, 78);
        let without = top_band_gap(10, 6, 9, None, 9, 78);
        assert_eq!(without, with + 4 + 3, "the label and its value");
    }

    #[test]
    fn fmt_freq_compact_units() {
        assert_eq!(fmt_freq_compact(1_000_000), "1M");
        assert_eq!(fmt_freq_compact(145_000_000), "145M");
        assert_eq!(fmt_freq_compact(24_000_000), "24M");
        assert_eq!(fmt_freq_compact(6_000_000_000), "6G"); // whole GHz drops decimal
        assert_eq!(fmt_freq_compact(1_766_000_000), "1.8G");
        assert_eq!(fmt_freq_compact(300_000), "300k");
    }

    #[test]
    fn range_frac_endpoints_monotonic_and_clamp() {
        let (lo, hi) = (1_000_000u64, 6_000_000_000u64);
        assert!((range_frac(lo, lo, hi) - 0.0).abs() < 1e-9, "min → 0");
        assert!((range_frac(hi, lo, hi) - 1.0).abs() < 1e-9, "max → 1");
        // The whole point of the γ-power axis: the low end no longer eats half the
        // bar. 120 MHz sat at ~0.55 on a log axis; here it must be well under a
        // quarter, and the GHz range gets the room instead.
        assert!(
            range_frac(120_000_000, lo, hi) < 0.25,
            "120 MHz should sit in the lower quarter, got {}",
            range_frac(120_000_000, lo, hi)
        );
        assert!(
            range_frac(1_000_000_000, lo, hi) > 0.40,
            "1 GHz should be past the low band, got {}",
            range_frac(1_000_000_000, lo, hi)
        );
        // Strictly increasing with frequency.
        assert!(range_frac(100_000_000, lo, hi) < range_frac(1_000_000_000, lo, hi));
        assert!(range_frac(1_000_000_000, lo, hi) < range_frac(3_000_000_000, lo, hi));
        // out-of-range clamps to the ends
        assert_eq!(range_frac(500_000, lo, hi), 0.0);
        assert_eq!(range_frac(9_000_000_000, lo, hi), 1.0);
    }

    #[test]
    fn rail_spans_always_track_width() {
        // The rail must be exactly track_w columns for every marker position and
        // both with/without a band callout, so the outer width math holds.
        let t = Theme::sdr();
        for track_w in [8usize, 20, 40, 67] {
            for marker in [0usize, 1, track_w / 2, track_w - 2, track_w - 1] {
                for band in [None, Some("2m"), Some("ISM433")] {
                    let w: usize = rail_spans(track_w, marker, band, &t)
                        .iter()
                        .map(|s| s.width())
                        .sum();
                    assert_eq!(
                        w, track_w,
                        "track_w={track_w} marker={marker} band={band:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn band_strip_total_width_matches_outer() {
        // The composed strip must be exactly `outer_width` columns so the ├/┤ caps
        // land on the border and nothing is truncated or padded. Exercised across
        // an in-band frequency (named tab) and an out-of-band one (% tab).
        let t = Theme::sdr();
        for outer in [60u16, 78, 120, 200] {
            for (lbl, line) in [
                (
                    "named",
                    compose_band_strip(
                        145_500_000,
                        1_000_000,
                        6_000_000_000,
                        "1M".into(),
                        "6G".into(),
                        &t,
                        outer,
                    ),
                ),
                (
                    "percent",
                    compose_band_strip(
                        200_000_000,
                        1_000_000,
                        6_000_000_000,
                        "1M".into(),
                        "6G".into(),
                        &t,
                        outer,
                    ),
                ),
            ] {
                let w: usize = line.spans.iter().map(|s| s.width()).sum();
                assert_eq!(w, outer as usize, "{lbl} strip width at outer={outer}");
                assert_eq!(line.spans.first().unwrap().content.as_ref(), "├");
                assert_eq!(line.spans.last().unwrap().content.as_ref(), "┤");
            }
        }
    }
    /// The header names the backend, and it names the right one.
    ///
    /// It used to work this out from the gain model: a device with one gain
    /// control was an RTL-SDR. That held while there were two backends and broke
    /// the moment a HackRF reached through SoapySDR reported a single overall
    /// gain, at which point the header introduced it as `rtl-sdr librtlsdr`.
    #[test]
    fn the_header_names_the_backend_the_device_came_from() {
        let chain = crate::state::fixture::draw(
            HeaderPanel,
            120,
            8,
            &crate::state::SdrMetrics::fixture()
                .streaming()
                .named_chain(),
        )
        .join("\n");
        assert!(chain.contains("soapysdr"), "{chain}");
        assert!(!chain.contains("librtlsdr"), "not an RTL-SDR:\n{chain}");

        // And a HackRF still shows its own firmware.
        let hackrf = crate::state::fixture::draw(
            HeaderPanel,
            120,
            8,
            &crate::state::SdrMetrics::fixture().streaming(),
        )
        .join("\n");
        assert!(!hackrf.contains("soapysdr"), "{hackrf}");
    }

    /// And it does not offer a front end boost the radio does not have. This is
    /// the fourth surface that had to learn the same thing, after the rail, the
    /// micro gain view and the Keys pane.
    #[test]
    fn the_header_omits_a_boost_the_device_does_not_have() {
        let chain = crate::state::fixture::draw(
            HeaderPanel,
            120,
            8,
            &crate::state::SdrMetrics::fixture()
                .streaming()
                .named_chain_no_boost(),
        )
        .join("\n");
        assert!(!chain.contains("AGC"), "{chain}");
        assert!(!chain.contains("AMP"), "{chain}");
        assert!(
            chain.contains("USB"),
            "the rest of the band survives:\n{chain}"
        );
    }

    /// A `SdrMetrics` sitting in the NET section on Wi-Fi channel 6, which is
    /// design section 9.2's own worked example.
    fn net_fixture() -> SdrMetrics {
        let mut m = SdrMetrics::fixture();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net".to_string();
        m.radio.frequency = 2_437_000_000;
        m.radio.config_sample_rate = 20_000_000.0;
        m
    }

    /// In NET the strip lights what the radio sees: the window around the
    /// tuning, the watched classic channels in their ink within it, and the
    /// BLE decoder's channel as a dot, the whole strip exactly the frame wide.
    #[test]
    fn the_net_strip_lights_the_window_and_marks_the_decoders() {
        let theme = crate::Theme::sdr();
        let mut m = net_fixture();
        m.radio.frequency = 2_410_000_000;
        m.radio.config_sample_rate = 8_000_000.0;
        // No baseband filter narrowing it: the span is the rate.
        m.radio.bb_filter_hz = 0;
        m.net.ble_channel = Some(3);
        m.net.bt_channels_watched = (5..=11).collect();
        let line = net_band_strip(&m, &theme, 191);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 191, "{text}");
        let lit = |c: ratatui::style::Color| {
            line.spans
                .iter()
                .filter(|s| s.content == "\u{2501}" && s.style.fg == Some(c))
                .count()
        };
        // Eight megahertz of an 83.5 MHz band on a ~174-column track.
        let window = lit(theme.border_accent) + lit(theme.net_bt) + 1;
        assert!((15..=19).contains(&window), "{window}: {text}");
        assert!(lit(theme.net_bt) >= 12, "{text}");
        let dots: Vec<_> = line
            .spans
            .iter()
            .filter(|s| s.content == "\u{25cf}")
            .collect();
        assert_eq!(dots.len(), 1, "{text}");
        assert_eq!(dots[0].style.fg, Some(theme.net_ble));
        // The dot sits inside the lit window, not somewhere on the dashes.
        let at = text.chars().position(|c| c == '\u{25cf}').unwrap();
        let first = text.chars().position(|c| c == '\u{2501}').unwrap();
        assert!(at > first && at < first + window, "{text}");
    }

    /// The mode is the first thing the header says and it is never ambiguous:
    /// exactly one of the two words is on screen, whichever mode is running.
    #[test]
    fn exactly_one_mode_word_is_on_screen() {
        for (mode, want, other) in [
            (crate::state::NetMode::Survey, "SURVEY", "LOCK"),
            (crate::state::NetMode::Lock, "LOCK", "SURVEY"),
        ] {
            let mut m = net_fixture();
            m.net.mode = mode;
            let out = crate::state::fixture::draw(HeaderPanel, 100, 5, &m).join("\n");
            assert!(out.contains(want), "{want} missing from:\n{out}");
            assert!(!out.contains(other), "{other} present too:\n{out}");
        }
    }

    /// Outside the section the header is the one it has always been: no mode
    /// word, and the gain staging still there.
    #[test]
    fn the_variant_is_confined_to_its_own_section() {
        let m = SdrMetrics::fixture();
        let out = crate::state::fixture::draw(HeaderPanel, 100, 5, &m).join("\n");
        assert!(!out.contains("SURVEY") && !out.contains("LOCK"), "{out}");
        assert!(!out.contains(" NET · "), "{out}");
    }

    #[test]
    fn the_channel_is_named_beside_the_frequency() {
        let out = crate::state::fixture::draw(HeaderPanel, 100, 5, &net_fixture()).join("\n");
        assert!(out.contains("ch 6"), "{out}");
        assert!(out.contains("2437.000 MHz"), "{out}");
        assert!(out.contains("20.000 Msps"), "{out}");

        // Between channels there is no channel, and the header says nothing
        // rather than rounding to the nearest one.
        let mut m = net_fixture();
        m.radio.frequency = 2_439_500_000;
        let out = crate::state::fixture::draw(HeaderPanel, 100, 5, &m).join("\n");
        assert!(!out.contains("ch "), "{out}");
        assert!(out.contains("2439.500 MHz"), "{out}");
    }

    /// N10's exit condition, reworked when the band grew: at every width what is
    /// left is what matters most. The rate goes first, then the frequency, and
    /// the mode survives to the last column.
    #[test]
    fn the_band_gives_way_least_important_first() {
        let theme = crate::Theme::sdr();
        let v = Style::default();
        let fields = [
            BandField::new("Wi-Fi ch 6".to_string(), v, 5),
            BandField::new("2437.000 MHz".to_string(), v, 2),
            BandField::new("20.000 Msps".to_string(), v, 1),
            BandField::new("decode 12%".to_string(), v, 3),
            BandField::new("gaps 0".to_string(), v, 4),
        ];
        let render = |w: u16| -> String {
            compose_net_band(crate::state::NetMode::Lock, &fields, &theme, w)
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };

        let wide = render(120);
        for f in &fields {
            assert!(wide.contains(&f.text()), "{} missing: {wide}", f.text());
        }
        // Reading order is kept whatever survives.
        assert!(wide.find("MHz") < wide.find("Msps"));
        assert!(wide.find("decode") < wide.find("gaps"));

        let middle = render(50);
        assert!(
            middle.contains("Wi-Fi ch 6") && middle.contains("gaps 0"),
            "{middle}"
        );
        assert!(
            !middle.contains("Msps"),
            "the rate should go first: {middle}"
        );

        let narrow = render(12);
        assert!(narrow.contains("LOCK"), "the mode must survive: {narrow}");
        assert!(!narrow.contains("ch 6"), "{narrow}");

        for w in [12u16, 30, 50, 120] {
            assert!(
                render(w).chars().count() <= w as usize,
                "width {w} overflowed: {:?}",
                render(w)
            );
        }
    }

    /// A radio parked on a BLE channel with the BLE decoder running is on BLE
    /// channel 37, not on whatever Wi-Fi number 2402 MHz is nearest to.
    #[test]
    fn a_running_decoder_names_its_own_channel() {
        let mut m = net_fixture();
        m.radio.frequency = 2_402_000_000;
        m.net.ble_channel = Some(37);
        let out = crate::state::fixture::draw(HeaderPanel, 120, 5, &m).join("\n");
        assert!(out.contains("\u{25cf} BLE 37 adv"), "{out}");
        assert!(!out.contains("Wi-Fi"), "{out}");
        // A data channel says so: advertising never comes there.
        m.net.ble_channel = Some(3);
        let out = crate::state::fixture::draw(HeaderPanel, 120, 5, &m).join("\n");
        assert!(out.contains("BLE 3 data"), "{out}");
        m.net.ble_channel = Some(37);

        // Classic Bluetooth watches a span of channels, and says which.
        m.net.ble_channel = None;
        m.net.bt_channels_watched = vec![38, 39, 40, 41];
        let out = crate::state::fixture::draw(HeaderPanel, 120, 5, &m).join("\n");
        assert!(out.contains("\u{25a0} BT 38\u{2013}41"), "{out}");

        // Both at once, when both are running.
        m.net.ble_channel = Some(37);
        let out = crate::state::fixture::draw(HeaderPanel, 120, 5, &m).join("\n");
        assert!(
            out.contains("\u{25cf} BLE 37 adv \u{b7} \u{25a0} BT 38\u{2013}41"),
            "{out}"
        );
    }

    /// Before the feed has delivered a block there is nothing to have been
    /// interrupted, and before a load has been measured there is no load: both
    /// are absent, never zero.
    #[test]
    fn gaps_and_load_appear_only_once_there_is_something_to_report() {
        let mut m = net_fixture();
        let out = crate::state::fixture::draw(HeaderPanel, 140, 5, &m).join("\n");
        assert!(!out.contains("gaps"), "{out}");
        assert!(!out.contains("decode"), "{out}");

        m.net.health.last_block = Some(std::time::Instant::now());
        m.net.health.gaps = 3;
        m.net.health.decode_load = Some(0.42);
        let out = crate::state::fixture::draw(HeaderPanel, 140, 5, &m).join("\n");
        assert!(out.contains("gaps 3"), "{out}");
        assert!(out.contains("decode 42%"), "{out}");
    }

    /// A percentage while it reads as one, a multiple of real time after.
    #[test]
    fn a_heavy_load_is_shown_as_a_multiple_of_real_time() {
        assert_eq!(load_text(0.42), "42%");
        assert_eq!(load_text(8.4), "840%");
        assert_eq!(load_text(1807.42), "1807\u{d7}");
    }

    /// The rail spans the band being worked, not the radio's whole range. On a
    /// HackRF the two differ by a factor of seventy, and on the wrong one the
    /// marker never moves.
    #[test]
    fn the_rail_spans_the_band_and_not_the_whole_radio() {
        let m = net_fixture();
        let net = crate::state::fixture::draw(HeaderPanel, 100, 5, &m).join("\n");
        assert!(net.contains("2400") && net.contains("2483"), "{net}");
        assert!(
            !net.contains(" 6G "),
            "the full tuning range leaked in:\n{net}"
        );

        let mut plain = m.clone();
        plain.ui.section = "lab".to_string();
        let out = crate::state::fixture::draw(HeaderPanel, 100, 5, &plain).join("\n");
        assert!(out.contains(" 6G "), "{out}");
        assert!(!out.contains("2400"), "{out}");
    }
}
