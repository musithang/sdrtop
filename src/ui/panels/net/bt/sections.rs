// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the Classic view and the Piconet view say alike about a piconet.
//!
//! **One set of limits.** The BR limits and the resolutions a reading must
//! beat are here once, so a figure is judged alike wherever it is drawn
//! (rule 5): the Classic view's summary line, the packet list's cells and
//! the bench's rows all read them from here.
//!
//! **The parts with one owner each.** The slot clock, the residual plot,
//! the HEADERS account and a piconet's channels as runs are drawn by one
//! function each, which ever panel shows them.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::signal::bt::piconet::{Deviation, Piconet};
use crate::signal::dsp::uncertainty::Uncertain;
use crate::state::SdrMetrics;
use crate::ui::widgets::limit::Limit;

/// Label width of a field row, the Piconets panel's detail block's and the
/// bench's.
pub(super) const LABEL_W: usize = 9;

/// BR's modulation index band, **read from the Core Specification 5.4,
/// Vol 2, Part A, 3.1.1** on the SIG's own site: "The
/// Modulation index shall be between 0.28 and 0.35" (GFSK, BT = 0.5,
/// 1 Msym/s).
pub(super) const BR_INDEX: Limit = Limit::Band {
    low: 0.28,
    high: 0.35,
};

/// The same band as a deviation: `h = 2 * delta_f / 1 Msym/s`, so 140 to
/// 175 kHz. Derived from [`BR_INDEX`], not a second figure from the text.
pub(super) const BR_DELTA_F1_KHZ: Limit = Limit::Band {
    low: 140.0,
    high: 175.0,
};

/// The same section: "the minimum frequency deviation, Fmin ... which
/// corresponds to 1010 sequence shall be no smaller than ±80% of the
/// frequency deviation (fd) ... which corresponds to a 00001111 sequence".
/// The text states it for the minimum; what is shown against it here is the
/// ratio of the means, which is what a header's symbols support, and the
/// row is labelled so.
pub(super) const BR_RATIO: Limit = Limit::Min(0.8);

/// Resolutions a reading must beat before it prints, as the BLE rows'
/// (`ble_detail`): a fraction of the band each limit states.
pub(super) const INDEX_RESOLUTION: f64 = 0.02;
pub(super) const DELTA_F1_RESOLUTION_KHZ: f64 = 10.0;
pub(super) const RATIO_RESOLUTION: f64 = 0.1;

/// The initial carrier's limit, **read from the Core Specification 5.4,
/// Vol 2, Part A, 3.1.3**: "The transmitted initial center frequency shall
/// be within ±75 kHz from Fc."
pub(super) const F0_LIMIT_KHZ: Limit = Limit::Band {
    low: -75.0,
    high: 75.0,
};

/// The drift's limit, **read from the same section's Table 3.3**: ±25 kHz
/// for a one-slot packet, ±40 kHz for three and five slots. What is read
/// here is the access code and header, which every packet type must keep
/// within 40 of its f0; a one-slot packet's 25 is over its whole length,
/// which a header cannot show. So 40 is what a reading is held to, and a
/// reading over it is a packet over its limit whatever its type.
pub(super) const DRIFT_LIMIT_KHZ: Limit = Limit::Band {
    low: -40.0,
    high: 40.0,
};

/// **The same table**: "Maximum drift rate 400 Hz/µs", allowed "anywhere in
/// a packet".
pub(super) const DRIFT_RATE_LIMIT: Limit = Limit::Band {
    low: -400.0,
    high: 400.0,
};

/// As the BLE rows' (`ble_detail`).
pub(super) const F0_RESOLUTION_KHZ: f64 = 10.0;
pub(super) const DRIFT_RESOLUTION_KHZ: f64 = 10.0;
pub(super) const DRIFT_RATE_RESOLUTION: f64 = 80.0;

/// Slot jitter's limit, **read from the Core Specification 5.4, Vol 2,
/// Part B, 2.2.5**: "The instantaneous timing shall not deviate more than
/// 1 μs from the average timing."
pub(super) const JITTER_LIMIT_US: Limit = Limit::Max(1.0);

/// A jitter reading must beat this before it prints: a quarter of the
/// limit, which a few dozen hits reach.
pub(super) const JITTER_RESOLUTION_US: f64 = 0.25;

/// The slot clock's limit, **read from the Core Specification 5.4, Vol 2,
/// Part B, 2.2.5**: "the average timing of packet transmission shall not
/// drift faster than 20 ppm relative to the ideal slot timing of 625 μs".
pub(super) const CLOCK_LIMIT_PPM: Limit = Limit::Band {
    low: -20.0,
    high: 20.0,
};

/// A clock reading must beat this before it prints: a twentieth of the
/// limit, which a minute of hits passes by far.
pub(super) const CLOCK_RESOLUTION_PPM: f64 = 1.0;

/// A piconet's slot clock from its fitted grid, the classic twin of the
/// census's crystal error: its slots run `rate_ppm` long on our clock, so
/// its clock runs that much slow, less our own oscillator's error, which a
/// reference takes out exactly as it does for a BLE offset
/// (`corrected_ppm`: the radio's LO and its sample clock come from one
/// crystal). With whether it is judged: only a reference makes it absolute,
/// and without one it is a reading, which the frame's [RELATIVE] says.
pub(super) fn clock_of(
    f: &crate::signal::bt::slots::SlotFit,
    state: &SdrMetrics,
) -> (Uncertain, bool) {
    let raw = Uncertain::from_sigma(-f.rate_ppm, f.rate_sigma_ppm);
    let (clock, provenance) = state.radio.corrected_ppm(raw, std::time::Instant::now());
    (clock, provenance != crate::state::Provenance::Unreferenced)
}

/// Where a referenced slot clock stands against 2.2.5's 20 ppm, in words:
/// outside, at the edge within twice its uncertainty, or inside.
pub(super) fn clock_verdict(clock: Uncertain) -> &'static str {
    let Limit::Band { low, high } = CLOCK_LIMIT_PPM else {
        return "";
    };
    let (v, s) = (clock.value(), clock.sigma());
    if v < low || v > high {
        "outside the 20 ppm limit"
    } else if v - 2.0 * s < low || v + 2.0 * s > high {
        "at the edge of the 20 ppm limit"
    } else {
        "inside the 20 ppm limit"
    }
}

/// The modulation index a side's (or one packet's) settled readings give,
/// with its uncertainty: `h = 2 * df1 / 1 Msym/s`. `None` with fewer than
/// two readings.
pub(super) fn index_of(dev: &Deviation) -> Option<Uncertain> {
    dev.settled.mean().map(|df1| df1.scale(2.0 / 1e6))
}

/// What a piconet's UAP rests on, in words, as the address mode shows it:
/// one value only a payload's CRC can choose (a header leaves two,
/// `header::PiconetClock`), with the CRCs that pass under it in the packets
/// kept; two candidates, where an encrypted link stays until a reconnect
/// sends a few packets in the clear; more, which further headers narrow; or
/// none narrowed yet. Candidates are listed while a reader can take them in
/// and never when masked, where two are half an address byte from one.
pub(super) fn uap_account(p: &Piconet, net: &crate::state::NetState) -> String {
    let masked = net.address_display == crate::state::AddressDisplay::Masked;
    let listed = |many: &[u8]| {
        if masked {
            String::new()
        } else {
            format!(
                " ({})",
                many.iter()
                    .map(|u| format!("{u:#04x}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    };
    match net.bt_uap.get(&p.lap).map(|u| u.as_slice()) {
        Some([one]) => {
            let passing = p
                .packets
                .iter()
                .filter(|k| k.payload == crate::signal::bt::piconet::PayloadVerdict::Crc(true))
                .count();
            let under = match passing {
                0 => String::new(),
                1 => " · 1 CRC passes under it".to_string(),
                n => format!(" · {n} CRCs pass under it"),
            };
            format!("{}, resolved by a payload CRC{under}", net.show_uap(*one))
        }
        Some(two @ [_, _]) => format!(
            "2 candidates{}: only a payload CRC chooses, which an encrypted link \
             gives only at a reconnect",
            listed(two)
        ),
        Some(few) if (3..=4).contains(&few.len()) => format!(
            "{} candidates{}; each further header narrows them",
            few.len(),
            listed(few)
        ),
        Some(many) if !many.is_empty() => {
            format!(
                "{} candidates; each further header narrows them",
                many.len()
            )
        }
        _ => "not narrowed: no header of it decoded yet".to_string(),
    }
}

/// The channels a piconet was heard on, as runs: `2-5, 17, 40-41`.
pub(super) fn channel_runs(mask: u128) -> String {
    let mut runs = Vec::new();
    let mut ch = 0u8;
    while ch < 79 {
        if mask & (1 << ch) == 0 {
            ch += 1;
            continue;
        }
        let start = ch;
        while ch + 1 < 79 && mask & (1 << (ch + 1)) != 0 {
            ch += 1;
        }
        runs.push(if start == ch {
            start.to_string()
        } else {
            format!("{start}-{ch}")
        });
        ch += 1;
    }
    runs.join(", ")
}

/// The residual plot's reach either side of the grid, µs: past the 1 µs
/// limit, so a residual beyond it shows as one.
const PLOT_US: f64 = 1.5;

/// The residuals of one piconet as a shape, stacked by who sent them: the
/// master's in `inks[0]`, then the slave's in `inks[1]`, then those of
/// unknown sender in `inks[2]` (the stale ink), so the two ends stand as
/// two humps of two colours when their timing differs, and nothing is put
/// on a side it was not heard from. `rows` rows of eighth blocks over
/// −[`PLOT_US`] to +[`PLOT_US`], the specification's ±1 µs (2.2.5) as `┊`
/// rules in the warning ink and zero as a dim one, then a tick row and a
/// label row. A cell shows one colour: the part that fills most of it.
/// Returns the lines, empty where the width cannot hold a readable plot,
/// and how many residuals fell beyond it.
pub(super) fn residual_shape(
    by_side: [&[f64]; 3],
    rows: usize,
    iw: usize,
    inks: [ratatui::style::Color; 3],
    theme: &crate::Theme,
) -> (Vec<Line<'static>>, usize) {
    const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let cols = iw.saturating_sub(4);
    let beyond = by_side
        .iter()
        .flat_map(|side| side.iter())
        .filter(|r| r.abs() >= PLOT_US)
        .count();
    if cols < 15 || rows == 0 {
        return (Vec::new(), beyond);
    }
    let col_of = |x: f64| {
        (((x + PLOT_US) / (2.0 * PLOT_US)) * cols as f64)
            .floor()
            .clamp(0.0, cols as f64 - 1.0) as usize
    };
    let mut bins = vec![[0u32; 3]; cols];
    for (k, side) in by_side.iter().enumerate() {
        for &r in side.iter().filter(|r| r.abs() < PLOT_US) {
            bins[col_of(r)][k] += 1;
        }
    }
    let most = bins
        .iter()
        .map(|b| b.iter().sum::<u32>())
        .max()
        .unwrap_or(0)
        .max(1);
    let scale = (rows * 8) as f64 / most as f64;
    // Each bin's segments as tops in eighths, stacked master, slave,
    // unknown, rounded cumulatively so the whole bar is its total's height.
    let tops: Vec<[usize; 3]> = bins
        .iter()
        .map(|b| {
            let mut sum = 0;
            let mut t = [0usize; 3];
            for k in 0..3 {
                sum += b[k];
                t[k] = (sum as f64 * scale).round() as usize;
            }
            t
        })
        .collect();
    let limits = [col_of(-1.0), col_of(1.0)];
    let zero = col_of(0.0);
    let mut out = Vec::with_capacity(rows + 2);
    for row in 0..rows {
        let base = (rows - 1 - row) * 8;
        let mut spans = vec![Span::raw("  ")];
        for (c, t) in tops.iter().enumerate() {
            let fill = t[2].saturating_sub(base).min(8);
            if fill > 0 {
                // The segment that fills most of this cell's part.
                let (lo, hi) = (base, base + fill);
                let mut bottom = 0;
                let mut best = (0, 0usize);
                for (k, &top) in t.iter().enumerate() {
                    let overlap = top.min(hi).saturating_sub(bottom.max(lo));
                    if overlap > best.1 {
                        best = (k, overlap);
                    }
                    bottom = top;
                }
                spans.push(Span::styled(
                    EIGHTHS[fill].to_string(),
                    Style::default().fg(inks[best.0]),
                ));
            } else if limits.contains(&c) {
                spans.push(Span::styled(
                    "\u{250a}",
                    Style::default().fg(theme.status_warn),
                ));
            } else if c == zero {
                spans.push(Span::styled(
                    "\u{250a}",
                    Style::default().fg(theme.border_dim),
                ));
            } else {
                spans.push(Span::raw(" "));
            }
        }
        out.push(Line::from(spans));
    }
    let ticks: String = (0..cols)
        .map(|c| {
            if limits.contains(&c) || c == zero {
                '\u{2534}'
            } else {
                '\u{2500}'
            }
        })
        .collect();
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(ticks, Style::default().fg(theme.border_dim)),
    ]));
    let mut labels = vec![' '; cols];
    for (c, text) in [(limits[0], "-1"), (zero, "0"), (limits[1], "+1")] {
        let at = c.saturating_sub(text.len() / 2).min(cols - text.len());
        for (i, ch) in text.chars().enumerate() {
            labels[at + i] = ch;
        }
    }
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            labels.into_iter().collect::<String>(),
            Style::default().fg(theme.label),
        ),
    ]));
    (out, beyond)
}

/// What the piconet's headers say, under its own heading, marked as the
/// port it is: the header decode is `libbtbb`'s, checked on the air
/// against two devices whose addresses were read off them
/// (`signal::bt::header`, rule 1).
///
/// **Read only under one UAP.** Before the UAP is one value the heading
/// says so and how many headers are waiting; no type is guessed from a
/// candidate (rule 2). A header that did not decode under the resolved UAP
/// is counted beside the ones that did, because a rising count is how a
/// wrong resolution would show.
pub(super) fn header_lines(
    p: &Piconet,
    state: &SdrMetrics,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::signal::bt::header::PacketType;
    let h = &p.headers;
    let field = |label: &str, value: String| {
        Line::from(vec![
            crate::ui::chrome::field(label, LABEL_W, theme),
            Span::styled(value, Style::default().fg(theme.value)),
        ])
    };
    let mut out = vec![crate::ui::chrome::section(
        "headers",
        "libbtbb port, checked on air",
        iw,
        theme,
    )];
    if h.captured == 0 {
        out.push(field(
            "captured",
            "none yet: no header followed a hit".to_string(),
        ));
        return out;
    }
    let uap = match state.net.bt_uap.get(&p.lap).map(|u| u.as_slice()) {
        Some([one]) => *one,
        other => {
            let n = other.map_or(0, |u| u.len());
            out.push(field(
                "captured",
                format!(
                    "{}, not read: UAP not resolved ({n} candidates)",
                    h.captured
                ),
            ));
            out.push(field("CLK1-6", clock_text(h.clock_hypotheses)));
            return out;
        }
    };
    let mut read = format!("{} of {} captured", h.decoded, h.captured);
    if h.undecoded > 0 {
        read.push_str(&format!(
            ", {} did not decode under {}",
            h.undecoded,
            state.net.show_uap(uap)
        ));
    }
    out.push(field("read", read));
    let mut mix: Vec<(u32, u8)> = (0..16u8)
        .map(|c| (h.types[c as usize], c))
        .filter(|(n, _)| *n > 0)
        .collect();
    mix.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let room = iw.saturating_sub(LABEL_W + 1);
    let types = if mix.is_empty() {
        "-".to_string()
    } else {
        mix.iter()
            .map(|(n, c)| format!("{} {n}", PacketType::from_code(*c).shown()))
            .collect::<Vec<_>>()
            .join(" \u{00b7} ")
    };
    for (i, chunk) in crate::ui::chrome::wrap(&types, room, 2)
        .into_iter()
        .enumerate()
    {
        out.push(field(if i == 0 { "types" } else { "" }, chunk));
    }
    let addrs: Vec<String> = (0..8u8)
        .filter(|a| h.lt_addrs & (1 << a) != 0)
        .map(|a| {
            if a == 0 {
                "0 (broadcast)".to_string()
            } else {
                a.to_string()
            }
        })
        .collect();
    out.push(field(
        "LT_ADDR",
        if addrs.is_empty() {
            "-".to_string()
        } else {
            addrs.join(", ")
        },
    ));
    out.push(field("CLK1-6", clock_text(h.clock_hypotheses)));
    out
}

/// The CLK1-6 hunt, in words: the whitening every header is read through
/// depends on it.
fn clock_text(hypotheses: u8) -> String {
    match hypotheses {
        0 => "not tracked yet".to_string(),
        1 => "found (1 of 64 hypotheses left)".to_string(),
        n => format!("{n} of 64 hypotheses left"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_runs_join_neighbours() {
        assert_eq!(channel_runs(0), "");
        assert_eq!(channel_runs(0b1111 << 2 | 1 << 17 | 1 << 78), "2-5, 17, 78");
    }

    /// The residuals stacked by who sent them: each end in its own colour,
    /// the master's below where a bin holds both, those of unknown sender
    /// in the stale ink and never on a side; the limits ruled, the axis
    /// labelled, and what fell past the plot counted.
    #[test]
    fn each_end_stands_in_its_own_colour() {
        let theme = crate::Theme::sdr();
        let (m, s) = (theme.series_color(1), theme.value);
        let inks = [m, s, theme.stale];
        let master: Vec<f64> = vec![-1.2; 12];
        let slave: Vec<f64> = vec![1.2; 12];
        let unknown: Vec<f64> = vec![0.0; 4];
        let (lines, beyond) = residual_shape([&master, &slave, &unknown], 6, 64, inks, &theme);
        assert_eq!(beyond, 0);
        assert_eq!(lines.len(), 6 + 2, "six rows, the ticks, the labels");
        let inks_of = |col: usize| -> Vec<Option<ratatui::style::Color>> {
            lines[..6]
                .iter()
                .filter_map(|l| {
                    let span = l.spans.get(1 + col)?;
                    (span.content != " " && span.content != "\u{250a}").then_some(span.style.fg)
                })
                .collect()
        };
        let cols = 60;
        let col_of = |x: f64| ((x + 1.5) / 3.0 * cols as f64).floor() as usize;
        assert!(
            inks_of(col_of(-1.2)).iter().all(|c| *c == Some(m)),
            "{:?}",
            inks_of(col_of(-1.2))
        );
        assert!(!inks_of(col_of(-1.2)).is_empty());
        assert!(inks_of(col_of(1.2)).iter().all(|c| *c == Some(s)));
        assert!(inks_of(col_of(0.0)).iter().all(|c| *c == Some(theme.stale)));

        // One bin with both: the master's part below, the slave's above.
        let (lines, _) = residual_shape([&[0.5; 6], &[0.5; 6], &[]], 6, 64, inks, &theme);
        let column: Vec<_> = lines[..6]
            .iter()
            .map(|l| l.spans[1 + col_of(0.5)].style.fg)
            .collect();
        assert_eq!(
            column.first(),
            Some(&Some(s)),
            "the slave's on top: {column:?}"
        );
        assert_eq!(
            column.last(),
            Some(&Some(m)),
            "the master's below: {column:?}"
        );

        // The axis, and what fell past it.
        let (lines, beyond) = residual_shape([&[2.0, 0.1], &[-3.0], &[]], 6, 64, inks, &theme);
        assert_eq!(beyond, 2);
        let labels: String = lines[7]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(labels.contains("-1") && labels.contains("+1"), "{labels:?}");
        let rule = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "\u{250a}")
            .unwrap();
        assert_eq!(
            rule.style.fg,
            Some(theme.status_warn),
            "the limits in the warning ink"
        );
        // Too narrow for a readable plot: none, and still the count.
        assert_eq!(
            residual_shape([&[3.0], &[], &[]], 6, 16, inks, &theme),
            (Vec::new(), 1)
        );
    }
}
