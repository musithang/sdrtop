// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The TUNER zone: the radio's whole tuning range with the 2.4 GHz band lit
//! inside it, the headroom either side as numbers, and the band itself drawn
//! out at full width with what lives in it.
//!
//! **Linear, and so mostly empty.** The ISM band is 83.5 MHz of a HackRF's
//! 6 GHz, about one cell in seventy, and that is the picture: a log scale would
//! make the band look like a fifth of what the tuner does, which is a claim
//! about the radio nobody measured. The second row is where the band gets its
//! width, as its own ruler.
//!
//! **Every position is a frequency, not a layout choice.** The BLE advertising
//! channels sit where 2402, 2426 and 2480 MHz fall on the ruler
//! (`signal::ble::channel`), and the classic grid's span is its own 2402 to
//! 2480 MHz (`signal::bt::channel`), so 38 lands a third of the way along
//! because 2426 MHz is a third of the way along.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::hardware::DeviceCapabilities;
use crate::signal::net::band::{HIGH_HZ as ISM_HIGH_HZ, LOW_HZ as ISM_LOW_HZ};
use crate::signal::net::gate::reaches_band;
use crate::ui::chrome::{field, section};

use super::LABEL;

/// The cell of a `cells`-wide row that `hz` falls in, on a linear scale from
/// `lo` to `hi`. The first cell is `lo`, the last is `hi`.
fn cell(hz: f64, lo: f64, hi: f64, cells: usize) -> usize {
    if cells < 2 || hi <= lo {
        return 0;
    }
    let t = ((hz - lo) / (hi - lo)).clamp(0.0, 1.0);
    (t * (cells - 1) as f64).round() as usize
}

fn mhz(hz: f64) -> String {
    let m = hz / 1e6;
    if m.fract() == 0.0 {
        format!("{m:.0}")
    } else {
        format!("{m:.1}")
    }
}

/// The zone, `iw` columns wide.
pub(super) fn lines(
    caps: &DeviceCapabilities,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let (tmin, tmax) = (caps.freq_min_hz as f64, caps.freq_max_hz as f64);
    let (blo, bhi) = (ISM_LOW_HZ as f64, ISM_HIGH_HZ as f64);
    let hint = format!("{:.3} to {:.3} MHz", tmin / 1e6, tmax / 1e6);
    let mut out = vec![section("tuner", &hint, iw, theme)];
    out.push(range_row(tmin, tmax, blo, bhi, iw, theme));
    out.push(headroom_row(caps, tmin, tmax, blo, bhi, iw, theme));
    out.push(ruler_row(iw, theme));
    out.push(marker_row(iw, theme));
    out.push(legend(iw, theme));
    out
}

/// `RANGE  1 ░░░░░░█░░░░░░░░░ 6000 MHz`: the tuner's range, the band lit.
fn range_row(
    tmin: f64,
    tmax: f64,
    blo: f64,
    bhi: f64,
    iw: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    // The scale holds the band even when the tuner does not, so a radio that
    // falls short is drawn falling short rather than having the band clipped
    // to its edge.
    let (lo, hi) = (tmin.min(blo), tmax.max(bhi));
    let left = format!("{} ", mhz(lo));
    let right = format!(" {} MHz", mhz(hi));
    let cells = iw.saturating_sub(LABEL + 1 + left.len() + right.len());
    let covered = reaches_band_hz(tmin, tmax, blo, bhi);
    let (b0, b1) = (cell(blo, lo, hi, cells), cell(bhi, lo, hi, cells));
    let mut spans = vec![
        field("RANGE", LABEL, theme),
        Span::styled(left, Style::default().fg(theme.label)),
    ];
    let mut run = String::new();
    let mut run_style = Style::default();
    let flush = |spans: &mut Vec<Span<'static>>, run: &mut String, style: Style| {
        if !run.is_empty() {
            spans.push(Span::styled(std::mem::take(run), style));
        }
    };
    for i in 0..cells {
        let at = lo + (hi - lo) * i as f64 / (cells.max(2) - 1) as f64;
        let (ch, style) = if (b0..=b1).contains(&i) {
            let ink = if covered {
                theme.value_hi
            } else {
                theme.status_crit
            };
            ('█', Style::default().fg(ink))
        } else if at >= tmin && at <= tmax {
            ('░', Style::default().fg(theme.border_dim))
        } else {
            (' ', Style::default())
        };
        if style != run_style {
            flush(&mut spans, &mut run, run_style);
            run_style = style;
        }
        run.push(ch);
    }
    flush(&mut spans, &mut run, run_style);
    spans.push(Span::styled(right, Style::default().fg(theme.label)));
    Line::from(spans)
}

fn reaches_band_hz(tmin: f64, tmax: f64, blo: f64, bhi: f64) -> bool {
    tmin <= blo && tmax >= bhi
}

/// How much tuner there is below and above the band, or how far short it
/// falls. Exact, from the capability record. The verdict words go first when
/// the zone is narrow, then the "above" figure: each part is drawn whole or
/// not at all.
fn headroom_row(
    caps: &DeviceCapabilities,
    tmin: f64,
    tmax: f64,
    blo: f64,
    bhi: f64,
    iw: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    let side = |gap: f64, where_: &str| {
        if gap >= 0.0 {
            format!("{:.3} MHz {where_}", gap / 1e6)
        } else {
            format!("{:.3} MHz short {where_}", -gap / 1e6)
        }
    };
    let verdict = if reaches_band(caps) {
        ("covers the band", theme.status_ok)
    } else {
        ("does not cover the band", theme.status_crit)
    };
    let parts = [
        (side(blo - tmin, "below"), theme.value),
        (side(tmax - bhi, "above"), theme.value),
        (verdict.0.to_string(), verdict.1),
    ];
    let mut spans = vec![field("", LABEL, theme)];
    let mut used = LABEL + 1;
    for (i, (text, ink)) in parts.into_iter().enumerate() {
        let gap = if i == 0 { 0 } else { 3 };
        if used + gap + text.chars().count() > iw {
            break;
        }
        used += gap + text.chars().count();
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(text, Style::default().fg(ink)));
    }
    Line::from(spans)
}

/// The width left for a band ruler after its label and end marks.
fn ruler_cells(iw: usize) -> usize {
    iw.saturating_sub(LABEL + 1 + "2400 ".len() + " 2483.5".len())
}

/// `BAND  2400 ··━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━·· 2483.5`: the ISM
/// band at full width, with the classic grid's span drawn in.
fn ruler_row(iw: usize, theme: &crate::Theme) -> Line<'static> {
    use crate::signal::bt::channel::{HIGH_HZ as BT_HIGH_HZ, LOW_HZ as BT_LOW_HZ};
    let (lo, hi) = (ISM_LOW_HZ as f64, ISM_HIGH_HZ as f64);
    let cells = ruler_cells(iw);
    let (c0, c1) = (
        cell(BT_LOW_HZ as f64, lo, hi, cells),
        cell(BT_HIGH_HZ as f64, lo, hi, cells),
    );
    let before = c0;
    let grid = (c1 + 1).saturating_sub(c0).min(cells);
    let after = cells.saturating_sub(before + grid);
    Line::from(vec![
        field("BAND", LABEL, theme),
        Span::styled(format!("{} ", mhz(lo)), Style::default().fg(theme.label)),
        Span::styled("·".repeat(before), Style::default().fg(theme.border_dim)),
        Span::styled("━".repeat(grid), Style::default().fg(theme.value)),
        Span::styled("·".repeat(after), Style::default().fg(theme.border_dim)),
        Span::styled(format!(" {}", mhz(hi)), Style::default().fg(theme.label)),
    ])
}

/// The key to the ruler's two marks, in the longest form that fits: whole,
/// short, or not at all on a zone too narrow for either.
fn legend(iw: usize, theme: &crate::Theme) -> Line<'static> {
    let indent = LABEL + 1 + "2400 ".len();
    let key = [
        "━ classic BT, 79 channels   ▲ BLE advertising",
        "━ classic BT   ▲ BLE adv",
    ]
    .into_iter()
    .find(|k| indent + k.chars().count() <= iw)
    .unwrap_or("");
    Line::from(vec![
        Span::raw(" ".repeat(indent)),
        Span::styled(key, Style::default().fg(theme.label)),
    ])
}

/// `▲37 ▲38 ▲39` under the ruler, each where its frequency falls.
fn marker_row(iw: usize, theme: &crate::Theme) -> Line<'static> {
    let (lo, hi) = (ISM_LOW_HZ as f64, ISM_HIGH_HZ as f64);
    let cells = ruler_cells(iw);
    let indent = LABEL + 1 + "2400 ".len();
    let mut row: Vec<char> = vec![' '; cells + 3];
    for (channel, hz) in [37u8, 38, 39]
        .into_iter()
        .zip(crate::signal::ble::channel::advertising_channels_hz())
    {
        let at = cell(hz as f64, lo, hi, cells);
        // The number follows the mark; near the right edge it would run off,
        // so it goes before instead.
        let text: Vec<char> = if at + 3 <= cells + 2 {
            format!("▲{channel}").chars().collect()
        } else {
            format!("{channel}▲").chars().collect()
        };
        let start = if text[0] == '▲' {
            at
        } else {
            at.saturating_sub(2)
        };
        for (k, ch) in text.into_iter().enumerate() {
            if let Some(slot) = row.get_mut(start + k) {
                *slot = ch;
            }
        }
    }
    let marks: String = row.into_iter().collect::<String>().trim_end().to_string();
    Line::from(vec![
        Span::raw(" ".repeat(indent)),
        Span::styled(marks, Style::default().fg(theme.value_hi)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SdrMetrics;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// The fixture is a HackRF, 1 MHz to 6 GHz: 2399 MHz of tuner below the
    /// band and 3516.5 above, stated exactly, and the band lit a little
    /// under halfway along a linear scale, as one or two cells.
    #[test]
    fn the_span_shows_where_the_band_sits_in_the_tuner() {
        let m = SdrMetrics::fixture();
        let rows = text(&lines(&m.caps, 80, &crate::Theme::sdr()));
        let all = rows.join("\n");
        assert!(all.contains("2399.000 MHz below"), "{all}");
        assert!(all.contains("3516.500 MHz above"), "{all}");
        assert!(all.contains("covers the band"), "{all}");
        let range = &rows[1];
        let first = range.find('░').unwrap();
        let lit = range.find('█').unwrap();
        let last = range.rfind('░').unwrap();
        let t = (lit - first) as f64 / (last - first) as f64;
        assert!((t - 0.40).abs() < 0.03, "{t}: {range}");
        assert!(range.matches('█').count() <= 2, "{range}");
    }

    /// The advertising channels are where their frequencies are: 38 about a
    /// third of the way along the ruler, 39 near its end, and the classic
    /// grid's span starts and ends two cells' worth of MHz in from the band's
    /// edges.
    #[test]
    fn the_band_ruler_puts_each_channel_at_its_frequency() {
        let m = SdrMetrics::fixture();
        let rows = text(&lines(&m.caps, 100, &crate::Theme::sdr()));
        let (ruler, marks) = (&rows[3], &rows[4]);
        let start = ruler.find("2400 ").unwrap() + "2400 ".len();
        let end = ruler.find(" 2483.5").unwrap();
        let col = |needle: &str| marks.find(needle).unwrap() as f64;
        // Char columns, not bytes: `▲` is three bytes.
        let chars = |s: &str, byte: usize| s[..byte].chars().count() as f64;
        let (s, e) = (chars(ruler, start), chars(ruler, end));
        let at = |n: &str| (chars(marks, col(n) as usize) - s) / (e - 1.0 - s);
        assert!((at("▲37") - 2.0 / 83.5).abs() < 0.03, "{marks}");
        assert!((at("▲38") - 26.0 / 83.5).abs() < 0.03, "{marks}");
        assert!(at("39") > 0.9, "{marks}");
        assert!(ruler.contains('━') && ruler.starts_with(" BAND"), "{ruler}");
        assert!(rows[5].contains("BLE advertising"), "{}", rows[5]);
    }

    /// A radio that stops short of the band is drawn stopping short, with the
    /// shortfall in MHz, and the band is not clipped to the tuner's edge.
    #[test]
    fn a_tuner_that_falls_short_is_drawn_short() {
        let mut m = SdrMetrics::fixture();
        let mut caps = (*m.caps).clone();
        caps.freq_min_hz = 24_000_000;
        caps.freq_max_hz = 1_766_000_000;
        m.caps = std::sync::Arc::new(caps);
        let rows = text(&lines(&m.caps, 80, &crate::Theme::sdr()));
        let all = rows.join("\n");
        assert!(all.contains("717.500 MHz short above"), "{all}");
        assert!(all.contains("does not cover the band"), "{all}");
        let range = &rows[1];
        assert!(
            range.rfind('░').unwrap() < range.find('█').unwrap(),
            "{range}"
        );
    }

    /// No row runs past the zone at any width the panel can be given, down
    /// to the panel's declared minimum.
    #[test]
    fn the_zone_fits_every_width() {
        let m = SdrMetrics::fixture();
        for iw in 46..160 {
            for row in text(&lines(&m.caps, iw, &crate::Theme::sdr())) {
                assert!(row.chars().count() <= iw, "{iw}: {row:?}");
            }
        }
    }
}
