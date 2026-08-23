//! `IqConstellationPanel` — 2-D braille dot-cloud of recent I/Q samples.
//!
//! Each frame shows up to [`CONSTELLATION_CAP`] normalised (I, Q) pairs from
//! the RX hot-path, decimated 1 : 1024. The cloud's position reveals the DC
//! offset; its shape reveals amplitude/phase imbalance (circular = perfect,
//! elliptical = amplitude imbalance, tilted = phase imbalance). A unit circle
//! and faint I/Q axes give a fixed reference frame.

use std::f64::consts::PI;

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        canvas::{Canvas, Line as CanvasLine, Points},
        Block, BorderType, Borders, Paragraph,
    },
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::Panel;

pub struct IqConstellationPanel;

/// Canvas coordinate half-extent — slightly wider than the unit circle so the
/// circle border and labels are not clipped.
const BOUND: f64 = 1.3;

/// Number of line segments used to approximate the unit circle.
const CIRCLE_SEGS: usize = 48;

/// Density-grid resolution (cells per axis) for the persistence colouring.
const DENSITY_GRID: usize = 28;

/// Number of heat buckets the cloud is split into, coolest → hottest.
const HEAT_LEVELS: usize = 5;

/// Cool→hot persistence palette: sparse points are a cool blue, dense cores glow
/// orange — the classic phosphor-scope look.
const HEAT: [Color; HEAT_LEVELS] = [
    Color::Rgb(35, 65, 115),   // sparse — cool blue
    Color::Rgb(30, 140, 150),  // teal
    Color::Rgb(70, 180, 90),   // green
    Color::Rgb(215, 200, 55),  // yellow
    Color::Rgb(245, 130, 35),  // hot orange (dense core)
];

/// Split the cloud into [`HEAT_LEVELS`] layers by local point density, so each can
/// be drawn in its own heat colour. Bins points on a [`DENSITY_GRID`]² grid over
/// the canvas extent; a point's bucket is `sqrt(cell_count / max_count)` (the sqrt
/// spreads the low end so sparse structure stays visible).
fn density_layers(coords: &[(f64, f64)]) -> Vec<Vec<(f64, f64)>> {
    let cell = |v: f64| -> usize {
        (((v + BOUND) / (2.0 * BOUND) * DENSITY_GRID as f64) as isize)
            .clamp(0, DENSITY_GRID as isize - 1) as usize
    };
    let mut counts = vec![0u32; DENSITY_GRID * DENSITY_GRID];
    for &(x, y) in coords {
        counts[cell(y) * DENSITY_GRID + cell(x)] += 1;
    }
    let max_c = counts.iter().copied().max().unwrap_or(1).max(1) as f64;

    let mut layers = vec![Vec::new(); HEAT_LEVELS];
    for &(x, y) in coords {
        let c = counts[cell(y) * DENSITY_GRID + cell(x)] as f64;
        let t = (c / max_c).sqrt();
        let k = ((t * HEAT_LEVELS as f64) as usize).min(HEAT_LEVELS - 1);
        layers[k].push((x, y));
    }
    layers
}

/// Fit a covariance ("RMS") ellipse to the cloud. Returns `(cx, cy, a, b, theta)`:
/// centre, semi-axes and tilt. The semi-axes use `sqrt(2·λ)` so a balanced ring is
/// traced exactly; amplitude imbalance then shows as `a≠b`, phase imbalance as a
/// non-zero tilt. `None` for too few points or a degenerate spread.
fn fit_ellipse(coords: &[(f64, f64)]) -> Option<(f64, f64, f64, f64, f64)> {
    let n = coords.len();
    if n < 16 { return None; }
    let nf = n as f64;
    let (mut sx, mut sy) = (0.0, 0.0);
    for &(x, y) in coords { sx += x; sy += y; }
    let (mx, my) = (sx / nf, sy / nf);

    let (mut cxx, mut cyy, mut cxy) = (0.0, 0.0, 0.0);
    for &(x, y) in coords {
        let (dx, dy) = (x - mx, y - my);
        cxx += dx * dx; cyy += dy * dy; cxy += dx * dy;
    }
    cxx /= nf; cyy /= nf; cxy /= nf;

    let tr = cxx + cyy;
    let det = cxx * cyy - cxy * cxy;
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let l1 = tr / 2.0 + disc;
    let l2 = (tr / 2.0 - disc).max(0.0);
    if l1 <= 1e-9 { return None; }

    let a = (2.0 * l1).sqrt();
    let b = (2.0 * l2).sqrt();
    let theta = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
    Some((mx, my, a, b, theta))
}

/// Scalar quality read-outs derived from the cloud, for the corner stats box.
/// `evm_*` / `mer_db` / `ecc` / `tilt_deg` are `None` until there are enough
/// points for a stable ellipse fit.
struct CloudStats {
    n:        usize,
    cx:       f64,
    cy:       f64,
    sigma:    f64,
    evm_rms:  Option<f64>,
    evm_pk:   Option<f64>,
    mer_db:   Option<f64>,
    ecc:      Option<f64>,
    tilt_deg: Option<f64>,
}

/// Derive the corner stats from the cloud and its fitted ellipse.
///
/// `sigma` is the radial spread (std of point radius about the centroid).
/// `evm_*` is a **scatter-derived proxy**, not symbol-referenced EVM: each point's
/// normalised radius `ρ = sqrt((u/a)² + (v/b)²)` in the fitted-ellipse frame is `1`
/// on the ellipse, so `ρ−1` measures how tightly the cloud hugs its own fitted ring
/// (amplitude/phase imbalance is captured separately by `ecc`/`tilt`, not here).
/// `mer_db = −20·log10(EVM_rms)`.
fn cloud_stats(coords: &[(f64, f64)], ellipse: Option<(f64, f64, f64, f64, f64)>) -> CloudStats {
    let n  = coords.len();
    let nf = n.max(1) as f64;
    let (mut sx, mut sy) = (0.0, 0.0);
    for &(x, y) in coords { sx += x; sy += y; }
    let (cx, cy) = (sx / nf, sy / nf);

    let (mut sr, mut sr2) = (0.0, 0.0);
    for &(x, y) in coords {
        let r = (x - cx).hypot(y - cy);
        sr += r; sr2 += r * r;
    }
    let mean_r = sr / nf;
    let sigma  = (sr2 / nf - mean_r * mean_r).max(0.0).sqrt();

    let mut s = CloudStats {
        n, cx, cy, sigma,
        evm_rms: None, evm_pk: None, mer_db: None, ecc: None, tilt_deg: None,
    };

    if let Some((ex, ey, a, b, th)) = ellipse {
        if a > 1e-9 && b > 1e-9 {
            let (ct, st) = (th.cos(), th.sin());
            let (mut acc, mut pk) = (0.0, 0.0f64);
            for &(x, y) in coords {
                let (dx, dy) = (x - ex, y - ey);
                let u =  dx * ct + dy * st;   // rotate into the ellipse's own frame
                let v = -dx * st + dy * ct;
                let rho = ((u / a).powi(2) + (v / b).powi(2)).sqrt();
                let dev = rho - 1.0;
                acc += dev * dev;
                pk = pk.max(dev.abs());
            }
            let evm_rms = (acc / nf).sqrt();
            let mer = if evm_rms > 1e-6 { -20.0 * evm_rms.log10() } else { 60.0 };
            s.evm_rms = Some(evm_rms);
            s.evm_pk  = Some(pk);
            s.mer_db  = Some(mer.min(60.0));
            // Cap the ratio: a near-collinear cloud (b→0, e.g. 90° phase imbalance)
            // would otherwise print a ~1e9 eccentricity.
            s.ecc     = Some((a / b.max(1e-9)).min(99.0));
            let mut tilt = th.to_degrees();
            while tilt >   90.0 { tilt -= 180.0; }
            while tilt <= -90.0 { tilt += 180.0; }
            s.tilt_deg = Some(tilt);
        }
    }
    s
}

impl Panel for IqConstellationPanel {
    fn name(&self) -> &'static str { "iq_constellation" }
    fn min_size(&self) -> (u16, u16) { (18, 10) }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        let stale = !state.radio.hw_streaming;
        let border_color = if focused { theme.border_focused }
            else if stale { theme.stale }
            else { theme.border_default };

        let title_line = Line::from(Span::styled(
            " IQ Constellation ",
            Style::default().fg(theme.label).add_modifier(Modifier::BOLD),
        ));
        let block = Block::default()
            .title(title_line)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if stale {
            f.render_widget(
                Paragraph::new(Span::styled("Waiting for RX\u{2026}", Style::default().fg(theme.label))),
                inner,
            );
            return;
        }

        if state.iq.constellation.is_empty() {
            f.render_widget(
                Paragraph::new(Span::styled("No samples yet\u{2026}", Style::default().fg(theme.label))),
                inner,
            );
            return;
        }

        // Pre-collect coords into an owned Vec so the closure can borrow them.
        let coords: Vec<(f64, f64)> = state.iq.constellation.iter()
            .map(|&(i, q)| (i as f64, q as f64))
            .collect();

        let dc_i = state.iq.dc_offset_i as f64;
        let dc_q = state.iq.dc_offset_q as f64;

        // Density-coloured layers (cool→hot) and the fitted imbalance ellipse,
        // computed once outside the paint closure and moved in.
        let layers = density_layers(&coords);
        let ellipse = fit_ellipse(&coords);

        let axis_color    = theme.border_dim;
        let circle_color  = theme.border_dim;
        let ref_color     = theme.border_dim;
        let ellipse_color = theme.border_focused;
        let dc_color      = theme.status_warn;
        let label_color   = theme.label;

        f.render_widget(
            Canvas::default()
                .x_bounds([-BOUND, BOUND])
                .y_bounds([-BOUND, BOUND])
                .paint(move |ctx| {
                    // I-axis (horizontal) + Q-axis (vertical)
                    ctx.draw(&CanvasLine { x1: -BOUND, y1: 0.0, x2: BOUND, y2: 0.0, color: axis_color });
                    ctx.draw(&CanvasLine { x1: 0.0, y1: -BOUND, x2: 0.0, y2: BOUND, color: axis_color });

                    // Faint ±0.5 reference ring (inner scale).
                    for k in 0..CIRCLE_SEGS {
                        let a0 = 2.0 * PI * k as f64 / CIRCLE_SEGS as f64;
                        let a1 = 2.0 * PI * (k + 1) as f64 / CIRCLE_SEGS as f64;
                        ctx.draw(&CanvasLine {
                            x1: 0.5 * a0.cos(), y1: 0.5 * a0.sin(),
                            x2: 0.5 * a1.cos(), y2: 0.5 * a1.sin(),
                            color: ref_color,
                        });
                    }
                    // Unit (1.0) reference circle.
                    for k in 0..CIRCLE_SEGS {
                        let a0 = 2.0 * PI * k as f64 / CIRCLE_SEGS as f64;
                        let a1 = 2.0 * PI * (k + 1) as f64 / CIRCLE_SEGS as f64;
                        ctx.draw(&CanvasLine {
                            x1: a0.cos(), y1: a0.sin(),
                            x2: a1.cos(), y2: a1.sin(),
                            color: circle_color,
                        });
                    }

                    // Constellation cloud — sparse (cool) layers first, dense (hot)
                    // core on top, for a phosphor-persistence look.
                    for (k, layer) in layers.iter().enumerate() {
                        ctx.draw(&Points { coords: layer, color: HEAT[k] });
                    }

                    // Fitted imbalance ellipse: axis ratio = amplitude imbalance,
                    // tilt = phase imbalance. Bright outline over the cloud.
                    if let Some((cx, cy, a, b, th)) = ellipse {
                        let (ct, st) = (th.cos(), th.sin());
                        let mut prev: Option<(f64, f64)> = None;
                        for k in 0..=CIRCLE_SEGS {
                            let t = 2.0 * PI * k as f64 / CIRCLE_SEGS as f64;
                            let (ex, ey) = (a * t.cos(), b * t.sin());
                            let x = cx + ex * ct - ey * st;
                            let y = cy + ex * st + ey * ct;
                            if let Some((px, py)) = prev {
                                ctx.draw(&CanvasLine { x1: px, y1: py, x2: x, y2: y, color: ellipse_color });
                            }
                            prev = Some((x, y));
                        }
                    }

                    // DC offset crosshair (short arms centred on the measured offset).
                    let arm = 0.07;
                    ctx.draw(&CanvasLine { x1: dc_i - arm, y1: dc_q,       x2: dc_i + arm, y2: dc_q,       color: dc_color });
                    ctx.draw(&CanvasLine { x1: dc_i,       y1: dc_q - arm, x2: dc_i,       y2: dc_q + arm, color: dc_color });

                    // Reference labels (no live numbers — just orientation).
                    let tick = |s: &str| Line::from(Span::styled(s.to_string(), Style::default().fg(label_color)));
                    ctx.print(BOUND - 0.16, 0.10, tick("I"));
                    ctx.print(0.07, BOUND - 0.08, tick("Q"));
                    ctx.print(1.02, -0.14, tick("1.0"));
                }),
            inner,
        );

        // --- corner stats overlay (drawn over the canvas) ----------------------
        // A vector-analyser read-out: EVM/MER/σ/n + fit in the top-right, the
        // density legend bottom-left, the measured centroid bottom-right. Text
        // cells overwrite the cloud underneath, so each box stays legible.
        let stats   = cloud_stats(&coords, ellipse);
        let lab     = theme.label;
        let lab_dim = theme.border_dim;
        let val     = theme.value_hi;
        let bold    = Style::default().fg(val).add_modifier(Modifier::BOLD);
        let dimst   = Style::default().fg(lab_dim);
        let labst   = Style::default().fg(lab);

        if inner.width >= 26 && inner.height >= 8 {
            let mut sl: Vec<Line> = Vec::new();
            if let (Some(r), Some(p)) = (stats.evm_rms, stats.evm_pk) {
                sl.push(Line::from(vec![
                    Span::styled("EVM ", labst),
                    Span::styled(format!("{:.1}% ", r * 100.0), bold),
                    Span::styled("rms · ", dimst),
                    Span::styled(format!("{:.0}% ", p * 100.0), Style::default().fg(val)),
                    Span::styled("pk", dimst),
                ]));
            }
            if let Some(m) = stats.mer_db {
                sl.push(Line::from(vec![
                    Span::styled("MER ", labst),
                    Span::styled(format!("{m:.1} dB"), bold),
                ]));
            }
            sl.push(Line::from(vec![
                Span::styled("σ ", labst),
                Span::styled(format!("{:.2}", stats.sigma), Style::default().fg(val)),
                Span::styled(" · n ", dimst),
                Span::styled(format!("{}", stats.n), Style::default().fg(val)),
            ]));
            if let (Some(e), Some(t)) = (stats.ecc, stats.tilt_deg) {
                sl.push(Line::from(vec![
                    Span::styled("fit ecc ", dimst),
                    Span::styled(format!("{e:.3}"), Style::default().fg(val)),
                    Span::styled(" · tilt ", dimst),
                    Span::styled(format!("{t:+.1}\u{b0}"), Style::default().fg(val)),
                ]));
            }
            // Wide enough for the longest line ("fit ecc 1.002 · tilt +0.2°" = 26);
            // a narrower box right-clips the leading "fit" off that line.
            let w = 28u16.min(inner.width);
            let h = (sl.len() as u16).min(inner.height);
            let rect = Rect { x: inner.x + inner.width - w, y: inner.y, width: w, height: h };
            f.render_widget(Paragraph::new(sl).alignment(Alignment::Right), rect);
        }

        if inner.width >= 24 && inner.height >= 6 {
            // Caption is decorative — only draw it when the whole line fits, otherwise
            // a centred truncation chops both ends into noise.
            const CAPTION: &str = "image mirrors the carrier about the LO \u{00b7} DC offset \u{2192} centre spike";
            let cap_h: u16 = if inner.height >= 7 && inner.width as usize >= CAPTION.chars().count() { 1 } else { 0 };
            let row_y = inner.y + inner.height - 2 - cap_h;

            // Centroid carries live numbers, so it gets the width it needs first; the
            // legend tiles into whatever is left and is dropped when too tight to read
            // (its widest line is "rms fit  ⊕ centroid" = 21 cells). This stops the
            // right-aligned centroid from left-clipping its own "I …" value.
            let cen = vec![
                Line::from(vec![
                    Span::styled("\u{2295} ", Style::default().fg(dc_color)),
                    Span::styled("centroid", dimst),
                ]),
                Line::from(Span::styled(
                    format!("I {:+.4} \u{b7} Q {:+.4}", stats.cx, stats.cy), labst,
                )),
            ];
            let cw = 22u16.min(inner.width);
            let cen_rect = Rect {
                x: inner.x + inner.width - cw, y: row_y,
                width: cw, height: 2,
            };
            f.render_widget(Paragraph::new(cen).alignment(Alignment::Right), cen_rect);

            let leg_w = inner.width.saturating_sub(cw);
            if leg_w >= 21 {
                let leg = vec![
                    Line::from(vec![
                        Span::styled("\u{28ff} ", Style::default().fg(HEAT[HEAT_LEVELS - 1])),
                        Span::styled("dense  ", dimst),
                        Span::styled("\u{2802} ", Style::default().fg(HEAT[0])),
                        Span::styled("sparse", dimst),
                    ]),
                    Line::from(vec![
                        Span::styled("\u{25ef} ", Style::default().fg(ellipse_color)),
                        Span::styled("rms fit  ", dimst),
                        Span::styled("\u{2295} ", Style::default().fg(dc_color)),
                        Span::styled("centroid", dimst),
                    ]),
                ];
                let leg_rect = Rect { x: inner.x, y: row_y, width: leg_w, height: 2 };
                f.render_widget(Paragraph::new(leg), leg_rect);
            }

            // Caption, full-width bottom row — echoes the scope's framing.
            if cap_h == 1 {
                let cap = Line::from(Span::styled(CAPTION, dimst));
                let rect = Rect {
                    x: inner.x, y: inner.y + inner.height - 1,
                    width: inner.width, height: 1,
                };
                f.render_widget(Paragraph::new(cap).alignment(Alignment::Center), rect);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_name_and_min_size() {
        let p = IqConstellationPanel;
        assert_eq!(p.name(), "iq_constellation");
        let (w, h) = p.min_size();
        assert!(w > 0 && h > 0);
    }

    #[test]
    fn circle_segs_constant_is_positive_even() {
        assert!(CIRCLE_SEGS > 0);
        assert_eq!(CIRCLE_SEGS % 2, 0, "even number of segments gives symmetric circle");
    }

    /// A unit circle of points: balanced → the fitted ellipse is ~circular (a≈b≈1).
    fn ring(n: usize, scale_i: f64, scale_q: f64) -> Vec<(f64, f64)> {
        (0..n).map(|k| {
            let a = 2.0 * PI * k as f64 / n as f64;
            (scale_i * a.cos(), scale_q * a.sin())
        }).collect()
    }

    #[test]
    fn fit_ellipse_balanced_ring_is_circular() {
        let (_, _, a, b, _) = fit_ellipse(&ring(256, 1.0, 1.0)).unwrap();
        assert!((a - 1.0).abs() < 0.05, "a≈1, got {a:.3}");
        assert!((b - 1.0).abs() < 0.05, "b≈1, got {b:.3}");
        assert!((a - b).abs() < 0.05, "balanced → a≈b");
    }

    #[test]
    fn fit_ellipse_amplitude_imbalance_stretches_axes() {
        // I stretched 2×, Q unchanged → major axis ~2× the minor, tilt ~0.
        let (_, _, a, b, th) = fit_ellipse(&ring(256, 2.0, 1.0)).unwrap();
        assert!(a > b, "major > minor");
        assert!((a / b - 2.0).abs() < 0.1, "axis ratio ≈ 2, got {:.2}", a / b);
        assert!(th.abs() < 0.05 || (th.abs() - PI).abs() < 0.05, "tilt ≈ 0 along I");
    }

    #[test]
    fn fit_ellipse_too_few_points_is_none() {
        assert!(fit_ellipse(&ring(8, 1.0, 1.0)).is_none());
        assert!(fit_ellipse(&[]).is_none());
    }

    #[test]
    fn density_layers_partition_all_points() {
        let coords = ring(200, 1.0, 1.0);
        let layers = density_layers(&coords);
        assert_eq!(layers.len(), HEAT_LEVELS);
        let total: usize = layers.iter().map(|l| l.len()).sum();
        assert_eq!(total, coords.len(), "every point lands in exactly one layer");
    }

    #[test]
    fn cloud_stats_balanced_ring_is_tight_and_round() {
        let coords = ring(256, 0.5, 0.5);
        let s = cloud_stats(&coords, fit_ellipse(&coords));
        assert_eq!(s.n, 256);
        // Points lie exactly on the fitted ring → near-zero EVM, high MER.
        assert!(s.evm_rms.unwrap() < 0.02, "evm {:?}", s.evm_rms);
        assert!(s.mer_db.unwrap() > 30.0, "mer {:?}", s.mer_db);
        assert!((s.ecc.unwrap() - 1.0).abs() < 0.05, "ecc {:?}", s.ecc);
        // Centroid of a centred ring is ~origin.
        assert!(s.cx.abs() < 1e-3 && s.cy.abs() < 1e-3);
    }

    #[test]
    fn cloud_stats_amplitude_imbalance_does_not_inflate_evm() {
        // A clean 2:1 elliptical ring: ecc≈2 but EVM stays low because the fit
        // captures the ellipse (imbalance is reported by ecc, not EVM).
        let coords = ring(256, 1.0, 0.5);
        let s = cloud_stats(&coords, fit_ellipse(&coords));
        assert!(s.evm_rms.unwrap() < 0.03, "evm {:?}", s.evm_rms);
        assert!((s.ecc.unwrap() - 2.0).abs() < 0.15, "ecc {:?}", s.ecc);
    }

    #[test]
    fn cloud_stats_scatter_raises_evm() {
        // Two concentric rings can't both lie on one ellipse → real radial scatter.
        let mut coords = ring(128, 0.4, 0.4);
        coords.extend(ring(128, 0.6, 0.6));
        let s = cloud_stats(&coords, fit_ellipse(&coords));
        assert!(s.evm_rms.unwrap() > 0.1, "expected scatter, evm {:?}", s.evm_rms);
        assert!(s.mer_db.unwrap() < 25.0, "mer {:?}", s.mer_db);
    }

    #[test]
    fn cloud_stats_too_few_points_has_no_evm() {
        let s = cloud_stats(&ring(8, 0.5, 0.5), None);
        assert!(s.evm_rms.is_none() && s.mer_db.is_none() && s.ecc.is_none());
        assert_eq!(s.n, 8);
    }

    #[test]
    fn density_layers_hot_core_for_concentrated_cloud() {
        // Most points piled on one spot + a few scattered → the hottest layer is used.
        let mut coords = vec![(0.1, 0.1); 500];
        coords.extend(ring(20, 1.0, 1.0));
        let layers = density_layers(&coords);
        assert!(!layers[HEAT_LEVELS - 1].is_empty(), "dense core should reach the hot layer");
    }
}
