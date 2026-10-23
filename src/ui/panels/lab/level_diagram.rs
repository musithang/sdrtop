// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `LevelDiagramPanel` - the Gain-Staging Level Diagram, centre of the Lab RF bench.
//!
//! Plots the modeled signal level and noise floor climbing stage-by-stage
//! (ANT▸LNA▸MIX▸VGA▸ADC). The vertical gap between the two traces is the SNR: it is
//! set at the antenna and only repositioned - not improved - by gain. Reading it left
//! to right tells the whole front-end story the left panel quantifies.
//!
//! **A chain with no published noise figures gets the measured half.** The
//! noise trace needs each stage's own noise figure, which nobody publishes
//! for most radios (an Airspy R2's included), so there the panel draws what
//! is measured: the signal walked back from the converter through each
//! stage's measured gain, the floor the FFT measured at the converter, and
//! where the radio clips; and it says why there is no noise trace.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::hardware::StageReading;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};
use crate::ui::rf_calc::{cascade, level_lineup, signal_lineup, Stage, StageLevel};

pub struct LevelDiagramPanel;

/// dBm window of the y-axis (modeled / relative). Wide enough to hold the antenna
/// thermal floor at the bottom and the ADC clip line near the top.
const TOP_DBM: f64 = 10.0;
const BOT_DBM: f64 = -110.0;

/// Interpolate a node series at fractional node position `p` (piecewise linear).
fn lerp_at(vals: &[f64], p: f64) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    let lo = (p.floor() as usize).min(vals.len() - 1);
    let hi = (lo + 1).min(vals.len() - 1);
    let frac = p - lo as f64;
    vals[lo] + (vals[hi] - vals[lo]) * frac
}

/// Map a dBm level to a chart row (0 = top, `h-1` = bottom). Out-of-range clamps.
fn dbm_row(dbm: f64, h: usize) -> usize {
    let t = ((TOP_DBM - dbm) / (TOP_DBM - BOT_DBM)).clamp(0.0, 1.0);
    (t * (h.saturating_sub(1)) as f64).round() as usize
}

impl Panel for LevelDiagramPanel {
    fn name(&self) -> &'static str {
        "level_diagram"
    }
    fn min_size(&self) -> (u16, u16) {
        (40, 14)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Gain-Staging Level Diagram")
            .stale_when(Staleness::NotStreaming)
            .tag_if(state.lab.rf_freeze.is_some(), Tag::Frozen)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let stale = !state.radio.hw_streaming;
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let dim = theme.border_dim;
        if !stale && !state.caps.friis_applicable {
            render_measured(f, inner, state, theme);
            return;
        }
        if stale {
            let msg = "\u{2014}\u{2014}\u{2014}".to_string();
            f.render_widget(
                Paragraph::new(Span::styled(msg, Style::default().fg(dim))),
                inner,
            );
            return;
        }

        // --- model: 5 nodes ANT, LNA, MIX, VGA, ADC (ADC = VGA output) ----------
        // Frozen snapshot when held, else the live gain/level.
        let fz = state.lab.rf_freeze.as_ref();
        let (amp, lna, vga) = fz
            .map(|f| (f.amp_enabled, f.lna_gain, f.vga_gain))
            .unwrap_or((
                state.radio.amp_enabled,
                state.shown_gain(),
                state.radio.secondary_gain(),
            ));
        let stages = cascade(amp, lna, vga);
        let adc_peak = fz
            .map(|f| f.peak_dbfs)
            .unwrap_or(state.signal.adc_peak_dbfs) as f64;
        let snr = fz.map(|f| f.snr_db).unwrap_or(state.signal.peak_to_nf_db) as f64;
        let mut nodes: Vec<StageLevel> = level_lineup(adc_peak, snr, &stages);
        if let Some(last) = nodes.last().cloned() {
            nodes.push(StageLevel {
                label: "ADC".to_string(),
                ..last
            });
        }
        let n = nodes.len();
        if n < 2 {
            return;
        }
        let sig: Vec<f64> = nodes.iter().map(|s| s.signal_dbm).collect();
        // This panel is gated on `friis_applicable`, so every node has a noise
        // figure; the fallback keeps the type total rather than unwrapping.
        let noise: Vec<f64> = nodes
            .iter()
            .map(|s| s.noise_dbm.unwrap_or(s.signal_dbm))
            .collect();

        let sig_col = theme.value_hi;
        let noise_col = theme.border_accent;
        let gap_col = dim;

        // --- layout ------------------------------------------------------------
        let iw = inner.width as usize;
        let ih = inner.height as usize;
        let gutter = 4usize; // "−110"
        let chart_w = iw.saturating_sub(gutter + 1);
        // Reserve: caption(1) + x-labels(1) + legend(1).
        let chart_h = ih.saturating_sub(3).clamp(4, 28);
        if chart_w < 8 || chart_h < 4 {
            f.render_widget(
                Paragraph::new(Span::styled(" level diagram", Style::default().fg(dim))),
                inner,
            );
            return;
        }

        let mut lines: Vec<Line> = Vec::new();

        // Caption.
        lines.push(Line::from(Span::styled(
            " level climbs stage-by-stage \u{00b7} the gap between signal and noise is the SNR",
            Style::default().fg(dim),
        )));

        // dBm gridline rows.
        let ticks = [10.0, -20.0, -50.0, -80.0, -110.0];
        let mut row_label = vec![String::new(); chart_h];
        for d in ticks {
            let r = dbm_row(d, chart_h);
            if row_label[r].is_empty() {
                row_label[r] = format!("{:>w$}", d as i32, w = gutter);
            }
        }
        // Reference lines: ADC clip at 0 dBm, and the converter's quantisation
        // floor below it. That floor was drawn at a flat -50 dBm, which is the
        // 8-bit answer and about 36 dB too high for a 14-bit radio.
        let clip_row = dbm_row(0.0, chart_h);
        let floor_row = dbm_row(
            -crate::ui::rf_calc::quantisation_snr_db(state.caps.sample_geometry.bits()),
            chart_h,
        );

        // Per-column interpolated signal/noise rows.
        let node_at = |col: usize| (col as f64 / (chart_w - 1) as f64) * (n - 1) as f64;
        let sig_row: Vec<usize> = (0..chart_w)
            .map(|c| dbm_row(lerp_at(&sig, node_at(c)), chart_h))
            .collect();
        let noise_row: Vec<usize> = (0..chart_w)
            .map(|c| dbm_row(lerp_at(&noise, node_at(c)), chart_h))
            .collect();

        for (row, g) in row_label.iter().enumerate() {
            // y gutter
            let mut spans: Vec<Span> = Vec::new();
            if g.is_empty() {
                spans.push(Span::styled(
                    format!("{:>gutter$}\u{2502}", "", gutter = gutter),
                    Style::default().fg(dim),
                ));
            } else {
                spans.push(Span::styled(
                    format!("{g}\u{2524}"),
                    Style::default().fg(dim),
                ));
            }
            // plot row
            let mut cells: Vec<(char, Color)> = Vec::with_capacity(chart_w);
            for c in 0..chart_w {
                let (ch, col) = match classify(row, sig_row[c], noise_row[c]) {
                    Cell::Signal => ('\u{2588}', sig_col),           // █ signal
                    Cell::Noise => ('\u{25ac}', noise_col),          // ▬ noise
                    Cell::UsableDr => ('\u{2591}', gap_col),         // ░ usable DR
                    Cell::Buried => ('\u{2592}', theme.status_warn), // ▒ signal buried
                    Cell::Empty if row == clip_row => ('\u{00b7}', theme.status_crit),
                    Cell::Empty if row == floor_row => ('\u{00b7}', theme.status_warn),
                    Cell::Empty => (' ', dim),
                };
                cells.push((ch, col));
            }
            spans.extend(coalesce(cells));
            lines.push(Line::from(spans));
        }

        // x-axis stage labels, centred under each node column.
        let mut axis: Vec<char> = vec![' '; chart_w];
        for (i, node) in nodes.iter().enumerate() {
            let col = ((i as f64 / (n - 1) as f64) * (chart_w - 1) as f64).round() as usize;
            let lbl = node.label.as_str();
            let start = col
                .saturating_sub(lbl.len() / 2)
                .min(chart_w.saturating_sub(lbl.len()));
            for (k, ch) in lbl.chars().enumerate() {
                if start + k < chart_w {
                    axis[start + k] = ch;
                }
            }
        }
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(gutter + 1)),
            Span::styled(
                axis.into_iter().collect::<String>(),
                Style::default().fg(theme.label),
            ),
        ]));

        // Legend + key figures.
        let headroom = state.ceiling_dbfs() - adc_peak;
        let mut legend = vec![
            Span::raw(" "),
            Span::styled("\u{2588} signal", Style::default().fg(sig_col)),
            Span::styled("  \u{25ac} noise", Style::default().fg(noise_col)),
        ];
        if snr < 0.0 {
            // Signal sits below the noise at the ADC - flag the buried band.
            legend.push(Span::styled(
                "  \u{2592} buried",
                Style::default().fg(theme.status_warn),
            ));
        } else {
            legend.push(Span::styled(
                "  \u{2591} usable DR",
                Style::default().fg(gap_col),
            ));
        }
        legend.push(Span::styled(
            format!("  \u{00b7} SNR {snr:.0} dB \u{00b7} headroom {headroom:.0} dB"),
            Style::default().fg(dim),
        ));
        lines.push(Line::from(legend));

        // Self-adjusting density: drop only as many airy spacers as needed to fit,
        // spread evenly, so a short pane keeps balanced breathing room. (chrome)
        crate::ui::chrome::collapse_spacers(&mut lines, ih);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// The diagram for a chain whose stages' noise figures nobody publishes: the
/// signal at each node, from the converter's measured peak walked back
/// through each stage's measured gain (the GAIN LINEUP's walk, drawn), the
/// floor the FFT measured at the converter, and the radio's ceiling. A stage
/// whose gain is not a dB here (outside its measured band, or set by the
/// AGC) leaves every level before it unknown, and the trace starts after it.
fn render_measured(f: &mut Frame, inner: Rect, state: &SdrMetrics, theme: &crate::Theme) {
    let dim = theme.border_dim;
    let specs = state.caps.gain.stages();
    let readings: Vec<Option<StageReading>> =
        (0..specs.len()).map(|i| state.stage_reading(i)).collect();
    let stages: Vec<Stage> = specs
        .iter()
        .zip(&readings)
        .map(|(s, r)| {
            let gain = match r {
                Some(StageReading::Db(db)) => *db,
                _ => f64::NAN,
            };
            Stage::unmodelled(&s.name, gain)
        })
        .collect();
    let adc_peak = state
        .lab
        .rf_freeze
        .as_ref()
        .map(|f| f.peak_dbfs)
        .unwrap_or(state.signal.adc_peak_dbfs) as f64;
    // ANT, each stage's output (the last is what the converter sees), ADC.
    let mut sig: Vec<f64> = signal_lineup(adc_peak, &stages)
        .iter()
        .map(|l| l.signal_dbm)
        .collect();
    sig.push(adc_peak);
    let mut labels = vec!["ANT".to_string()];
    labels.extend(specs.iter().zip(&readings).map(|(s, r)| match r {
        Some(StageReading::Db(db)) => format!("{} {db:+.0}", s.name),
        Some(StageReading::Position { at, of }) => format!("{} {at}/{of}", s.name),
        Some(StageReading::Auto) => format!("{} AGC", s.name),
        None => s.name.clone(),
    }));
    labels.push("ADC".to_string());
    let n = sig.len();
    let ceiling = state.ceiling_dbfs();
    let floor = state
        .waterfall
        .last_fft
        .as_ref()
        .filter(|fr| fr.enbw_hz > 0.0)
        .map(|fr| (fr.noise_floor as f64, fr.enbw_hz));

    // The window: 0 dBFS at the top, 20 dB rows down to below the lowest
    // thing drawn.
    let lowest = sig
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .chain(floor.map(|(db, _)| db))
        .fold(f64::INFINITY, f64::min);
    let bottom = if lowest.is_finite() {
        (((lowest - 10.0) / 20.0).floor() * 20.0).min(-40.0)
    } else {
        -120.0
    };
    let iw = inner.width as usize;
    let ih = inner.height as usize;
    let gutter = 4usize;
    let chart_w = iw.saturating_sub(gutter + 1);
    // Caption, axis, legend, the figure, the note.
    let chart_h = ih.saturating_sub(5).clamp(4, 28);
    if chart_w < 8 || n < 2 {
        f.render_widget(
            Paragraph::new(Span::styled(" level diagram", Style::default().fg(dim))),
            inner,
        );
        return;
    }
    let row_of = |db: f64| -> usize {
        let t = ((0.0 - db) / (0.0 - bottom)).clamp(0.0, 1.0);
        (t * (chart_h - 1) as f64).round() as usize
    };
    let node_at = |col: usize| (col as f64 / (chart_w - 1) as f64) * (n - 1) as f64;

    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        " signal measured at the converter, walked back stage by stage",
        Style::default().fg(dim),
    ))];
    let mut row_label = vec![String::new(); chart_h];
    let mut tick = 0.0;
    while tick >= bottom {
        let r = row_of(tick);
        if row_label[r].is_empty() {
            row_label[r] = format!("{:>w$}", tick as i32, w = gutter);
        }
        tick -= 20.0;
    }
    let ceiling_row = row_of(ceiling);
    let floor_row = floor.map(|(db, _)| row_of(db));
    // The floor is measured at the converter: drawn over the last segment.
    let floor_from = (((n - 2) as f64 / (n - 1) as f64) * (chart_w - 1) as f64).round() as usize;
    let sig_row: Vec<Option<usize>> = (0..chart_w)
        .map(|c| {
            let p = node_at(c);
            let lo = (p.floor() as usize).min(n - 1);
            let hi = (lo + 1).min(n - 1);
            (sig[lo].is_finite() && sig[hi].is_finite()).then(|| row_of(lerp_at(&sig, p)))
        })
        .collect();
    for (row, g) in row_label.iter().enumerate() {
        let mut spans = vec![Span::styled(
            if g.is_empty() {
                format!("{:>gutter$}\u{2502}", "")
            } else {
                format!("{g}\u{2524}")
            },
            Style::default().fg(dim),
        )];
        let cells: Vec<(char, Color)> = (0..chart_w)
            .map(|c| {
                if sig_row[c] == Some(row) {
                    ('\u{2588}', theme.value_hi)
                } else if floor_row == Some(row) && c >= floor_from {
                    ('\u{25ac}', theme.border_accent)
                } else if row == ceiling_row && c % 2 == 0 {
                    ('\u{00b7}', theme.status_crit)
                } else {
                    (' ', dim)
                }
            })
            .collect();
        spans.extend(coalesce(cells));
        lines.push(Line::from(spans));
    }

    // Node labels under their columns, left to right, a label that would
    // run into the one before it left out rather than overprinted.
    let mut axis: Vec<char> = vec![' '; chart_w];
    let mut free_from = 0usize;
    for (i, label) in labels.iter().enumerate() {
        let col = ((i as f64 / (n - 1) as f64) * (chart_w - 1) as f64).round() as usize;
        let len = label.chars().count();
        let start = col.saturating_sub(len / 2).min(chart_w.saturating_sub(len));
        if start < free_from {
            continue;
        }
        for (k, ch) in label.chars().enumerate() {
            if start + k < chart_w {
                axis[start + k] = ch;
            }
        }
        free_from = start + len + 1;
    }
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(gutter + 1)),
        Span::styled(
            axis.into_iter().collect::<String>(),
            Style::default().fg(theme.label),
        ),
    ]));

    let clips = match state.caps.gain.ceiling_part() {
        Some(part) => format!("{part} clips {ceiling:.0} dBFS"),
        None => "clips 0 dBFS".to_string(),
    };
    let floor_text = match floor {
        Some((db, enbw)) => format!(
            "floor {db:.0} dBFS/{}",
            crate::ui::widgets::micro_common::fmt_rbw(enbw)
        ),
        None => "no floor measured yet".to_string(),
    };
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "\u{2588} signal, dBFS at the converter",
            Style::default().fg(theme.value_hi),
        ),
        Span::styled(
            format!("  \u{25ac} {floor_text}"),
            Style::default().fg(theme.border_accent),
        ),
    ]));
    // The ceiling on a line of its own with its distance to the floor: one
    // legend line could not hold both at a laptop's width.
    let mut ceiling_spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("\u{00b7} {clips}"),
            Style::default().fg(theme.status_crit),
        ),
    ];
    if let Some((db, enbw)) = floor {
        ceiling_spans.push(Span::styled(
            format!(
                ", {:.0} dB above the floor per {}",
                ceiling - db,
                crate::ui::widgets::micro_common::fmt_rbw(enbw)
            ),
            Style::default().fg(dim),
        ));
    }
    lines.push(Line::from(ceiling_spans));
    lines.push(Line::from(Span::styled(
        format!(" no noise trace: {}", state.caps.gain.no_cascade_reason()),
        Style::default().fg(dim),
    )));
    crate::ui::chrome::collapse_spacers(&mut lines, ih);
    f.render_widget(Paragraph::new(lines), inner);
}

/// What a chart row holds for a column, from the signal-trace row `sr` and the
/// noise-trace row `nr` (rows grow downward, so a *smaller* row is a *higher* level).
/// The `Buried` case keeps the diagram honest when the noise climbs above the signal
/// (negative SNR): the band between them is flagged instead of left blank.
#[derive(PartialEq, Debug)]
enum Cell {
    Signal,
    Noise,
    UsableDr,
    Buried,
    Empty,
}

fn classify(row: usize, sr: usize, nr: usize) -> Cell {
    if row == sr {
        Cell::Signal
    }
    // signal wins ties (SNR ≈ 0)
    else if row == nr {
        Cell::Noise
    } else if row > sr.min(nr) && row < sr.max(nr) {
        if sr < nr {
            Cell::UsableDr
        } else {
            Cell::Buried
        }
    } else {
        Cell::Empty
    }
}

/// Coalesce a per-cell `(char, colour)` row into runs of same-coloured spans.
fn coalesce(cells: Vec<(char, Color)>) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_col: Option<Color> = None;
    for (ch, col) in cells {
        if run_col != Some(col) {
            if let Some(c) = run_col {
                spans.push(Span::styled(
                    std::mem::take(&mut run),
                    Style::default().fg(c),
                ));
            }
            run_col = Some(col);
        }
        run.push(ch);
    }
    if let Some(c) = run_col {
        spans.push(Span::styled(run, Style::default().fg(c)));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbm_row_maps_top_and_bottom() {
        assert_eq!(dbm_row(TOP_DBM, 10), 0, "top of range → row 0");
        assert_eq!(dbm_row(BOT_DBM, 10), 9, "bottom → last row");
        // Clamps beyond the window.
        assert_eq!(dbm_row(100.0, 10), 0);
        assert_eq!(dbm_row(-500.0, 10), 9);
    }

    #[test]
    fn lerp_at_interpolates_between_nodes() {
        let v = [0.0, 10.0, 20.0];
        assert!((lerp_at(&v, 0.0) - 0.0).abs() < 1e-9);
        assert!((lerp_at(&v, 0.5) - 5.0).abs() < 1e-9);
        assert!((lerp_at(&v, 2.0) - 20.0).abs() < 1e-9);
        assert!(
            (lerp_at(&v, 9.0) - 20.0).abs() < 1e-9,
            "past the end clamps to last"
        );
    }

    #[test]
    fn panel_name() {
        assert_eq!(LevelDiagramPanel.name(), "level_diagram");
    }

    #[test]
    fn classify_normal_signal_above_noise() {
        // signal at row 2 (higher level), noise at row 6 (lower level)
        assert_eq!(classify(2, 2, 6), Cell::Signal);
        assert_eq!(classify(6, 2, 6), Cell::Noise);
        assert_eq!(
            classify(4, 2, 6),
            Cell::UsableDr,
            "between them = usable DR"
        );
        assert_eq!(classify(0, 2, 6), Cell::Empty);
        assert_eq!(classify(9, 2, 6), Cell::Empty);
    }

    #[test]
    fn classify_inverted_noise_above_signal() {
        // Noise louder than signal: noise at row 2, signal at row 6.
        assert_eq!(classify(2, 6, 2), Cell::Noise);
        assert_eq!(classify(6, 6, 2), Cell::Signal);
        assert_eq!(classify(4, 6, 2), Cell::Buried, "buried band, not blank");
    }

    #[test]
    fn classify_coincident_is_signal() {
        // SNR ≈ 0: the two traces land on the same row → signal drawn, no blank gap.
        assert_eq!(classify(3, 3, 3), Cell::Signal);
    }

    fn r2(hz: u64) -> SdrMetrics {
        let mut m = SdrMetrics::fixture()
            .streaming()
            .with_carrier(0.0, 40.0)
            .band_measured_chain()
            .on_curve(0, 10);
        m.radio.frequency = hz;
        m.signal.adc_peak_dbfs = -27.0;
        m
    }

    /// **On a chain with no published noise figures the diagram is the
    /// measured half**: the signal walked back from the converter through
    /// each stage's measured gain, the floor the FFT measured at the
    /// converter, and where this radio clips; it says why there is no noise
    /// trace instead of leaving the panel empty.
    #[test]
    fn an_r2_gets_the_measured_diagram() {
        // At the Lab RF bench's own width on a laptop, where a longer legend
        // was cut.
        let out =
            crate::state::fixture::draw(LevelDiagramPanel, 73, 26, &r2(92_800_000)).join("\n");
        for want in [
            "ANT",
            "LNA +17",
            "MIX +1",
            "VGA +34",
            "ADC",
            "\u{2588}",
            "floor -100 dBFS/",
            "tuner clips -3 dBFS",
            "97 dB above the floor per",
            "\u{25ac} floor -100 dBFS/39.1 kHz",
            "no noise trace: each stage's own noise figure is not published",
        ] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
    }

    /// Outside the LNA's band its gain reads as its step and the levels
    /// before it are unknown, so the trace starts after it; under the AGC
    /// the stages it sets read AGC.
    #[test]
    fn the_measured_diagram_stops_where_a_gain_is_not_known() {
        let far =
            crate::state::fixture::draw(LevelDiagramPanel, 90, 22, &r2(943_400_000)).join("\n");
        assert!(far.contains("LNA 6/14"), "{far}");
        assert!(!far.contains("NaN"), "{far}");
        let mut agc = r2(92_800_000);
        agc.radio.amp_enabled = true;
        let out = crate::state::fixture::draw(LevelDiagramPanel, 90, 22, &agc).join("\n");
        assert!(out.contains("LNA AGC") && out.contains("MIX AGC"), "{out}");
    }

    /// A radio with no ceiling of its own clips at the converter, and says
    /// that instead.
    #[test]
    fn a_chain_with_no_ceiling_clips_at_the_converter() {
        let m = SdrMetrics::fixture()
            .streaming()
            .named_chain()
            .with_carrier(0.0, 20.0);
        let out = crate::state::fixture::draw(LevelDiagramPanel, 90, 22, &m).join("\n");
        assert!(out.contains("clips 0 dBFS"), "{out}");
        assert!(!out.contains("tuner"), "{out}");
    }
}
