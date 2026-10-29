// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `LevelDiagramPanel` - the Gain-Staging Level Diagram, centre of the Lab RF bench.
//!
//! The chain is the x-axis (`ANT ─┤LNA├─┤MIX├─┤VGA├─▶ ADC`) and the signal is
//! a **staircase** on it: the level holds between two stages and steps at
//! each one, so every step is that stage's gain and every tread is a node's
//! level. It used to be a ramp interpolated between nodes, which drew levels
//! at places in the chain where nothing has a level, and was a wall of blocks
//! besides.
//!
//! At the right edge is the converter's window, the one place where every
//! line is measured: where the radio clips, where the signal sits, where the
//! FFT's floor is, and the distances between them.
//!
//! **Measured and modelled are drawn apart.** The signal is the converter's
//! measured peak walked back through each stage's gain, a solid line. The
//! floor is the FFT's, solid, and only where it was measured: at the
//! converter. On a chain whose stages' noise figures sdrtop knows (the
//! HackRF), the noise is walked back from that floor through them, dashed,
//! because it is a model. On every other radio nobody publishes those
//! figures (an Airspy R2's included), so there is no noise walk, and the
//! panel says why instead of drawing one.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::hardware::StageReading;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};
use crate::ui::rf_calc::{cascade, level_lineup, signal_lineup, Stage};

pub struct LevelDiagramPanel;

/// The y gutter: `-120┤`.
const GUTTER: usize = 5;

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
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if !state.radio.hw_streaming {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "\u{2014}\u{2014}\u{2014}",
                    Style::default().fg(theme.border_dim),
                )),
                inner,
            );
            return;
        }
        let lines = diagram(&chain_of(state), state, inner.width, inner.height, theme);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// What the diagram draws, gathered once from the live or the held state.
struct Chain {
    /// Each stage's name, and what is written under it (its gain, its step,
    /// or `AGC`).
    stages: Vec<(String, String)>,
    /// The signal at each node, dBFS: the antenna, then each stage's output,
    /// the last being what the converter sees. NaN where it is not known.
    signal: Vec<f64>,
    /// The noise at the same nodes, from the stages' noise figures; `None`
    /// where those figures are not known, or no floor anchors them.
    noise: Option<Vec<f64>>,
    /// The converter's measured peak, dBFS.
    adc_peak: f64,
    /// The FFT's floor at the converter and the bandwidth it was measured in
    /// (dBFS, Hz).
    floor: Option<(f64, f64)>,
}

/// The chain as the diagram draws it: the held snapshot when the bench is
/// frozen, else the live state.
fn chain_of(state: &SdrMetrics) -> Chain {
    let fz = state.lab.rf_freeze.as_ref();
    let adc_peak = fz
        .map(|f| f.peak_dbfs)
        .unwrap_or(state.signal.adc_peak_dbfs) as f64;
    let floor = match fz {
        Some(f) => f.floor.map(|(db, enbw)| (db as f64, enbw)),
        None => state
            .waterfall
            .last_fft
            .as_ref()
            .filter(|fr| fr.enbw_hz > 0.0)
            .map(|fr| (fr.noise_floor as f64, fr.enbw_hz)),
    };

    if state.caps.friis_applicable {
        let (amp, lna, vga) = fz
            .map(|f| (f.amp_enabled, f.lna_gain, f.vga_gain))
            .unwrap_or((
                state.radio.amp_enabled,
                state.shown_gain(),
                state.radio.secondary_gain(),
            ));
        let stages = cascade(amp, lna, vga);
        // The noise is anchored on the floor the FFT measured, so its last
        // node is that floor. With no floor there is nothing to anchor it on.
        let lineup = level_lineup(
            adc_peak,
            floor.map(|(db, _)| adc_peak - db).unwrap_or(f64::NAN),
            &stages,
        );
        return Chain {
            stages: stages
                .iter()
                .map(|s| (s.label.clone(), format!("{:+.0}", s.gain_db + 0.0)))
                .collect(),
            signal: lineup.iter().map(|l| l.signal_dbm).collect(),
            noise: floor.map(|_| {
                lineup
                    .iter()
                    .map(|l| l.noise_dbm.unwrap_or(f64::NAN))
                    .collect()
            }),
            adc_peak,
            floor,
        };
    }

    let specs = state.caps.gain.stages();
    let readings: Vec<Option<StageReading>> = match fz {
        Some(f) => f.readings.clone(),
        None => (0..specs.len()).map(|i| state.stage_reading(i)).collect(),
    };
    let reading = |i: usize| readings.get(i).copied().flatten();
    // A stage whose gain is not a dB here (outside its measured band, or set
    // by the AGC) leaves every level before it unknown.
    let stages: Vec<Stage> = specs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let gain = match reading(i) {
                Some(StageReading::Db(db)) => db,
                _ => f64::NAN,
            };
            Stage::unmodelled(&s.name, gain)
        })
        .collect();
    Chain {
        stages: specs
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let under = match reading(i) {
                    Some(StageReading::Db(db)) => format!("{:+.0}", db + 0.0),
                    Some(StageReading::Position { at, of }) => format!("{at}/{of}"),
                    Some(StageReading::Auto) => "AGC".to_string(),
                    None => String::new(),
                };
                (s.name.clone(), under)
            })
            .collect(),
        signal: signal_lineup(adc_peak, &stages)
            .iter()
            .map(|l| l.signal_dbm)
            .collect(),
        noise: None,
        adc_peak,
        floor,
    }
}

/// How a step trace is drawn: the run along a tread, the riser, and the four
/// corners where a riser leaves one tread and meets the next.
struct Pen {
    run: char,
    riser: char,
    up_from: char,
    up_to: char,
    down_from: char,
    down_to: char,
}

/// The signal: measured, heavy.
const HEAVY: Pen = Pen {
    run: '\u{2501}',
    riser: '\u{2503}',
    up_from: '\u{251b}',
    up_to: '\u{250f}',
    down_from: '\u{2513}',
    down_to: '\u{2517}',
};

/// The noise: modelled, dashed.
const DASHED: Pen = Pen {
    run: '\u{2504}',
    riser: '\u{2506}',
    up_from: '\u{2518}',
    up_to: '\u{250c}',
    down_from: '\u{2510}',
    down_to: '\u{2514}',
};

/// The 20 dB grid's dot; a trace or a label may take its cell.
const GRID_DOT: char = '\u{00b7}';

/// A plot drawn cell by cell, then cut into styled lines.
struct Grid {
    cells: Vec<Vec<(char, Color, bool)>>,
}

impl Grid {
    fn new(w: usize, h: usize, ink: Color) -> Self {
        Grid {
            cells: vec![vec![(' ', ink, false); w]; h],
        }
    }
    fn set(&mut self, r: usize, x: usize, ch: char, col: Color) {
        if let Some(c) = self.cells.get_mut(r).and_then(|row| row.get_mut(x)) {
            *c = (ch, col, false);
        }
    }
    /// Nothing but the background or the grid's dot is there.
    fn free(&self, r: usize, x: usize) -> bool {
        self.cells
            .get(r)
            .and_then(|row| row.get(x))
            .is_some_and(|c| c.0 == ' ' || c.0 == GRID_DOT)
    }
    fn text(&mut self, r: usize, x: usize, s: &str, col: Color) {
        for (k, ch) in s.chars().enumerate() {
            self.set(r, x + k, ch, col);
        }
    }
    /// Writes `s` only where every cell it needs is free, so a label never
    /// overprints a trace.
    fn text_if_free(&mut self, r: usize, x: usize, s: &str, col: Color) -> bool {
        let fits = (0..s.chars().count()).all(|k| self.free(r, x + k));
        if fits {
            self.text(r, x, s, col);
        }
        fits
    }
    fn bold(&mut self, r: usize, x: usize, n: usize) {
        for k in 0..n {
            if let Some(c) = self.cells.get_mut(r).and_then(|row| row.get_mut(x + k)) {
                c.2 = true;
            }
        }
    }
    fn line(&self, r: usize) -> Line<'static> {
        let mut spans = Vec::new();
        let mut run = String::new();
        let mut cur: Option<(Color, bool)> = None;
        let style = |(c, b): (Color, bool)| {
            let s = Style::default().fg(c);
            if b {
                s.add_modifier(Modifier::BOLD)
            } else {
                s
            }
        };
        for &(ch, col, b) in &self.cells[r] {
            if cur != Some((col, b)) {
                if let Some(c) = cur {
                    spans.push(Span::styled(std::mem::take(&mut run), style(c)));
                }
                cur = Some((col, b));
            }
            run.push(ch);
        }
        if let Some(c) = cur {
            spans.push(Span::styled(run, style(c)));
        }
        Line::from(spans)
    }
}

/// Where the treads and risers fall across a plot `w` columns wide, for
/// `stages` stages: one tread a node, a riser at each stage.
struct Steps {
    treads: Vec<(usize, usize)>,
    risers: Vec<usize>,
}

impl Steps {
    fn new(w: usize, stages: usize) -> Self {
        let risers: Vec<usize> = (1..=stages).map(|k| k * w / (stages + 1)).collect();
        let mut treads = Vec::with_capacity(stages + 1);
        let mut from = 0;
        for &r in &risers {
            treads.push((from, r.saturating_sub(1)));
            from = r + 1;
        }
        treads.push((from, w.saturating_sub(1)));
        Steps { treads, risers }
    }
}

/// A staircase through `rows` (one a node, `None` where the level is not
/// known), its treads drawn for the first `runs` nodes. `over` lets it draw
/// over what is already there; without it, it only fills free cells.
#[allow(clippy::too_many_arguments)]
fn staircase(
    g: &mut Grid,
    x0: usize,
    steps: &Steps,
    rows: &[Option<usize>],
    runs: usize,
    pen: &Pen,
    col: Color,
    over: bool,
) {
    let put = |g: &mut Grid, r: usize, x: usize, ch: char| {
        if over || g.free(r, x0 + x) {
            g.set(r, x0 + x, ch, col);
        }
    };
    for (j, &(a, b)) in steps.treads.iter().enumerate().take(runs) {
        if let Some(r) = rows[j] {
            for x in a..=b {
                put(g, r, x, pen.run);
            }
        }
    }
    for (k, &x) in steps.risers.iter().enumerate() {
        match (rows[k], rows[k + 1]) {
            (Some(from), Some(to)) if from == to => put(g, from, x, pen.run),
            (Some(from), Some(to)) if to < from => {
                put(g, from, x, pen.up_from);
                put(g, to, x, pen.up_to);
                for r in to + 1..from {
                    put(g, r, x, pen.riser);
                }
            }
            (Some(from), Some(to)) => {
                put(g, from, x, pen.down_from);
                put(g, to, x, pen.down_to);
                for r in from + 1..to {
                    put(g, r, x, pen.riser);
                }
            }
            // Nothing known before this stage: the trace starts here.
            (None, Some(to)) => put(g, to, x, pen.run),
            _ => {}
        }
    }
}

/// The whole panel body, `w × h`.
fn diagram(
    c: &Chain,
    state: &SdrMetrics,
    w: u16,
    h: u16,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let dim = theme.border_dim;
    let (iw, ih) = (w as usize, h as usize);
    // The converter's window: its words where they fit, its numbers where not.
    let wide = iw >= 66;
    let zone = if wide { 18 } else { 11 };
    let cw = iw.saturating_sub(GUTTER + zone);
    let foot = if c.noise.is_some() || state.caps.friis_applicable {
        3
    } else {
        4
    };
    let ch = ih.saturating_sub(1 + foot);
    let s = c.stages.len();
    if cw < 20 || ch < 6 || c.signal.len() != s + 1 {
        return vec![Line::from(Span::styled(
            " level diagram",
            Style::default().fg(dim),
        ))];
    }

    // 0 dBFS at the top, 20 dB lines down to below the lowest thing drawn.
    let lowest = c
        .signal
        .iter()
        .chain(c.noise.iter().flatten())
        .copied()
        .chain(c.floor.map(|(db, _)| db))
        .filter(|v| v.is_finite())
        .fold(f64::INFINITY, f64::min);
    let bottom = if lowest.is_finite() {
        (((lowest - 8.0) / 20.0).floor() * 20.0).min(-40.0)
    } else {
        -120.0
    };
    let row_of = |db: f64| -> Option<usize> {
        db.is_finite()
            .then(|| ((-db / -bottom).clamp(0.0, 1.0) * (ch - 1) as f64).round() as usize)
    };
    let steps = Steps::new(cw, s);
    let last = steps.treads[s];
    let zx = GUTTER + cw;

    let mut g = Grid::new(iw, ch, theme.value);
    let mut tick = 0.0;
    while tick >= bottom {
        if let Some(r) = row_of(tick) {
            g.text(r, 0, &format!("{:>4}\u{2524}", tick as i32), dim);
            for x in (GUTTER..zx).step_by(3) {
                g.set(r, x, GRID_DOT, dim);
            }
        }
        tick -= 20.0;
    }
    for r in 0..ch {
        if g.free(r, GUTTER - 1) {
            g.set(r, GUTTER - 1, '\u{2502}', dim);
        }
    }

    let ceiling = state.ceiling_dbfs();
    let ceil_row = row_of(ceiling);
    let floor_row = c.floor.and_then(|(db, _)| row_of(db));
    let sig_row = row_of(c.adc_peak);
    // The ceiling and the floor belong to the converter: the last tread.
    if let Some(r) = ceil_row {
        for x in last.0..=last.1 {
            g.set(r, GUTTER + x, '\u{2508}', theme.status_crit);
        }
    }
    if let Some(r) = floor_row {
        for x in last.0..=last.1 {
            g.set(r, GUTTER + x, '\u{2500}', theme.border_accent);
        }
    }
    if let Some(noise) = &c.noise {
        let mut rows: Vec<Option<usize>> = noise.iter().map(|&v| row_of(v)).collect();
        rows[s] = floor_row;
        staircase(
            &mut g,
            GUTTER,
            &steps,
            &rows,
            s,
            &DASHED,
            theme.border_accent,
            false,
        );
    }
    let rows: Vec<Option<usize>> = c.signal.iter().map(|&v| row_of(v)).collect();
    staircase(
        &mut g,
        GUTTER,
        &steps,
        &rows,
        s + 1,
        &HEAVY,
        theme.value_hi,
        true,
    );
    // Each node's level over its tread; the converter's is in the window.
    for (j, &(a, b)) in steps.treads.iter().enumerate().take(s) {
        if let Some(r) = rows[j] {
            let t = format!("{:.0}", c.signal[j]);
            let x = GUTTER + ((a + b) / 2).saturating_sub(t.len() / 2);
            if r == 0 || !g.text_if_free(r - 1, x, &t, theme.label) {
                g.text_if_free(r + 1, x, &t, theme.label);
            }
        }
    }

    // The window: a ruler from the ceiling through the signal to the floor,
    // each distance written between its two ends.
    // Two ends on neighbouring rows leave no room between them, and the
    // distance goes under the lower one instead, where `under` allows.
    let span = |g: &mut Grid,
                a: Option<usize>,
                b: Option<usize>,
                under: bool,
                text: String,
                col: Color| {
        if let (Some(a), Some(b)) = (a, b) {
            let (top, bot) = (a.min(b), a.max(b));
            for r in top + 1..bot {
                g.set(r, zx, '\u{2502}', theme.label);
            }
            if bot > top + 1 {
                g.text((top + bot) / 2, zx + 2, &text, col);
            } else if under && bot != top && bot + 1 < ch {
                g.text(bot + 1, zx + 2, &text, col);
            }
        }
    };
    let headroom = crate::ui::rf_calc::shown_headroom_db(ceiling, c.adc_peak);
    span(
        &mut g,
        ceil_row,
        sig_row,
        false,
        if wide {
            format!("{headroom:.0} dB headroom")
        } else {
            format!("{headroom:.0} dB")
        },
        theme.value,
    );
    if let Some((floor, _)) = c.floor {
        let gap = c.adc_peak - floor;
        // Worded where the zone holds the words: a three-figure gap, which a
        // radio with this much range reaches, no longer fits beside them.
        let (worded, col) = if gap >= 0.0 {
            (format!("{gap:.0} dB over floor"), theme.value)
        } else {
            (format!("{:.0} dB under floor", -gap), theme.status_warn)
        };
        let text = if wide && worded.len() <= zone - 2 {
            worded
        } else {
            format!("{gap:.0} dB")
        };
        span(&mut g, sig_row, floor_row, true, text, col);
    }
    if let Some(r) = ceil_row {
        let (who, kind) = state
            .ceiling()
            .map_or(("ADC", crate::hardware::CeilingKind::Clips), |c| {
                (c.part, c.kind)
            });
        // The part's name where the zone holds it, the bare word where not:
        // the key under the plot names the part either way.
        let named = format!("{who} {} {ceiling:.0}", kind.verb());
        let text = if wide && named.chars().count() <= zone - 2 {
            named
        } else {
            format!("{} {ceiling:.0}", kind.noun())
        };
        g.set(r, zx, '\u{252c}', theme.status_crit);
        g.text(r, zx + 2, &text, theme.status_crit);
    }
    if let (Some(r), Some((floor, _))) = (floor_row, c.floor) {
        g.set(r, zx, '\u{2534}', theme.border_accent);
        let text = if wide {
            format!("floor {floor:.0}")
        } else {
            format!("{floor:.0}")
        };
        g.text(r, zx + 2, &text, theme.border_accent);
    }
    if let Some(r) = sig_row {
        let text = format!("{:.0} dBFS", c.adc_peak);
        g.set(r, zx, '\u{25cf}', theme.value_hi);
        g.text(r, zx + 2, &text, theme.value_hi);
        g.bold(r, zx + 2, text.len());
    }

    // The chain under it: each stage boxed at its riser, its gain beneath.
    let mut axis = Grid::new(iw, 2, theme.value);
    axis.text(0, 0, "    \u{2514}", dim);
    for x in GUTTER..zx {
        axis.set(0, x, '\u{2500}', dim);
    }
    // Digital stages after the converter: the walk starts at the converter
    // and ends at the word the samples are, and the antenna is not on it.
    let (start, end) = if state.caps.gain.stages_are_digital() {
        ("ADC", "\u{25b6} word")
    } else {
        ("ANT", "\u{25b6} ADC")
    };
    axis.text(0, zx, end, theme.label);
    axis.text(1, GUTTER + 1, start, theme.label);
    // Left to right; a box that would run into the one before it is left
    // out rather than overprinted.
    let (mut free_box, mut free_under) = (GUTTER, GUTTER + 5);
    for ((name, under), &r) in c.stages.iter().zip(&steps.risers) {
        let x = GUTTER + r;
        let n = name.chars().count();
        let bx = x.saturating_sub(n.div_ceil(2));
        if bx >= free_box && bx + n + 2 <= zx {
            axis.text(0, bx, &format!("\u{2524}{name}\u{251c}"), dim);
            axis.text(0, bx + 1, name, theme.value);
            free_box = bx + n + 2;
        }
        let u = under.chars().count();
        let ux = x.saturating_sub(u / 2);
        if ux >= free_under && ux + u <= zx {
            axis.text(1, ux, under, theme.value_hi);
            free_under = ux + u + 1;
        }
    }
    let total = c.signal[s] - c.signal[0];
    if total.is_finite() {
        axis.text(1, zx, &format!("\u{03a3} {total:+.0} dB"), theme.label);
    }

    let caption = if c.noise.is_some() {
        " signal measured at the converter; noise from each stage's figure"
    } else if state.caps.gain.stages_are_digital() {
        " measured in the samples, walked back through the digital gain"
    } else {
        " signal measured at the converter, walked back stage by stage"
    };
    let mut lines = vec![Line::from(Span::styled(caption, Style::default().fg(dim)))];
    lines.extend((0..ch).map(|r| g.line(r)));
    lines.push(axis.line(0));
    lines.push(axis.line(1));

    let floor_key = match c.floor {
        Some((_, enbw)) => format!(
            "\u{2500} floor /{}",
            crate::ui::widgets::micro_common::fmt_rbw(enbw)
        ),
        None => "no floor measured yet".to_string(),
    };
    let mut legend = vec![
        Span::raw(" "),
        Span::styled("\u{2501} signal, dBFS", Style::default().fg(theme.value_hi)),
        Span::styled(
            format!("   {floor_key}"),
            Style::default().fg(theme.border_accent),
        ),
    ];
    if c.noise.is_some() {
        legend.push(Span::styled(
            "   \u{2504} noise, modelled",
            Style::default().fg(theme.border_accent),
        ));
    }
    legend.push(Span::styled(
        match state.ceiling() {
            Some(c) if c.kind == crate::hardware::CeilingKind::Holds => {
                format!("   \u{2508} hold, {}", c.part)
            }
            _ => "   \u{2508} clip".to_string(),
        },
        Style::default().fg(theme.status_crit),
    ));
    lines.push(Line::from(legend));
    if !state.caps.friis_applicable {
        lines.push(Line::from(Span::styled(
            format!(" no noise trace: {}", state.caps.gain.no_cascade_reason()),
            Style::default().fg(dim),
        )));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_name() {
        assert_eq!(LevelDiagramPanel.name(), "level_diagram");
    }

    fn draw(m: &SdrMetrics, w: u16, h: u16) -> String {
        crate::state::fixture::draw(LevelDiagramPanel, w, h, m).join("\n")
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

    fn hackrf() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        m.signal.adc_peak_dbfs = -14.0;
        if let Some(fr) = m.waterfall.last_fft.as_mut() {
            fr.noise_floor = -72.0;
        }
        m
    }

    /// **On a chain with no published noise figures the diagram is the
    /// measured half**: the signal walked back from the converter through
    /// each stage's measured gain, the chain boxed under it with each
    /// stage's gain, and the converter's window with where this radio
    /// clips, the signal, the floor and the distances between them; it says
    /// why there is no noise trace.
    #[test]
    fn an_r2_gets_the_measured_diagram() {
        // At the Lab RF bench's own width on a laptop.
        let out = draw(&r2(92_800_000), 73, 26);
        for want in [
            "\u{2524}LNA\u{251c}",
            "\u{2524}MIX\u{251c}",
            "\u{2524}VGA\u{251c}",
            "+17",
            "+1",
            "+34",
            "\u{25b6} ADC",
            "ANT",
            "\u{03a3} +51 dB",
            "\u{2501}",
            "\u{25cf} -27 dBFS",
            "tuner clips -3",
            "24 dB headroom",
            "73 dB over floor",
            "floor -100",
            "\u{2500} floor /39.1 kHz",
            "no noise trace: each stage's own noise figure is not published",
        ] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
        assert!(!out.contains('\u{2504}'), "no modelled noise:\n{out}");
    }

    /// Outside the LNA's band its gain reads as its step and the levels
    /// before it are unknown, so the trace starts after it and there is no
    /// total; under the AGC the stages it sets read AGC.
    #[test]
    fn the_measured_diagram_stops_where_a_gain_is_not_known() {
        let far = draw(&r2(943_400_000), 90, 22);
        assert!(far.contains("6/14"), "{far}");
        assert!(!far.contains("NaN") && !far.contains('\u{03a3}'), "{far}");
        let mut agc = r2(92_800_000);
        agc.radio.amp_enabled = true;
        let out = draw(&agc, 90, 22);
        assert_eq!(out.matches("AGC").count(), 2, "{out}");
    }

    /// The HF+'s ceiling is its digital AGC's hold, and the diagram says it
    /// holds there, wide or narrow.
    #[test]
    fn the_hf_plus_window_shows_the_digital_agcs_hold() {
        let mut m = SdrMetrics::fixture()
            .streaming()
            .with_carrier(0.0, 30.0)
            .airspyhf();
        m.signal.adc_peak_dbfs = -30.0;
        let wide = draw(&m, 73, 26);
        assert!(wide.contains("\u{252c} hold -14"), "{wide}");
        assert!(wide.contains("\u{2508} hold, digital AGC"), "{wide}");
        // A gap of three figures is given as its number, not cut mid-word.
        if let Some(fr) = m.waterfall.last_fft.as_mut() {
            fr.noise_floor = -133.0;
        }
        let deep = draw(&m, 73, 26);
        assert!(
            deep.contains("103 dB") && !deep.contains("over floo"),
            "{deep}"
        );
        assert!(!wide.contains("clip"), "{wide}");
    }

    /// **On the HF+ the stages are digital, after the converter**: the walk
    /// starts at the converter and ends at the word, the antenna is not on
    /// the axis, and the caption says the signal was measured in the samples.
    #[test]
    fn the_hf_plus_walk_starts_at_the_converter() {
        let mut m = SdrMetrics::fixture()
            .streaming()
            .with_carrier(0.0, 30.0)
            .airspyhf();
        m.signal.adc_peak_dbfs = -30.0;
        let out = draw(&m, 73, 26);
        for want in [
            "measured in the samples, walked back through the digital gain",
            "\u{25b6} word",
            "ADC",
        ] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
        assert!(
            !out.contains("ANT") && !out.contains("\u{25b6} ADC"),
            "{out}"
        );
    }

    /// A radio with no ceiling of its own clips at the converter, and says
    /// that instead.
    #[test]
    fn a_chain_with_no_ceiling_clips_at_the_converter() {
        let m = SdrMetrics::fixture()
            .streaming()
            .named_chain()
            .with_carrier(0.0, 20.0);
        let out = draw(&m, 90, 22);
        assert!(out.contains("ADC clips 0"), "{out}");
        assert!(!out.contains("tuner"), "{out}");
    }

    /// **On the HackRF the noise is walked back from the floor the FFT
    /// measured**, dashed because it is modelled from the stages' noise
    /// figures. It used to be anchored on the sample peak less a spectral
    /// ratio, which put the converter's noise 18 dB off the floor here.
    #[test]
    fn a_hackrf_gets_the_modelled_noise_on_the_measured_floor() {
        let out = draw(&hackrf(), 73, 26);
        for want in [
            "noise from each stage's figure",
            "\u{2504} noise, modelled",
            "\u{2504}",
            "\u{2524}LNA\u{251c}",
            "\u{2524}MIX\u{251c}",
            "\u{2524}VGA\u{251c}",
            "+24",
            "-7",
            "+30",
            "\u{03a3} +47 dB",
            "ADC clips 0",
            "\u{25cf} -14 dBFS",
            "14 dB headroom",
            "58 dB over floor",
            "floor -72",
        ] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
        assert!(!out.contains("no noise trace"), "{out}");
        assert!(!out.contains("-54"), "the old anchor:\n{out}");
    }

    /// No floor measured yet: nothing to anchor the noise on, so no noise
    /// trace and no distance to a floor, and the legend says why.
    #[test]
    fn with_no_floor_there_is_no_noise_walk() {
        let mut m = hackrf();
        m.waterfall.last_fft = None;
        let out = draw(&m, 73, 26);
        assert!(out.contains("no floor measured yet"), "{out}");
        assert!(
            !out.contains('\u{2504}') && !out.contains("over floor"),
            "{out}"
        );
    }

    /// A signal under the floor is said to be under it, not drawn as a
    /// negative distance over it.
    #[test]
    fn a_signal_under_the_floor_says_so() {
        let mut m = hackrf();
        m.signal.adc_peak_dbfs = -80.0;
        let out = draw(&m, 73, 26);
        assert!(out.contains("8 dB under floor"), "{out}");
    }

    /// **Frozen, the whole chain is held**: the stages' readings and the
    /// floor as well as the peak, so a gain moved after the freeze does not
    /// redraw half of a held picture.
    #[test]
    fn a_frozen_bench_holds_every_stage_and_the_floor() {
        let mut m = r2(92_800_000);
        m.lab.rf_freeze = Some(crate::state::RfFreeze {
            band_hist: [0; crate::hardware::ADC_BANDS],
            peak_dbfs: -27.0,
            rms_dbfs: -40.0,
            clip_events: 0,
            snr_db: 40.0,
            amp_enabled: false,
            lna_gain: 0,
            vga_gain: 0,
            floor: Some((-100.0, 39_062.5)),
            readings: (0..3).map(|i| m.stage_reading(i)).collect(),
        });
        m = m.on_curve(0, 0);
        m.signal.adc_peak_dbfs = -60.0;
        if let Some(fr) = m.waterfall.last_fft.as_mut() {
            fr.noise_floor = -90.0;
        }
        let out = draw(&m, 73, 26);
        for want in ["+17", "+34", "\u{25cf} -27 dBFS", "floor -100"] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
    }

    /// At the panel's smallest size the window keeps its numbers and drops
    /// its words, so no reading is cut.
    #[test]
    fn the_smallest_panel_keeps_the_numbers() {
        let out = draw(&r2(92_800_000), 40, 14);
        for want in [
            "clip -3",
            "\u{25cf} -27 dBFS",
            "\u{2534} -100",
            "\u{25b6} ADC",
        ] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
    }

    /// Not streaming, it draws nothing but the dash.
    #[test]
    fn stopped_it_draws_the_dash() {
        let out = draw(&SdrMetrics::fixture(), 73, 26);
        assert!(out.contains("\u{2014}\u{2014}\u{2014}"), "{out}");
        assert!(!out.contains("ADC"), "{out}");
    }

    #[test]
    fn a_staircase_steps_at_each_stage() {
        let steps = Steps::new(20, 3);
        assert_eq!(steps.risers, vec![5, 10, 15]);
        assert_eq!(steps.treads, vec![(0, 4), (6, 9), (11, 14), (16, 19)]);
        let mut g = Grid::new(20, 5, Color::Reset);
        let rows = [Some(4), Some(2), Some(3), Some(0)];
        staircase(&mut g, 0, &steps, &rows, 4, &HEAVY, Color::Reset, true);
        let text: Vec<String> = (0..5)
            .map(|r| g.cells[r].iter().map(|c| c.0).collect())
            .collect();
        assert_eq!(text[0], "               ┏━━━━");
        assert_eq!(text[1], "               ┃    ");
        assert_eq!(text[2], "     ┏━━━━┓    ┃    ");
        assert_eq!(text[3], "     ┃    ┗━━━━┛    ");
        assert_eq!(text[4], "━━━━━┛              ");
    }
}
