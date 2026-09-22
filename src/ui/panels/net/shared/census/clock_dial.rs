// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! One clock, as a moving-coil ppm meter: what `CLOCK ERROR` becomes when a
//! device is selected and the panel has room for it.
//!
//! **A different question from the room's, so a different instrument.** With
//! nothing selected the block answers "whose clock is worst" and ranks every
//! device on a null meter (`clock_error`). A selection asks about one device,
//! and a list of the others would only be in the way; this answers "how far
//! off is this one, and what does that mean", in the form everyone reads
//! without being taught: a dial with a needle, zero straight up, and a scale
//! that turns from green to red where the specification says it should.
//!
//! **The scale is the same judgement as the meters'.** The zones are
//! `clock_error::verdict`'s thresholds (inside on every advertising channel,
//! the channel-dependent band between, outside on every channel), drawn only
//! with a frequency reference; without one the arc is one neutral colour,
//! because every offset still carries our own oscillator's error and a red
//! zone would be a verdict nobody measured.
//!
//! **Beside the dial, the reading and what it means.** The value in big
//! digits with its uncertainty, where the clock ranks in the room, the same
//! error in kHz on each advertising channel (one ppm, three different kHz,
//! which is why the census keeps ppm), the error as a watch would show it,
//! the margin to the limit where one can be judged, and what the offset was
//! measured against. The detail block below drops its own crystal line while
//! the dial is drawn: one fact, one place (rule 6).
//!
//! When the panel has no room for the dial, the selected device gets its one
//! null meter instead (`clock_error::lines` with `only`), never the room's.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::canvas::{Canvas, Line as CanvasLine, Points},
    Frame,
};

use super::clock_error::{full_scale, limit_ppm, meter_ink, verdict, Verdict};
use crate::signal::dsp::uncertainty::Uncertain;
use crate::signal::net::census::Device;
use crate::state::{Provenance, SdrMetrics};
use crate::ui::widgets::bigdigits::glyph;
use crate::ui::widgets::reading::Reading;

/// Rows the dial takes, and so the text beside it.
pub(super) const DIAL_ROWS: usize = 11;

/// Columns: a semicircle on braille dots, which are square when a cell is
/// twice as tall as it is wide, with room outside the arc for its labels.
pub(super) const DIAL_COLS: usize = 40;

/// Columns the text beside the dial needs: a gap and the three-channel kHz
/// line, the longest of them.
pub(super) const SIDE_COLS: usize = 3 + 58;

/// Headroom past the needle, so it never lies flat against the stop where a
/// reader cannot tell "at full scale" from "off the scale".
const HEADROOM: f64 = 1.25;

/// The smallest full scale the dial uses: a clock a few tenths of a ppm out
/// is not drawn at a zoom that makes it look like a bad one.
const MIN_SCALE_PPM: f64 = 10.0;

/// Seconds in a day, for the watch line.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// What the dial needs, owned, so the paint closure can take it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Dial {
    ppm: Uncertain,
    scale: f64,
    /// `Some((inside, outside))` when the specification can be judged.
    limit: Option<(f64, f64)>,
    needle: Color,
}

impl Dial {
    /// The angle `v` ppm sits at: zero straight up, full scale flat either
    /// side, past it clamped to the stop.
    fn angle(&self, v: f64) -> f64 {
        std::f64::consts::FRAC_PI_2
            - (v / self.scale).clamp(-1.0, 1.0) * std::f64::consts::FRAC_PI_2
    }

    /// Draw it into `area`, which the caller has sized to [`DIAL_COLS`] by
    /// [`DIAL_ROWS`] and kept clear of text.
    pub(super) fn render(&self, f: &mut Frame, area: Rect, theme: &crate::Theme) {
        let d = *self;
        let (ok, warn, crit, accent, dim, label, hi) = (
            theme.status_ok,
            theme.status_warn,
            theme.status_crit,
            theme.border_accent,
            theme.border_dim,
            theme.label,
            theme.value_hi,
        );
        let at = |r: f64, a: f64| (r * a.cos(), r * a.sin());
        let canvas = Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([-1.42, 1.42])
            .y_bounds([-0.12, 1.42])
            .paint(move |ctx| {
                // The scale: a band three dots deep, coloured by zone.
                let steps = 400;
                let mut zones: [Vec<(f64, f64)>; 4] = Default::default();
                for i in 0..=steps {
                    let v = -d.scale + 2.0 * d.scale * i as f64 / steps as f64;
                    let a = d.angle(v);
                    let zone = match d.limit {
                        None => 3,
                        Some((inside, _)) if v.abs() <= inside => 0,
                        Some((_, outside)) if v.abs() <= outside => 1,
                        Some(_) => 2,
                    };
                    for r in [1.0, 0.97, 0.94] {
                        zones[zone].push(at(r, a));
                    }
                }
                for (z, ink) in [(0, ok), (1, warn), (2, crit), (3, accent)] {
                    ctx.draw(&Points {
                        coords: &zones[z],
                        color: ink,
                    });
                }
                // Ticks outside the band at every quarter, the labelled
                // halves longer.
                for k in -4..=4i32 {
                    let a = d.angle(d.scale * k as f64 / 4.0);
                    let (x1, y1) = at(1.05, a);
                    let (x2, y2) = at(if k % 2 == 0 { 1.15 } else { 1.09 }, a);
                    ctx.draw(&CanvasLine {
                        x1,
                        y1,
                        x2,
                        y2,
                        color: dim,
                    });
                }
                ctx.layer();
                // The needle: two strokes side by side, so it reads as a
                // pointer and not a scratch, and a hub over its root.
                let a = d.angle(d.ppm.value());
                let (nx, ny) = (-a.sin() * 0.012, a.cos() * 0.012);
                let (tx, ty) = at(0.88, a);
                for s in [-1.0, 1.0] {
                    ctx.draw(&CanvasLine {
                        x1: s * nx,
                        y1: s * ny,
                        x2: tx + s * nx,
                        y2: ty + s * ny,
                        color: d.needle,
                    });
                }
                let hub: Vec<(f64, f64)> = (0..60)
                    .flat_map(|i| {
                        let t = i as f64 / 60.0 * std::f64::consts::TAU;
                        [0.03, 0.06, 0.09].map(|r| (r * t.cos(), (r * t.sin()).max(0.0)))
                    })
                    .collect();
                ctx.draw(&Points {
                    coords: &hub,
                    color: hi,
                });
                ctx.layer();
                // The scale's figures, outside the ticks.
                let style = Style::default().fg(label);
                let half = d.scale / 2.0;
                for (v, text) in [
                    (-d.scale, format!("-{:.0}", d.scale)),
                    (-half, format!("-{half:.0}")),
                    (0.0, "0".to_string()),
                    (half, format!("+{half:.0}")),
                    (d.scale, format!("+{:.0}", d.scale)),
                ] {
                    let (x, y) = at(1.3, d.angle(v));
                    // Centred on its tick: a column is this much of the x
                    // range, so shift by half the text's width.
                    let w = text.chars().count() as f64 * (2.84 / DIAL_COLS as f64);
                    ctx.print(x - w / 2.0, y, Span::styled(text, style));
                }
            });
        f.render_widget(canvas, area);
    }
}

/// `worst`, `2nd worst`, `11th worst`, `21st worst`.
fn ordinal(rank: usize) -> String {
    if rank == 1 {
        return "worst".to_string();
    }
    let suffix = match (rank % 10, rank % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{rank}{suffix} worst")
}

/// `s` in big glyphs, three rows, one column apart.
fn big(s: &str, ink: Color) -> [Vec<Span<'static>>; 3] {
    let style = Style::default().fg(ink).add_modifier(Modifier::BOLD);
    let mut rows: [Vec<Span<'static>>; 3] = Default::default();
    for (i, c) in s.chars().enumerate() {
        let g = glyph(c);
        for (r, row) in rows.iter_mut().enumerate() {
            if i > 0 {
                row.push(Span::raw(" "));
            }
            row.push(Span::styled(g[r].to_string(), style));
        }
    }
    rows
}

/// The error as a watch driven by this crystal would show it: `loses 0.80
/// ±0.03 s a day`, or, relative to ours, `0.80 ±0.03 s a day slower than
/// ours`. A crystal `x` ppm slow loses `x` microseconds every second; the
/// uncertainty scales with it.
fn watch(u: Uncertain, judged: bool) -> String {
    let per_day = u.scale(SECONDS_PER_DAY * 1e-6 * u.value().signum());
    let day = Reading::new(per_day, "s a day", f64::INFINITY).text();
    let slow = u.value() < 0.0;
    if judged {
        format!("{} {day}", if slow { "loses" } else { "gains" })
    } else {
        format!("{day} {} than ours", if slow { "slower" } else { "faster" })
    }
}

/// The margin to the specification, in the verdict's own terms: inside or
/// outside by how much, or why it cannot be called. Never "pass" (idiom B).
fn spec(u: Uncertain) -> String {
    let (inside, outside) = limit_ppm();
    let v = u.value().abs();
    match verdict(u) {
        Verdict::Inside => format!("inside by {:.1} ppm", inside - v),
        Verdict::Outside => format!("outside by {:.1} ppm", v - outside),
        Verdict::Close => "too close to call".to_string(),
    }
}

/// The text beside the dial, exactly [`DIAL_ROWS`] lines.
fn side(
    u: Uncertain,
    rank: usize,
    of: usize,
    state: &SdrMetrics,
    now: std::time::Instant,
    ink: Color,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let judged = state.radio.offset_basis(now).provenance != Provenance::Unreferenced;
    let label = Style::default().fg(theme.label);
    let value = Style::default().fg(theme.value);
    let field = |name: &str, spans: Vec<Span<'static>>| {
        let mut out = vec![Span::styled(format!("{name:<8}"), label)];
        out.extend(spans);
        Line::from(out)
    };

    // The figure to the places its uncertainty earns, as every reading.
    let places = u.decimals().unwrap_or(2).max(0) as usize;
    let [top, middle, mut bottom] = big(&format!("{:+.*}", places, u.value()), ink);
    bottom.push(Span::styled(
        format!("  ±{:.*} ppm", places, u.sigma()),
        label,
    ));

    let khz = |ch: u8| {
        crate::signal::ble::channel::centre_hz(ch)
            .map(|hz| format!("{:+.1}", u.value() * hz as f64 * 1e-9))
            .unwrap_or_else(|| "-".to_string())
    };
    let against = match state.radio.reference.as_ref() {
        Some(r) if judged => format!("against {}", r.source),
        Some(_) => "our own oscillator, the reference expired".to_string(),
        None => "our own oscillator".to_string(),
    };

    let mut out = vec![
        Line::default(),
        Line::from(top),
        Line::from(middle),
        Line::from(bottom),
        Line::default(),
        field(
            "clock",
            vec![Span::styled(format!("{} of {of}", ordinal(rank)), value)],
        ),
        field(
            "on air",
            vec![
                Span::styled(
                    format!("{} · {} · {} kHz", khz(37), khz(38), khz(39)),
                    value,
                ),
                Span::styled("  ch 37 · 38 · 39", label),
            ],
        ),
        field("a watch", vec![Span::styled(watch(u, judged), value)]),
        field(
            "spec",
            if judged {
                vec![
                    Span::styled(spec(u), Style::default().fg(ink)),
                    Span::styled("  of ±150 kHz", label),
                ]
            } else {
                vec![Span::styled("no limit without a reference", label)]
            },
        ),
        field("vs", vec![Span::styled(against, label)]),
    ];
    out.resize(DIAL_ROWS, Line::default());
    out
}

/// The block for a selected device whose clock is measured, when it fits in
/// `max_lines` by `iw`: the section rule and the text beside the dial, each
/// line indented past where the dial will be drawn, and the dial itself for
/// the caller to draw over that space once the text is down.
pub(super) fn view(
    devices: &[Device],
    selected: [u8; 6],
    state: &SdrMetrics,
    now: std::time::Instant,
    iw: usize,
    max_lines: usize,
    theme: &crate::Theme,
) -> Option<(Vec<Line<'static>>, Dial)> {
    if max_lines < DIAL_ROWS + 1 || iw < DIAL_COLS + SIDE_COLS {
        return None;
    }
    let ranked = super::clock_error::ranked(devices, state, now);
    let rank = ranked.iter().position(|(d, _)| d.address == selected)?;
    let (d, u) = ranked[rank];

    let judged = state.radio.offset_basis(now).provenance != Provenance::Unreferenced;
    let limit = limit_ppm();
    let ink = meter_ink(u, judged, theme);
    let reach = if judged {
        u.value().abs().max(limit.0)
    } else {
        u.value().abs()
    };
    let dial = Dial {
        ppm: u,
        scale: full_scale((reach * HEADROOM).max(MIN_SCALE_PPM)),
        limit: judged.then_some(limit),
        needle: ink,
    };

    let mut lines = vec![crate::ui::chrome::section(
        "clock error",
        &d.address_text(&state.net, None),
        iw,
        theme,
    )];
    for l in side(u, rank + 1, ranked.len(), state, now, ink, theme) {
        let mut spans = vec![Span::raw(" ".repeat(DIAL_COLS + 3))];
        spans.extend(l.spans);
        lines.push(Line::from(spans));
    }
    Some((lines, dial))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rank_reads_as_english() {
        let got: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101]
            .into_iter()
            .map(ordinal)
            .collect();
        assert_eq!(
            got,
            [
                "worst",
                "2nd worst",
                "3rd worst",
                "4th worst",
                "11th worst",
                "12th worst",
                "13th worst",
                "21st worst",
                "22nd worst",
                "23rd worst",
                "101st worst"
            ]
        );
    }

    /// **Zero is straight up and full scale is flat**, and a needle past
    /// full scale rests on the stop rather than swinging round underneath.
    #[test]
    fn the_needle_stands_up_at_zero_and_lies_flat_at_full_scale() {
        let d = Dial {
            ppm: Uncertain::exact(0.0),
            scale: 100.0,
            limit: None,
            needle: Color::Reset,
        };
        let pi = std::f64::consts::PI;
        assert!((d.angle(0.0) - pi / 2.0).abs() < 1e-12);
        assert!((d.angle(-100.0) - pi).abs() < 1e-12, "left for slow");
        assert!(d.angle(100.0).abs() < 1e-12, "right for fast");
        assert_eq!(d.angle(-400.0), d.angle(-100.0));
    }

    /// A crystal 9.3 ppm slow loses 0.80 s a day; relative to ours, the same
    /// figure is said as a difference, not as the device's own drift.
    #[test]
    fn the_watch_line_is_the_error_in_seconds_a_day() {
        let slow = Uncertain::from_sigma(-9.3, 0.23);
        assert_eq!(watch(slow, true), "loses 0.804 ±0.020 s a day");
        assert_eq!(watch(slow, false), "0.804 ±0.020 s a day slower than ours");
        let fast = Uncertain::from_sigma(12.0, 0.5);
        assert!(
            watch(fast, true).starts_with("gains 1.04"),
            "{}",
            watch(fast, true)
        );
    }

    /// The margin says the same thing the colour does, the amber band
    /// included, and never the word "pass".
    #[test]
    fn the_spec_line_says_what_the_colour_says() {
        let u = |v| Uncertain::from_sigma(v, 0.1);
        assert_eq!(spec(u(-9.3)), "inside by 51.2 ppm");
        assert_eq!(spec(u(-95.6)), "outside by 33.2 ppm");
        assert_eq!(spec(u(61.5)), "too close to call");
        for v in [-9.3, -95.6, 61.5] {
            assert!(!spec(u(v)).to_lowercase().contains("pass"));
        }
    }
}
