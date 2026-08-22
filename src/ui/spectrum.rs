use std::sync::Arc;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        canvas::{Canvas, Line as CanvasLine, Points},
        Borders, Paragraph,
    },
    Frame,
};

use crate::palette::{magnitude_to_color_themed, ColorDepth};
use crate::state::{SdrMetrics, SpectrumStyle};
use crate::ui::band_plan::BAND_PLAN;
use crate::ui::chrome;
use crate::ui::panel::{Bond, Panel};

/// Dim an `Rgb` color's brightness by `f` (0.0–1.0). Non-Rgb colors pass through.
fn dim(c: Color, f: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * f) as u8, (g as f32 * f) as u8, (b as f32 * f) as u8,
        ),
        other => other,
    }
}

/// Map a frequency to a canvas x-coordinate in `[0, n-1]`, or `None` if out of view.
fn freq_to_canvas_x(freq_hz: f64, left_hz: f64, bw: f64, n: f64) -> Option<f64> {
    if bw <= 0.0 { return None; }
    let frac = (freq_hz - left_hz) / bw;
    if (0.0..=1.0).contains(&frac) { Some(frac * (n - 1.0)) } else { None }
}

/// A canvas x — the units [`freq_to_canvas_x`] returns, spanning `0..n-1` — as a
/// terminal column inside `width`.
///
/// The two coordinate systems are easy to confuse and cost nothing to mix up
/// silently: the canvas is as wide as the spectrum has bins, typically 2048, while
/// the column is bounded by the panel, typically under 200. Using one as the other
/// clamps every position to the right-hand edge, which is exactly what the OBW label
/// did at every terminal size it was tried at.
fn canvas_x_to_col(x: f64, n: f64, width: u16) -> u16 {
    if n <= 1.0 || width == 0 { return 0; }
    let frac = (x / (n - 1.0)).clamp(0.0, 1.0);
    ((frac * width as f64).round() as u16).min(width - 1)
}

/// How far a bin must rise above the noise floor to count as a real signal peak.
/// Well above typical FFT noise ripple, so only solid carriers qualify — which is
/// what keeps the auto-flagged set stable frame-to-frame (no flicker on noise).
const PEAK_PROMINENCE_DB: f32 = 10.0;

/// Detect the strongest spectral peaks for auto-marking. Returns the bin indices
/// of local maxima that rise at least `PEAK_PROMINENCE_DB` above `noise_floor`,
/// each separated from already-chosen peaks by `min_sep` bins, strongest first,
/// capped at `max_peaks`. Pure + deterministic so it can be unit-tested.
pub(crate) fn detect_peaks(bins: &[f32], noise_floor: f32, max_peaks: usize, min_sep: usize) -> Vec<usize> {
    if bins.len() < 3 || max_peaks == 0 { return Vec::new(); }
    let thresh = noise_floor + PEAK_PROMINENCE_DB;

    // Local maxima above the threshold. A plateau registers only on its rising
    // edge (`v > left`, `v >= right`), so flat tops don't yield duplicates.
    let mut cands: Vec<usize> = (1..bins.len() - 1)
        .filter(|&i| bins[i] >= thresh && bins[i] > bins[i - 1] && bins[i] >= bins[i + 1])
        .collect();
    cands.sort_by(|&a, &b| bins[b].partial_cmp(&bins[a]).unwrap_or(std::cmp::Ordering::Equal));

    let mut chosen: Vec<usize> = Vec::new();
    for c in cands {
        if chosen.iter().all(|&p| (p as isize - c as isize).unsigned_abs() >= min_sep) {
            chosen.push(c);
            if chosen.len() >= max_peaks { break; }
        }
    }
    chosen
}

// ── Spectrum step sizes ───────────────────────────────────────────────────────

pub const SPECTRUM_STEPS: &[u64] = &[
    1_000, 5_000, 10_000, 25_000, 100_000, 500_000, 1_000_000, 5_000_000, 10_000_000,
];

pub fn prev_spectrum_step(current: u64) -> u64 {
    match SPECTRUM_STEPS.iter().position(|&s| s == current) {
        Some(idx) => SPECTRUM_STEPS[idx.saturating_sub(1)],
        // Not in list: find the largest step strictly below current
        None => SPECTRUM_STEPS.iter().copied()
            .filter(|&s| s < current)
            .last()
            .unwrap_or(SPECTRUM_STEPS[0]),
    }
}

pub fn next_spectrum_step(current: u64) -> u64 {
    match SPECTRUM_STEPS.iter().position(|&s| s == current) {
        Some(idx) => SPECTRUM_STEPS[(idx + 1).min(SPECTRUM_STEPS.len() - 1)],
        // Not in list: find the smallest step strictly above current
        None => SPECTRUM_STEPS.iter().copied()
            .find(|&s| s > current)
            .unwrap_or(*SPECTRUM_STEPS.last().unwrap()),
    }
}

fn fmt_khz(hz: u64) -> String {
    if hz >= 1_000_000 { format!("{:.1}M", hz as f64 / 1_000_000.0) }
    else               { format!("{}k", hz / 1_000) }
}

pub fn fmt_spectrum_step(hz: u64) -> String {
    if hz >= 1_000_000 { format!("{} MHz", hz / 1_000_000) }
    else { format!("{} kHz", hz / 1_000) }
}

/// Build the frequency-scale spans for an axis/ruler `width` columns wide: a `┬`
/// tick + MHz label at each quarter, the inter-tick gaps filled with `fill`.
/// Reused by the spectrum's own axis (fill `' '`) and the bonded shared ruler on
/// the waterfall's top border (fill `'─'`, so it reads as a continuous rule).
pub fn freq_scale_spans(left_hz: f64, bw: f64, width: usize,
                        tick_color: Color, label_color: Color, fill: char) -> Vec<Span<'static>> {
    let labels: Vec<String> = (0..=4)
        .map(|i| format!("{:.2}M", (left_hz + bw * i as f64 / 4.0) / 1_000_000.0))
        .collect();
    let lw  = labels.iter().map(|s| s.len()).max().unwrap_or(7);
    let seg = width.saturating_sub(lw) / 4;
    let mut spans: Vec<Span> = Vec::with_capacity(12);
    for (i, lab) in labels.iter().enumerate() {
        spans.push(Span::styled("\u{252C}", Style::default().fg(tick_color))); // ┬
        if i < 4 {
            let pad = seg.saturating_sub(1).saturating_sub(lab.len());
            spans.push(Span::styled(lab.clone(), Style::default().fg(label_color)));
            spans.push(Span::styled(fill.to_string().repeat(pad), Style::default().fg(tick_color)));
        } else {
            spans.push(Span::styled(lab.clone(), Style::default().fg(label_color)));
        }
    }
    spans
}

/// The occupied-bandwidth window as a symmetric span around `center_hz` — the
/// app's own display convention ("the channel is centred at the tuned LO",
/// matching the ACPR offset math and the lab_iq carrier model), not necessarily
/// the SM.328 cumulative method's true (possibly asymmetric) cutoff bins, which
/// aren't retained past the FFT worker. A clearly-labelled approximation, good
/// for the primary lab_signal use case of a single centred carrier.
fn obw_bounds(center_hz: u64, obw_hz: u64) -> (f64, f64) {
    let half = obw_hz as f64 / 2.0;
    (center_hz as f64 - half, center_hz as f64 + half)
}

/// Signed delta between two markers: `(Δf_hz, Δlevel_db)`, second minus first —
/// matches the mockup's "Δ +180.0 kHz  -38.1 dB" reading (MKR2 relative to MKR1).
fn marker_delta(freq_a: u64, level_a: f32, freq_b: u64, level_b: f32) -> (i64, f32) {
    (freq_b as i64 - freq_a as i64, level_b - level_a)
}

/// Border set for the spectrum given its bond. `Bond::Below` drops the bottom
/// border (the waterfall's top border below becomes the shared ruler); otherwise
/// the panel is fully framed.
fn bond_borders(bond: Bond) -> Borders {
    if bond == Bond::Below {
        Borders::TOP | Borders::LEFT | Borders::RIGHT
    } else {
        Borders::ALL
    }
}

pub struct SpectrumPanel;

impl Panel for SpectrumPanel {
    fn name(&self) -> &'static str { "spectrum" }
    fn min_size(&self) -> (u16, u16) { (40, 10) }
    fn focus_key(&self) -> Option<char> { Some('e') }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("← →", "Tune frequency"),
            ("[ ]", "Step size"),
            ("↑ ↓", "Zoom y-axis"),
            ("J K",  "Cursor"),
            ("M",    "Place/remove marker"),
            ("B",    "Cycle channel BW on nearest marker"),
            ("H",    "Hold/unhold frame"),
            ("D",    "Draw style"),
        ]
    }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        render(f, area, state, theme, focused, Bond::None);
    }
}

/// Free render entry point so the layout engine can bond the spectrum to the
/// waterfall below it. `Bond::Below` drops the bottom border and the panel's own
/// frequency-axis row — the waterfall's top border becomes the shared ruler.
pub fn render(f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool, bond: Bond) {
        let stale = state.waterfall.last_fft.as_ref()
            .map(|fr| fr.timestamp.elapsed() > std::time::Duration::from_millis(500))
            .unwrap_or(false);
        let no_data = state.waterfall.last_fft.is_none();

        let border_color = if focused          { theme.border_focused }
            else if stale || no_data           { theme.stale }
            else                               { theme.border_accent };

        let borders = bond_borders(bond);

        // Nameplate: SPECTRUM with the 'E' focus key highlighted, plus live tags.
        let key_style  = Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD);
        let name_style = Style::default().fg(theme.label).add_modifier(Modifier::BOLD);
        let mut title_spans = chrome::nameplate(vec![
            Span::styled("SP", name_style),
            Span::styled("E", key_style),
            Span::styled("CTRUM", name_style),
        ], border_color);
        if state.spectrum.hold.is_some() {
            title_spans.push(Span::styled(" [HOLD]", Style::default().fg(theme.status_warn)));
        }
        if stale {
            title_spans.push(Span::styled(" [STALE]", Style::default().fg(theme.stale)));
        }
        let title_line = Line::from(title_spans);

        match state.waterfall.last_fft.as_ref() {
            None => {
                f.render_widget(
                    Paragraph::new("Waiting for RX\u{2026}")
                        .block(chrome::deck_block_borders(border_color, borders).title(title_line))
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(theme.label)),
                    area,
                );
                if bond == Bond::Below { chrome::corner_accents_top(f, area, border_color); }
                else { chrome::corner_accents(f, area, border_color); }
            }
            Some(frame) => {
                let outer_block = chrome::deck_block_borders(border_color, borders).title(title_line);
                let inner = outer_block.inner(area);
                f.render_widget(outer_block, area);
                if bond == Bond::Below { chrome::corner_accents_top(f, area, border_color); }
                else { chrome::corner_accents(f, area, border_color); }

                // Layout: dBFS label column (6) | canvas+freq[+indicator]
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(6), Constraint::Min(1)])
                    .split(inner);

                // Bonded below: no own frequency-axis row (the shared ruler covers
                // it), reclaiming that row for the plot.
                let show_freq = bond != Bond::Below;
                let v_constraints: Vec<Constraint> = match (focused, show_freq) {
                    (true,  true)  => vec![Constraint::Min(4), Constraint::Length(1), Constraint::Length(1)],
                    (true,  false) => vec![Constraint::Min(4), Constraint::Length(1)],
                    (false, true)  => vec![Constraint::Min(4), Constraint::Length(1)],
                    (false, false) => vec![Constraint::Min(4)],
                };
                let rows    = Layout::default().direction(Direction::Vertical)
                    .constraints(v_constraints.clone()).split(cols[1]);
                let db_rows = Layout::default().direction(Direction::Vertical)
                    .constraints(v_constraints).split(cols[0]);

                let canvas_area = rows[0];
                let (freq_area, indicator_area): (Option<Rect>, Option<Rect>) = match (focused, show_freq) {
                    (true,  true)  => (Some(rows[1]), Some(rows[2])),
                    (true,  false) => (None,          Some(rows[1])),
                    (false, true)  => (Some(rows[1]), None),
                    (false, false) => (None,          None),
                };

                let full_n = frame.bins_dbfs.len();
                if full_n == 0 { return; }
                let full_bw = frame.sample_rate;
                if full_bw <= 0.0 { return; }
                let full_left = frame.center_freq_hz as f64 - full_bw / 2.0;

                // Shared frequency zoom: when bonded below the waterfall, both plots
                // narrow to the same centre slice of bins (factor `hz_zoom`), so the
                // instrument zooms as one around the tuned frequency. Standalone the
                // spectrum shows the full span (zoom 1).
                let zoom = if bond == Bond::Below { (state.waterfall.hz_zoom as usize).max(1) } else { 1 };
                let (bins, peaks, held_bins, n_bins, left_hz, bw) = if zoom > 1 {
                    let visible_n = (full_n / zoom).max(1);
                    let lo = (full_n / 2).saturating_sub(visible_n / 2).min(full_n - visible_n);
                    let hi = lo + visible_n;
                    let bin_hz = full_bw / full_n as f64;
                    // Defensive clamp: any series whose length differs from the live
                    // bins is windowed against its own length so slicing can't panic.
                    let win = |v: &[f32]| {
                        let l = lo.min(v.len());
                        let r = hi.min(v.len());
                        Arc::new(v[l..r].to_vec())
                    };
                    let held = state.spectrum.hold.as_ref().map(|h| win(h));
                    (win(&frame.bins_dbfs), win(&frame.peak_hold), held,
                     visible_n, full_left + lo as f64 * bin_hz, visible_n as f64 * bin_hz)
                } else {
                    // Arc::clone is O(1) — no data copied.
                    (Arc::clone(&frame.bins_dbfs), Arc::clone(&frame.peak_hold),
                     state.spectrum.hold.clone(), full_n, full_left, full_bw)
                };
                let right_hz = left_hz + bw;
                let noise_floor = frame.noise_floor;
                // Cheap Arc clones of the *displayed* bins kept for the post-canvas
                // auto-peak flag section — the canvas paint closure moves bins/peaks.
                let flag_bins = Arc::clone(&bins);
                let flag_peaks = Arc::clone(&peaks);

                // Dynamic y-range from state (user-controlled zoom)
                let y_min_f = state.spectrum.y_min;
                let y_max_f = state.spectrum.y_max;

                // Lab "instrument mode" overlays: a user-set reference level line and
                // a captured reference (CAL) trace ghost — only in the measurement labs.
                let lab_mode = state.ui.is_lab_mode();
                let lab_ref: Option<f64> = if lab_mode {
                    state.lab.ref_dbfs.map(|r| r.clamp(y_min_f, y_max_f) as f64)
                } else { None };
                let lab_trace: Option<Arc<Vec<f32>>> = if lab_mode { state.lab.ref_trace.clone() } else { None };
                let ref_line_color = theme.value_hi;
                let cal_color = theme.observer;
                let style = state.spectrum.style;

                // Cursor power
                let cursor_power: Option<f32> = state.spectrum.cursor_freq.and_then(|cf| {
                    let frac = (cf as f64 - left_hz) / bw;
                    if (0.0..=1.0).contains(&frac) {
                        let idx = (frac * (n_bins - 1) as f64).round() as usize;
                        Some(bins[idx.min(n_bins - 1)])
                    } else { None }
                });
                let cursor_freq_mhz = state.spectrum.cursor_freq
                    .map(|f| f as f64 / 1_000_000.0);

                // ── Braille dot-matrix spectrum (ratatui Canvas) ──────────
                // The Canvas widget rasterises every shape onto a 2×4 braille dot
                // grid per cell, giving the classic phosphor dot-matrix instrument
                // look at a stable, terminal-size-independent density.
                let n      = n_bins as f64;
                let y_min  = y_min_f as f64;
                let y_max  = y_max_f as f64;
                let span   = (y_max_f - y_min_f).max(1e-3);
                let depth  = ColorDepth::detect();

                // Height-based color gradient, pre-computed once (the closure can't
                // borrow Theme). Color depends ONLY on vertical position, never on the
                // fluctuating per-bin dB — so the spectrum can never flicker in color
                // frame to frame. One band per braille dot-row for a gap-free fill.
                //
                //   band_dim    → the soft glowing body (dimmed gradient)
                //   band_bright → the crisp full-brightness top edge
                let v_steps = (canvas_area.height as usize * 4).clamp(1, 512);
                let height_color = |s: usize| -> Color {
                    let frac = if v_steps > 1 { s as f32 / (v_steps - 1) as f32 } else { 0.0 };
                    magnitude_to_color_themed(y_min_f + frac * span, y_min_f, y_max_f, depth, theme)
                };
                let band_y:      Vec<f32>   = (0..v_steps)
                    .map(|s| {
                        let frac = if v_steps > 1 { s as f32 / (v_steps - 1) as f32 } else { 0.0 };
                        y_min_f + frac * span
                    })
                    .collect();
                let band_dim:    Vec<Color> = (0..v_steps).map(|s| dim(height_color(s), 0.45)).collect();
                let band_bright: Vec<Color> = (0..v_steps).map(height_color).collect();

                let peak_hold_color   = theme.peak_hold;
                let noise_floor_color = theme.noise_floor;
                // Frozen hold ghost: a soft, dimmed phosphor so it reads as a "past"
                // snapshot of the whole spectrum without competing with the live trace.
                let hold_color  = dim(theme.border_focused, 0.50);
                let cursor_color = theme.value_hi;
                let marker_color    = theme.status_warn;
                let bw_border_color = theme.border_accent;
                // Graticule: a faint reference grid (SA screen) drawn behind the
                // trace, at the same dB ticks and frequency quarters as the axes.
                let grid_color = theme.stale;

                // Signal-characterization overlays (OBW band, Δ readout, noise-floor
                // / legend labels) are specific to the lab_signal instrument — the
                // only lab preset that bonds spectrum+waterfall in its Body column.
                // Every other preset that bonds them (main, command_rail) is untouched.
                let signal_overlay = state.ui.active_preset == "lab_signal";
                let obw_color = dim(theme.border_accent, 0.55);
                // OBW band bounds (canvas x): a symmetric window around the tuned
                // centre. `None` on an edge means it's out of the current zoomed view.
                let (obw_lo_x, obw_hi_x) = if signal_overlay && frame.occupied_bw_hz > 0 {
                    let (lo_hz, hi_hz) = obw_bounds(frame.center_freq_hz, frame.occupied_bw_hz);
                    (freq_to_canvas_x(lo_hz, left_hz, bw, n), freq_to_canvas_x(hi_hz, left_hz, bw, n))
                } else {
                    (None, None)
                };

                // Cursor canvas x-coordinate (0..n-1).
                let cursor_x_canvas = state.spectrum.cursor_freq.and_then(|cf| {
                    freq_to_canvas_x(cf as f64, left_hz, bw, n)
                });

                // Marker x-coordinates + optional channel-BW boundary pairs.
                struct MarkerCanvas { x: Option<f64>, bw_lo: Option<f64>, bw_hi: Option<f64> }
                let marker_data: Vec<MarkerCanvas> = state.spectrum.markers.iter()
                    .filter_map(|mk| {
                        let x = freq_to_canvas_x(mk.freq_hz as f64, left_hz, bw, n);
                        let (bw_lo, bw_hi) = if let Some(ch_bw) = mk.channel_bw_hz {
                            let half = ch_bw as f64 / 2.0;
                            (freq_to_canvas_x(mk.freq_hz as f64 - half, left_hz, bw, n),
                             freq_to_canvas_x(mk.freq_hz as f64 + half, left_hz, bw, n))
                        } else { (None, None) };
                        if x.is_some() || bw_lo.is_some() || bw_hi.is_some() {
                            Some(MarkerCanvas { x, bw_lo, bw_hi })
                        } else { None }
                    })
                    .collect();

                f.render_widget(
                    Canvas::default()
                        .x_bounds([0.0, (n - 1.0).max(0.0)])
                        .y_bounds([y_min, y_max])
                        .paint(move |ctx| {
                            // 0. Graticule — faint dB + frequency reference grid,
                            //    drawn first so the trace and fill sit on top of it.
                            //    Only the parts above the signal show through, exactly
                            //    like a spectrum-analyser screen.
                            for i in 0..=4 {
                                let yv = y_min + (y_max - y_min) * (i as f64 / 4.0);
                                ctx.draw(&CanvasLine { x1: 0.0, y1: yv, x2: n - 1.0, y2: yv, color: grid_color });
                            }
                            for i in 0..=4 {
                                let xv = (n - 1.0).max(0.0) * (i as f64 / 4.0);
                                ctx.draw(&CanvasLine { x1: xv, y1: y_min, x2: xv, y2: y_max, color: grid_color });
                            }
                            // 1. Hold ghost — the entire frozen spectrum as a soft outline.
                            if let Some(ref held) = held_bins {
                                for i in 1..held.len() {
                                    ctx.draw(&CanvasLine {
                                        x1: (i - 1) as f64, y1: held[i - 1].clamp(y_min_f, y_max_f) as f64,
                                        x2: i as f64,       y2: held[i].clamp(y_min_f, y_max_f) as f64,
                                        color: hold_color,
                                    });
                                }
                            }
                            // 2. Filled body — solid horizontal runs per band (every
                            //    column that reaches this height), continuous so no
                            //    isolated dots blink on/off as bins jitter. Skipped for
                            //    Scatter; Braille dims it (a soft glow under the crisp
                            //    edge), Fill keeps it full-brightness (a heavy body).
                            if style != SpectrumStyle::Scatter {
                                for s in 0..v_steps {
                                    let yb  = band_y[s];
                                    let ybf = yb as f64;
                                    let color = if style == SpectrumStyle::Fill { band_bright[s] } else { band_dim[s] };
                                    let mut i = 0usize;
                                    while i < bins.len() {
                                        if bins[i] >= yb {
                                            let start = i;
                                            while i < bins.len() && bins[i] >= yb { i += 1; }
                                            ctx.draw(&CanvasLine {
                                                x1: start as f64, y1: ybf,
                                                x2: (i - 1) as f64, y2: ybf,
                                                color,
                                            });
                                        } else {
                                            i += 1;
                                        }
                                    }
                                }
                            }
                            // 3. Live edge — crisp full-brightness trace connecting the bin
                            //    tops, coloured by HEIGHT (stable, never flickers). Only
                            //    Braille draws it (over its dim body); Fill's bright body
                            //    is its own edge, Scatter has no line.
                            if style == SpectrumStyle::Braille {
                                for i in 1..bins.len() {
                                    let y0 = bins[i - 1].clamp(y_min_f, y_max_f);
                                    let y1 = bins[i].clamp(y_min_f, y_max_f);
                                    let frac = (((y0 + y1) * 0.5 - y_min_f) / span).clamp(0.0, 1.0);
                                    let idx  = ((frac * (v_steps - 1) as f32) as usize).min(v_steps - 1);
                                    ctx.draw(&CanvasLine {
                                        x1: (i - 1) as f64, y1: y0 as f64,
                                        x2: i as f64,       y2: y1 as f64,
                                        color: band_bright[idx],
                                    });
                                }
                            }
                            // 3b. Scatter — an airy dot per bin at its top, no fill or line.
                            if style == SpectrumStyle::Scatter {
                                for i in 0..bins.len() {
                                    let yv   = bins[i].clamp(y_min_f, y_max_f);
                                    let frac = ((yv - y_min_f) / span).clamp(0.0, 1.0);
                                    let idx  = ((frac * (v_steps - 1) as f32) as usize).min(v_steps - 1);
                                    let pt = [(i as f64, yv as f64)];
                                    ctx.draw(&Points { coords: &pt, color: band_bright[idx] });
                                }
                            }
                            // 4. Peak hold — a single connected gold line tracing the decaying
                            //    max envelope. A line (not scattered points) stays calm and
                            //    readable instead of blinking dot by dot.
                            for i in 1..peaks.len() {
                                ctx.draw(&CanvasLine {
                                    x1: (i - 1) as f64, y1: peaks[i - 1].clamp(y_min_f, y_max_f) as f64,
                                    x2: i as f64,       y2: peaks[i].clamp(y_min_f, y_max_f) as f64,
                                    color: peak_hold_color,
                                });
                            }
                            // 5. Noise floor reference line.
                            let nf = noise_floor.clamp(y_min_f, y_max_f) as f64;
                            ctx.draw(&CanvasLine { x1: 0.0, y1: nf, x2: n - 1.0, y2: nf, color: noise_floor_color });
                            // 5b. CAL reference-trace ghost — the captured baseline,
                            //     drawn only when it matches the current bin count
                            //     (i.e. at the same zoom it was captured).
                            if let Some(ref tr) = lab_trace {
                                if tr.len() == bins.len() {
                                    for i in 1..tr.len() {
                                        ctx.draw(&CanvasLine {
                                            x1: (i - 1) as f64, y1: tr[i - 1].clamp(y_min_f, y_max_f) as f64,
                                            x2: i as f64,       y2: tr[i].clamp(y_min_f, y_max_f) as f64,
                                            color: cal_color,
                                        });
                                    }
                                }
                            }
                            // 5c. REF level — a horizontal line at the set dBFS.
                            if let Some(ry) = lab_ref {
                                ctx.draw(&CanvasLine { x1: 0.0, y1: ry, x2: n - 1.0, y2: ry, color: ref_line_color });
                            }
                            // 6. Markers + channel-BW boundaries.
                            for md in &marker_data {
                                if let Some(cx) = md.x {
                                    ctx.draw(&CanvasLine { x1: cx, y1: y_min, x2: cx, y2: y_max, color: marker_color });
                                }
                                if let Some(lo) = md.bw_lo {
                                    ctx.draw(&CanvasLine { x1: lo, y1: y_min, x2: lo, y2: y_max, color: bw_border_color });
                                }
                                if let Some(hi) = md.bw_hi {
                                    ctx.draw(&CanvasLine { x1: hi, y1: y_min, x2: hi, y2: y_max, color: bw_border_color });
                                }
                            }
                            // 6b. OBW band boundaries (lab_signal only).
                            if let Some(x) = obw_lo_x {
                                ctx.draw(&CanvasLine { x1: x, y1: y_min, x2: x, y2: y_max, color: obw_color });
                            }
                            if let Some(x) = obw_hi_x {
                                ctx.draw(&CanvasLine { x1: x, y1: y_min, x2: x, y2: y_max, color: obw_color });
                            }
                            // 7. Tuning cursor — full-height line, always visible.
                            if let Some(cx) = cursor_x_canvas {
                                ctx.draw(&CanvasLine { x1: cx, y1: y_min, x2: cx, y2: y_max, color: cursor_color });
                            }
                        }),
                    canvas_area,
                );

                // ── Band plan overlay (text on top of canvas, top row) ────
                if canvas_area.height >= 2 && canvas_area.width > 4 {
                    let cw = canvas_area.width as f64;
                    let mut next_free_col: i32 = -1;
                    for &(band_s, band_e, label) in BAND_PLAN {
                        let bs = band_s as f64;
                        let be = band_e as f64;
                        if bs >= right_hz || be <= left_hz { continue; }
                        let vis_s   = bs.max(left_hz);
                        let vis_e   = be.min(right_hz);
                        let center  = (vis_s + vis_e) / 2.0;
                        let frac    = (center - left_hz) / bw;
                        let col     = (frac * cw) as u16;
                        let lw      = label.len() as u16;
                        let col     = col.min(canvas_area.width.saturating_sub(lw));
                        if (col as i32) < next_free_col { continue; }
                        next_free_col = col as i32 + lw as i32 + 1;
                        f.render_widget(
                            Paragraph::new(Span::styled(label, Style::default().fg(theme.label))),
                            Rect { x: canvas_area.x + col, y: canvas_area.y, width: lw, height: 1 },
                        );
                    }
                }

                // ── Auto-peak flags ───────────────────────────────────────
                // Automatically flag the strongest carriers with a gold ▲ + their
                // frequency, anchored at the peak column and stacked upward from the
                // peak-hold tip (a stable y, so the flag doesn't bounce with the live
                // trace). Drawn before user markers so a deliberate ▼ marker wins any
                // overlap. Distinct glyph (▲) and colour from user markers (▼).
                if canvas_area.height >= 3 && canvas_area.width > 6 {
                    let cw = canvas_area.width;
                    let ch = canvas_area.height;
                    let min_sep = (n_bins / 24).max(1);
                    // Detect on the *displayed* (possibly zoomed) bins so each flag's
                    // column and frequency match the visible window. Using the full
                    // frame here mislocated every flag (and showed wrong MHz) when zoomed.
                    let peak_idxs = detect_peaks(&flag_bins[..], noise_floor, 5, min_sep);

                    // Per-row occupancy so flags never type over each other.
                    let mut row_occ: Vec<Vec<(u16, u16)>> = vec![Vec::new(); ch as usize];
                    for idx in peak_idxs {
                        let freq   = left_hz + bw * (idx as f64 / (n_bins - 1).max(1) as f64);
                        let frac_x = ((freq - left_hz) / bw).clamp(0.0, 1.0);
                        let col0   = (frac_x * cw as f64) as u16;
                        let amp    = flag_peaks.get(idx).copied().unwrap_or(flag_bins[idx]);
                        let frac_y = ((y_max_f - amp) / span).clamp(0.0, 1.0);
                        let tip_row = (frac_y * (ch - 1) as f32) as u16;

                        let num   = format!("{:.2}", freq / 1_000_000.0);
                        let lw    = 1 + num.chars().count() as u16; // ▲ + digits
                        let col   = col0.min(cw.saturating_sub(lw));

                        // Prefer the row just above the tip; climb until a clear slot.
                        let mut r = tip_row.saturating_sub(1) as i32;
                        let mut placed: Option<u16> = None;
                        while r >= 0 {
                            let ru = r as u16;
                            let clear = row_occ[ru as usize].iter()
                                .all(|&(s, e)| col + lw <= s || col >= e);
                            if clear { placed = Some(ru); break; }
                            r -= 1;
                        }
                        if let Some(ru) = placed {
                            row_occ[ru as usize].push((col, col + lw + 1));
                            // Soft flag: the ▲ keeps the peak-hold hue to mark the
                            // carrier, the frequency text is dimmed so it reads as a
                            // quiet annotation rather than shouting over the trace.
                            f.render_widget(
                                Paragraph::new(Line::from(vec![
                                    Span::styled("\u{25B2}", Style::default().fg(theme.peak_hold)),
                                    Span::styled(num, Style::default().fg(dim(theme.peak_hold, 0.55))),
                                ])),
                                Rect { x: canvas_area.x + col, y: canvas_area.y + ru,
                                       width: lw.min(cw.saturating_sub(col)), height: 1 },
                            );
                        }
                    }
                }

                // ── Marker labels — collision-aware multi-row placement ───
                if canvas_area.height >= 3 {
                    let cw = canvas_area.width as f64;
                    // max rows: up to 1/3 of canvas height, at least 1, at most 4
                    let max_rows = ((canvas_area.height / 3) as usize).max(1).min(4);

                    // Collect visible markers sorted left→right by frequency
                    let mut visible: Vec<(f64, &crate::state::SpectrumMarker)> = state.spectrum.markers.iter()
                        .filter_map(|mk| {
                            let frac = (mk.freq_hz as f64 - left_hz) / bw;
                            if (0.0..=1.0).contains(&frac) { Some((frac, mk)) } else { None }
                        })
                        .collect();
                    visible.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

                    // row_end[i] = first free column on row i (1-indexed in canvas rows)
                    let mut row_end: Vec<u16> = vec![0u16; max_rows];

                    for (frac, mk) in visible {
                        let bw_suffix = match (mk.channel_bw_hz, mk.measured_bw_hz) {
                            (Some(ch), Some(meas)) => {
                                let pct = meas as f64 / ch as f64 * 100.0 - 100.0;
                                format!(" {}/{} {:+.0}%", fmt_khz(ch), fmt_khz(meas), pct)
                            }
                            (Some(ch), None) => format!(" {}?", fmt_khz(ch)),
                            _ => String::new(),
                        };
                        let text = format!("▼{}{}", mk.label, bw_suffix);
                        let lw   = text.chars().count() as u16;
                        let col  = (frac * cw) as u16;
                        let col  = col.min(canvas_area.width.saturating_sub(lw));

                        // Pick the first row where this label fits without overlap
                        let row = row_end.iter().position(|&end| col >= end)
                            .unwrap_or(max_rows - 1);
                        row_end[row] = row_end[row].max(col + lw + 1);

                        f.render_widget(
                            Paragraph::new(Span::styled(
                                text,
                                Style::default().fg(theme.status_warn).add_modifier(Modifier::BOLD),
                            )),
                            Rect {
                                x: canvas_area.x + col,
                                y: canvas_area.y + 1 + row as u16,
                                width: lw,
                                height: 1,
                            },
                        );
                    }
                }

                // ── Signal-characterization overlays (lab_signal only) ────
                if signal_overlay && canvas_area.height >= 3 && canvas_area.width > 24 {
                    let ch = canvas_area.height;
                    let mut obw_label_cols: Option<(u16, u16)> = None;

                    // OBW label — centred under its two boundary lines, bottom row.
                    if let (Some(lo_x), Some(hi_x)) = (obw_lo_x, obw_hi_x) {
                        let label = format!("OBW {}", fmt_khz(frame.occupied_bw_hz));
                        let lw = label.chars().count() as u16;
                        let mid_col = canvas_x_to_col((lo_x + hi_x) / 2.0, n, canvas_area.width);
                        let col = mid_col.saturating_sub(lw / 2).min(canvas_area.width.saturating_sub(lw));
                        obw_label_cols = Some((col, col + lw));
                        f.render_widget(
                            Paragraph::new(Span::styled(label, Style::default().fg(obw_color))),
                            Rect {
                                x: canvas_area.x + col, y: canvas_area.y + ch - 1,
                                width: lw.min(canvas_area.width.saturating_sub(col)), height: 1,
                            },
                        );
                    }

                    // Noise-floor label — one row above the bottom, near the left
                    // edge, at the row the noise-floor line actually sits.
                    let nf_frac = ((y_max_f - noise_floor.clamp(y_min_f, y_max_f)) / span).clamp(0.0, 1.0);
                    let nf_row  = ((nf_frac * (ch - 1) as f32) as u16).min(ch.saturating_sub(2));
                    let nf_label = format!("noise floor {:.0} dBFS", noise_floor);
                    let nf_lw = nf_label.chars().count() as u16;
                    if nf_lw < canvas_area.width {
                        f.render_widget(
                            Paragraph::new(Span::styled(nf_label, Style::default().fg(dim(noise_floor_color, 0.8)))),
                            Rect { x: canvas_area.x + 1, y: canvas_area.y + nf_row, width: nf_lw, height: 1 },
                        );
                    }

                    // Δ readout box — top-right, MKR2 relative to MKR1, when both
                    // markers exist and both fall within the currently zoomed view.
                    if let (Some(m1), Some(m2)) = (state.spectrum.markers.first(), state.spectrum.markers.get(1)) {
                        let level_at = |freq_hz: u64| -> Option<f32> {
                            let frac = (freq_hz as f64 - left_hz) / bw;
                            if (0.0..=1.0).contains(&frac) {
                                let idx = (frac * (n_bins - 1) as f64).round() as usize;
                                flag_bins.get(idx.min(n_bins.saturating_sub(1))).copied()
                            } else { None }
                        };
                        if let (Some(l1), Some(l2)) = (level_at(m1.freq_hz), level_at(m2.freq_hz)) {
                            let (df_hz, da_db) = marker_delta(m1.freq_hz, l1, m2.freq_hz, l2);
                            let line1 = format!("\u{0394} {:+.1} kHz", df_hz as f64 / 1_000.0);
                            let line2 = format!("  {:+.1} dB", da_db);
                            let bwid = line1.chars().count().max(line2.chars().count()) as u16;
                            if bwid < canvas_area.width {
                                let col = canvas_area.width - bwid;
                                f.render_widget(
                                    Paragraph::new(Span::styled(line1, Style::default().fg(theme.value_hi))),
                                    Rect { x: canvas_area.x + col, y: canvas_area.y, width: bwid, height: 1 },
                                );
                                f.render_widget(
                                    Paragraph::new(Span::styled(line2, Style::default().fg(theme.value))),
                                    Rect { x: canvas_area.x + col, y: canvas_area.y + 1, width: bwid, height: 1 },
                                );
                            }
                        }
                    }

                    // Trace legend — bottom-right, best-effort: skipped entirely if
                    // it would collide with the OBW label sharing the same row.
                    let legend = vec![
                        Span::styled("\u{2501}", Style::default().fg(theme.border_accent)),
                        Span::styled(" live  ", Style::default().fg(theme.label)),
                        Span::styled("\u{2501}", Style::default().fg(peak_hold_color)),
                        Span::styled(" peak-hold", Style::default().fg(theme.label)),
                    ];
                    let legend_w: u16 = legend.iter().map(|s| s.content.chars().count() as u16).sum();
                    if legend_w < canvas_area.width {
                        let col = canvas_area.width - legend_w;
                        let clear = obw_label_cols.map(|(s, e)| col >= e || col + legend_w <= s).unwrap_or(true);
                        if clear {
                            f.render_widget(
                                Paragraph::new(Line::from(legend)),
                                Rect { x: canvas_area.x + col, y: canvas_area.y + ch - 1, width: legend_w, height: 1 },
                            );
                        }
                    }
                }

                // ── Frequency axis (own row; omitted when bonded below) ───
                if let Some(freq_area) = freq_area {
                    let freq_spans = freq_scale_spans(left_hz, bw, canvas_area.width as usize,
                                                      border_color, theme.value, ' ');
                    f.render_widget(Paragraph::new(Line::from(freq_spans)), freq_area);
                }

                // ── Tuning / cursor indicator (focus only) ────────────────
                if let Some(ind_area) = indicator_area {
                    let step_str  = fmt_spectrum_step(state.spectrum.step_hz);
                    let freq_str  = format!("  {:.3} MHz  ", state.radio.frequency as f64 / 1_000_000.0);

                    let right_info: String = match (cursor_freq_mhz, cursor_power) {
                        (Some(cf), Some(pwr)) => format!("  cur: {:.3} MHz  {:.1} dBFS  step {}  J/K", cf, pwr, step_str),
                        _ => format!("  step {}  [/]", step_str),
                    };

                    let center_len    = 2 + freq_str.chars().count();
                    let right_info_w  = right_info.chars().count();
                    let dashes        = (ind_area.width as usize).saturating_sub(center_len + right_info_w);
                    // Center the ◀ freq ▶ handle in the panel. The trailing
                    // right_info sits to the right of the handle, so the left arm
                    // must balance right_arm + right_info_w — not just half the
                    // dashes — or the handle drifts left of center.
                    let left_arm      = ((ind_area.width as usize).saturating_sub(center_len) / 2).min(dashes);
                    let right_arm     = dashes - left_arm;
                    let line  = Line::from(vec![
                        Span::styled("─".repeat(left_arm), Style::default().fg(theme.border_dim)),
                        Span::styled("◀", Style::default().fg(theme.border_accent).add_modifier(Modifier::BOLD)),
                        Span::styled(freq_str, Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD)),
                        Span::styled("▶", Style::default().fg(theme.border_accent).add_modifier(Modifier::BOLD)),
                        Span::styled("─".repeat(right_arm), Style::default().fg(theme.border_dim)),
                        Span::styled(right_info, Style::default().fg(theme.label)),
                    ]);
                    f.render_widget(Paragraph::new(line), ind_area);
                }

                // ── dBFS axis (ticked scale, tracks zoom) ─────────────────
                // The right edge is a vertical rule `│`; at each labelled value
                // it becomes a tick `┤`, so the dB scale reads like a ruled
                // instrument axis instead of a plain border.
                let h = db_rows[0].height as usize;
                if h > 0 {
                    let mut row_label: Vec<Option<String>> = vec![None; h];
                    for i in 0..=4 {
                        let frac = i as f32 / 4.0;
                        let db   = y_max_f - (y_max_f - y_min_f) * frac;
                        let row  = (frac * h.saturating_sub(1) as f32).round() as usize;
                        row_label[row.min(h - 1)] = Some(format!("{:>5.0}", db));
                    }
                    let lines: Vec<Line> = (0..h).map(|r| {
                        let (lbl, edge) = match &row_label[r] {
                            Some(s) => (s.clone(), "┤"),
                            None    => (" ".repeat(5), "│"),
                        };
                        Line::from(vec![
                            Span::styled(lbl,  Style::default().fg(theme.value)),
                            Span::styled(edge, Style::default().fg(border_color)),
                        ])
                    }).collect();
                    f.render_widget(Paragraph::new(lines), db_rows[0]);
                }
            }
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat noise floor at -90 dBFS with two carriers poking through.
    fn noisy_spectrum() -> Vec<f32> {
        let mut b = vec![-90.0f32; 200];
        b[50]  = -40.0; // strong
        b[150] = -55.0; // weaker, well separated
        b
    }

    #[test]
    fn detect_peaks_finds_carriers_above_noise() {
        let b = noisy_spectrum();
        let peaks = detect_peaks(&b, -90.0, 5, 4);
        assert_eq!(peaks, vec![50, 150], "strongest first, both above +10 dB prominence");
    }

    #[test]
    fn detect_peaks_ignores_sub_prominence_bumps() {
        let mut b = vec![-90.0f32; 200];
        b[50] = -40.0;   // real signal (+50 dB)
        b[120] = -82.0;  // only +8 dB → below the 10 dB threshold
        let peaks = detect_peaks(&b, -90.0, 5, 4);
        assert_eq!(peaks, vec![50]);
    }

    #[test]
    fn detect_peaks_enforces_min_separation() {
        let mut b = vec![-90.0f32; 200];
        b[50] = -30.0;   // strongest
        b[52] = -35.0;   // close second, within min_sep → dropped
        b[150] = -40.0;  // far enough → kept
        let peaks = detect_peaks(&b, -90.0, 5, 8);
        assert_eq!(peaks, vec![50, 150]);
    }

    #[test]
    fn detect_peaks_caps_at_max() {
        let mut b = vec![-90.0f32; 200];
        for i in 0..6 { b[20 + i * 25] = -40.0; }
        let peaks = detect_peaks(&b, -90.0, 3, 4);
        assert_eq!(peaks.len(), 3);
    }

    #[test]
    fn detect_peaks_flat_top_no_duplicate() {
        let mut b = vec![-90.0f32; 200];
        b[50] = -40.0; b[51] = -40.0; b[52] = -40.0; // 3-wide plateau
        let peaks = detect_peaks(&b, -90.0, 5, 4);
        assert_eq!(peaks, vec![50], "plateau yields one peak at its rising edge");
    }

    #[test]
    fn detect_peaks_empty_on_pure_noise() {
        let b = vec![-90.0f32; 200];
        assert!(detect_peaks(&b, -90.0, 5, 4).is_empty());
    }

    #[test]
    fn bond_below_drops_bottom_border() {
        // Bonded-below: no bottom border (the waterfall's top rule takes over);
        // every other edge stays.
        let b = bond_borders(Bond::Below);
        assert!(!b.contains(Borders::BOTTOM));
        assert!(b.contains(Borders::TOP | Borders::LEFT | Borders::RIGHT));
        // Standalone / above: fully framed.
        assert_eq!(bond_borders(Bond::None), Borders::ALL);
        assert_eq!(bond_borders(Bond::Above), Borders::ALL);
    }

    #[test]
    fn canvas_x_maps_across_the_column_range() {
        // 2048 bins of canvas onto 100 columns.
        assert_eq!(canvas_x_to_col(0.0, 2048.0, 100), 0);
        assert_eq!(canvas_x_to_col(2047.0, 2048.0, 100), 99);
        // The midpoint — the case that was broken: read as a column it clamped to
        // the right-hand edge on every terminal size, so the OBW label never sat
        // under the band it annotates.
        let mid = canvas_x_to_col(1023.5, 2048.0, 100);
        assert!((mid as i64 - 50).abs() <= 1, "midpoint landed at column {mid}");
    }

    #[test]
    fn canvas_x_survives_degenerate_geometry() {
        assert_eq!(canvas_x_to_col(500.0, 2048.0, 0), 0);
        assert_eq!(canvas_x_to_col(0.0, 1.0, 80), 0);
        // Out-of-range input is clamped rather than wrapping past the panel.
        assert_eq!(canvas_x_to_col(-50.0, 2048.0, 80), 0);
        assert_eq!(canvas_x_to_col(9_000.0, 2048.0, 80), 79);
    }

    #[test]
    fn obw_bounds_is_symmetric_around_centre() {
        assert_eq!(obw_bounds(100_000_000, 180_000), (99_910_000.0, 100_090_000.0));
        assert_eq!(obw_bounds(92_800_000, 0), (92_800_000.0, 92_800_000.0), "zero OBW collapses to a point at centre");
    }

    #[test]
    fn marker_delta_is_second_minus_first() {
        // 92.800 MHz / -19.1 dBFS vs 92.980 MHz / -57.2 dBFS → the mockup's
        // "Δ +180.0 kHz  -38.1 dB" reading.
        let (df_hz, da_db) = marker_delta(92_800_000, -19.1, 92_980_000, -57.2);
        assert_eq!(df_hz, 180_000);
        assert!((da_db - (-38.1)).abs() < 1e-4, "got {da_db}");
    }

    #[test]
    fn marker_delta_sign_flips_with_argument_order() {
        let (df_hz, da_db) = marker_delta(92_980_000, -57.2, 92_800_000, -19.1);
        assert_eq!(df_hz, -180_000);
        assert!((da_db - 38.1).abs() < 1e-4, "got {da_db}");
    }

    #[test]
    fn freq_scale_spans_has_five_ticks_and_quarter_labels() {
        // 100 MHz centre, 2 MHz span → labels at 99.00 … 101.00 in 0.50 steps.
        let spans = freq_scale_spans(99_000_000.0, 2_000_000.0, 60,
                                     Color::Reset, Color::Reset, '─');
        let ticks = spans.iter().filter(|s| s.content == "\u{252C}").count();
        assert_eq!(ticks, 5, "one ┬ per quarter, inclusive of both ends");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        for mhz in ["99.00M", "99.50M", "100.00M", "100.50M", "101.00M"] {
            assert!(text.contains(mhz), "ruler should label {mhz}; got {text:?}");
        }
    }
}
