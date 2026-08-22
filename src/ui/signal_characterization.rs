//! `signal_characterization` — the left column of the `lab_signal` preset's
//! redesign (DSN-2026-07).
//!
//! An airy read-out of what the signal at centre *is* and how clean it is, built
//! as a single Line stack fitted with `chrome::fit_spacers` and grouped by the
//! shared `chrome::section` nameplates, exactly like `iq_diagnostics`,
//! `rf_chain`, and `timing_diagnostics`:
//!
//!   1. RADIO HEADLINE   — the peak/noise figure + a status lamp.
//!   2. SIGNAL METRICS   — channel power, peak (+freq), noise floor, occupied BW,
//!      peak hold.
//!   3. ADJACENT CHANNEL — ACPR L/R, a badness-fill bar per side plus the
//!      absolute level of the louder adjacent band.
//!   4. SPECTRAL SHAPE   — C/N trend + crest.
//!   5. Verdict           — a rule-based, plain-language read of the same four
//!      zones (modulation / SNR / ACPR / OBW). No ML, no demod — pure function
//!      of what's already measured, in the spirit of `timing_diagnostics::verdict_copy`.
//!
//! Every scalar comes from the latest coherent FFT frame (`state.waterfall.last_fft`),
//! so the numbers agree with the bonded spectrum beside it.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::state::{FftFrame, Modulation, SdrMetrics};
use crate::ui::chrome::{field, section};
use crate::ui::panel::Panel;

pub struct SignalCharacterizationPanel;

/// Label-column width — clears the longest label ("Channel power" = 13) plus a gap.
const FIELD_W: usize = 14;

/// Status-lamp / headline colour by peak-to-noise: clean ≥ 20 dB, usable ≥ 10 dB,
/// else weak. Same thresholds the signal strip and micro views already use.
fn snr_color(snr: f32, theme: &crate::Theme) -> Color {
    if snr >= 20.0 { theme.status_ok } else if snr >= 10.0 { theme.status_warn } else { theme.status_crit }
}

/// `92.800 MHz` / `1.234500 GHz` — the same precise readout the lab marker bar uses.
fn fmt_freq(hz: u64) -> String {
    if hz >= 1_000_000_000 { format!("{:.6} GHz", hz as f64 / 1e9) }
    else                   { format!("{:.3} MHz", hz as f64 / 1e6) }
}

/// Occupied-bandwidth readout: MHz / kHz / Hz by magnitude.
pub(crate) fn fmt_bw(hz: u64) -> String {
    if hz >= 1_000_000      { format!("{:.3} MHz", hz as f64 / 1e6) }
    else if hz >= 1_000     { format!("{:.1} kHz", hz as f64 / 1e3) }
    else                    { format!("{hz} Hz") }
}

/// Noise floor as a power spectral density, `dBFS/Hz`, or `None` when the frame
/// carries no usable resolution bandwidth.
///
/// The one figure on this panel that is genuinely meaningless on its own. Every
/// other level here is a power — a peak, a channel, an adjacent band — and a power
/// is a power whatever the analyser's resolution. A noise floor is not: it is the
/// power that happened to land in one bin, so it rises with the bin width and says
/// as much about the sample rate as about the radio.
///
/// Measured on the same station at two rates: the floor read -81.1 dBFS at 2 Msps
/// and -73.8 dBFS at 10 Msps, a 7.3 dB difference that is entirely the 5× wider bin
/// (theory says 7.0). As a density the same two readings are -112.8 and
/// -112.5 dBFS/Hz — the same radio, correctly reported as the same radio.
///
/// Shown *beside* the per-bin figure rather than instead of it, because the per-bin
/// number is the one that matches where the noise visually sits on the trace next
/// to this panel.
fn noise_density_dbfs_hz(noise_floor_dbfs: f32, enbw_hz: f64) -> Option<f32> {
    if !enbw_hz.is_finite() || enbw_hz <= 0.0 || !noise_floor_dbfs.is_finite() { return None; }
    Some(noise_floor_dbfs - 10.0 * enbw_hz.log10() as f32)
}

/// ACPR row label — `L -200k`, `R +25k`. Derived from the offset the measurement
/// actually used, so the two can never disagree: the spacing follows the
/// modulation now, and a hardcoded label would quietly lie on every band but FM
/// broadcast.
fn acpr_label(side: char, offset_hz: f64, sign: char) -> String {
    let mag = if offset_hz >= 1_000_000.0 {
        format!("{:.1}M", offset_hz / 1e6)
    } else {
        format!("{:.0}k", offset_hz / 1e3)
    };
    format!("{side} {sign}{mag}")
}

/// The ACPR bar's own display floor — not a regulatory spectral-mask limit (we
/// don't assert one), just how far down this gauge reads before showing a fully
/// clean, empty bar. A ratio at 0 dB (touching the carrier) reads full/red.
const ACPR_BAR_FLOOR_DB: f32 = -80.0;

/// Map an ACPR ratio to a ⅛-block badness bar: more fill = closer to the
/// carrier = worse (green→red, same grading the timing deadline bars use). No
/// reference tick — unlike the timing budget bar, there is no verified
/// regulatory ACPR threshold to mark, so the bar shows the measurement only.
fn acpr_bar(db: f32, bar_w: usize, theme: &crate::Theme) -> Vec<Span<'static>> {
    let clamped = db.clamp(ACPR_BAR_FLOOR_DB, 0.0);
    let badness = ((clamped - ACPR_BAR_FLOOR_DB) * 10.0).round() as u32;
    let max_badness = ((0.0 - ACPR_BAR_FLOOR_DB) * 10.0).round() as u32;
    crate::ui::charts::gain_bar_colored(badness, max_badness, bar_w, theme.status_ok, theme.status_crit, theme.border_dim)
}

/// SNR floor below which the panel won't even hazard a modulation guess (mirrors
/// [`crate::state::CLASSIFY_MIN_SNR_DB`], the classifier's own gate) — below this
/// there's nothing to characterize.
const VERDICT_NO_SIGNAL_SNR_DB: f32 = crate::state::CLASSIFY_MIN_SNR_DB;
/// SNR at/above which the carrier reads as genuinely clean — the same "clean"
/// threshold [`snr_color`] already uses everywhere else in this panel.
const VERDICT_CLEAN_SNR_DB: f32 = 20.0;
/// ACPR worse (less negative) than this is flagged as adjacent-channel splatter
/// worth a note. sdrtop's own instrument reading, not an asserted regulatory mask
/// — same honesty stance as [`ACPR_BAR_FLOOR_DB`].
const VERDICT_ACPR_CONCERN_DB: f32 = -20.0;

/// The verdict card's severity, driving its colour and mark glyph. `NoSignal`
/// reads dim/neutral, not critical — an empty channel isn't a fault. `pub(crate)`
/// so the lab_signal marker bar's QUALITY field (`lab_chrome::signal_marker_lines`)
/// can read the exact same severity the card shows — one source of truth, same
/// precedent as `image_scope::CarrierImage`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VerdictLevel { Clean, Caution, NoSignal }

impl VerdictLevel {
    /// Short word for a tight space (the marker bar), distinct from the verdict
    /// card's fuller headline (e.g. "WEAK WFM SIGNAL" / "CLEAN WFM SIGNAL").
    pub(crate) fn short_label(self) -> &'static str {
        match self {
            VerdictLevel::Clean    => "Clean",
            VerdictLevel::Caution  => "Caution",
            VerdictLevel::NoSignal => "No signal",
        }
    }
}

/// Plain-language, rule-based verdict — purely a function of what Tier A already
/// measures (modulation, SNR, ACPR, occupied BW). No ML, no demod; mirrors
/// `timing_diagnostics::verdict_copy`'s honest-narrative approach. `pub(crate)`
/// for the same reason as [`VerdictLevel`].
pub(crate) fn verdict(modulation: Modulation, snr_db: f32, acpr_lower_db: f32, acpr_upper_db: f32, obw_hz: u64)
    -> (VerdictLevel, String, String)
{
    if !modulation.is_known() || snr_db < VERDICT_NO_SIGNAL_SNR_DB {
        return (
            VerdictLevel::NoSignal,
            "NO SIGNAL".to_string(),
            "Nothing clearly above the noise floor at centre.".to_string(),
        );
    }

    let obw_str = fmt_bw(obw_hz);
    let mod_label = modulation.label();
    // Worst (least negative — closest to the carrier) adjacent-channel ratio, when
    // the pair was measurable. Both sides, because the clause below prints both: on
    // one alone it would read "ACPR -inf/-38 dB".
    let worst_acpr = (acpr_lower_db.is_finite() && acpr_upper_db.is_finite())
        .then(|| acpr_lower_db.max(acpr_upper_db));

    if snr_db < VERDICT_CLEAN_SNR_DB {
        return (
            VerdictLevel::Caution,
            format!("WEAK {mod_label} SIGNAL"),
            format!("{mod_label} carrier detected, but only {snr_db:.0} dB above the noise floor \u{2014} marginal."),
        );
    }

    if let Some(w) = worst_acpr {
        if w > VERDICT_ACPR_CONCERN_DB {
            return (
                VerdictLevel::Caution,
                format!("{mod_label} CARRIER \u{2014} ADJACENT SPLATTER"),
                format!("Strong carrier ({snr_db:.0} dB), {obw_str} occupied, but only {w:.0} dB adjacent-channel suppression."),
            );
        }
    }

    let acpr_note = match worst_acpr {
        Some(_) => format!(", ACPR {acpr_lower_db:.0}/{acpr_upper_db:.0} dB"),
        None    => String::new(),
    };
    (
        VerdictLevel::Clean,
        format!("CLEAN {mod_label} SIGNAL"),
        format!("Strong, well-separated {mod_label} carrier \u{2014} {snr_db:.0} dB above noise, {obw_str} occupied{acpr_note}."),
    )
}

/// The strongest live bin as `(level_dbfs, freq_hz)`, mapping the bin index back to
/// frequency across the captured span. `None` for an empty frame.
///
/// Skips the DC artefact and looks only near centre, via the same helper and radius
/// the FFT worker's own peak search uses. Without the guard this row names the tuned
/// frequency and the front end's LO leakage every time the channel is quiet, which
/// is exactly when the reading matters; without the radius it can name a station the
/// radio is not tuned to, and then disagree with the headline computed above it.
fn peak_bin(fr: &FftFrame) -> Option<(f32, u64)> {
    let bins = &fr.bins_dbfs;
    let n = bins.len();
    let radius = crate::signal::fft::centre_radius_bins(n, fr.sample_rate);
    let (idx, best) = crate::signal::fft::strongest_real_bin(bins, Some(radius))?;
    let left = fr.center_freq_hz as f64 - fr.sample_rate / 2.0;
    let span_frac = if n > 1 { idx as f64 / (n - 1) as f64 } else { 0.0 };
    let freq = (left + span_frac * fr.sample_rate).max(0.0).round() as u64;
    Some((best, freq))
}

impl Panel for SignalCharacterizationPanel {
    fn name(&self) -> &'static str { "signal_characterization" }
    fn min_size(&self) -> (u16, u16) { (30, 12) }
    fn focus_key(&self) -> Option<char> { Some('x') }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("C", "Snapshot to log")]
    }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        // FFT-driven panel: stale the instant the latest frame ages past the shared
        // 500 ms threshold (or there is no frame yet), like the other signal views.
        let frame = state.waterfall.last_fft.as_ref();
        let stale = frame.map(|fr| fr.timestamp.elapsed().as_millis() > 500).unwrap_or(true);

        let name_style = Style::default().fg(theme.label).add_modifier(Modifier::BOLD);
        let mut title = vec![Span::raw(" "), Span::styled("Signal Characterization", name_style)];
        if stale { title.push(Span::styled(" [STALE]", Style::default().fg(theme.stale))); }
        title.push(Span::raw(" "));

        let border = if focused { theme.border_focused } else if stale { theme.stale } else { theme.border_default };
        let block = Block::default()
            .title(Line::from(title))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border));
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 { return; }
        let iw = inner.width as usize;

        let val = Style::default().fg(theme.value);
        let dim = Style::default().fg(theme.stale);
        let dash = || Span::styled("---".to_string(), dim);

        let mut lines: Vec<Line> = Vec::new();

        // ── RADIO HEADLINE ──────────────────────────────────────────────────
        lines.push(section("RADIO HEADLINE", "", iw, theme));
        if let Some(fr) = frame.filter(|_| !stale) {
            let snr = fr.peak_to_nf_db;
            let col = snr_color(snr, theme);
            let mut hspans = vec![
                Span::raw(" "),
                Span::styled(format!("{snr:.1}"), Style::default().fg(col).add_modifier(Modifier::BOLD)),
                Span::styled(" dB", val),
                Span::styled("  peak / noise", dim),
            ];
            // MOD badge — the classifier's estimate of what's at centre.
            if state.signal.modulation.is_known() {
                hspans.push(Span::styled(
                    format!("   {}", state.signal.modulation.label()),
                    Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD),
                ));
            }
            hspans.push(Span::styled("   \u{25cf}", Style::default().fg(col)));
            lines.push(Line::from(hspans));
        } else {
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled("\u{25cb} IDLE \u{2014} RX stopped", dim),
            ]));
        }

        lines.push(Line::raw(""));

        // ── SIGNAL METRICS ──────────────────────────────────────────────────
        lines.push(section("SIGNAL METRICS", "", iw, theme));
        let metric = |name: &str, body: Vec<Span<'static>>| -> Line<'static> {
            let mut spans = vec![field(name, FIELD_W, theme)];
            spans.extend(body);
            Line::from(spans)
        };
        if let Some(fr) = frame.filter(|_| !stale) {
            lines.push(metric("Channel power", if fr.channel_power_dbfs.is_finite() {
                vec![Span::styled(format!("{:.1} dBFS", fr.channel_power_dbfs), val)]
            } else { vec![dash()] }));

            lines.push(metric("Peak", match peak_bin(fr) {
                Some((lvl, hz)) => vec![
                    Span::styled(format!("{lvl:.1} dBFS"), val),
                    Span::styled(format!("   {}", fmt_freq(hz)), dim),
                ],
                None => vec![dash()],
            }));

            let mut nf_row = vec![Span::styled(format!("{:.1} dBFS", fr.noise_floor), val)];
            if let Some(d) = noise_density_dbfs_hz(fr.noise_floor, fr.enbw_hz) {
                nf_row.push(Span::styled(format!("   {d:.1} dBFS/Hz"), dim));
            }
            lines.push(metric("Noise floor", nf_row));

            lines.push(metric("Occupied BW", if fr.occupied_bw_hz > 0 {
                vec![
                    Span::styled(fmt_bw(fr.occupied_bw_hz), val),
                    Span::styled("   99% power", dim),
                ]
            } else { vec![dash()] }));

            // Same scope as `peak_bin`: a peak hold sitting on the DC artefact
            // since the session started, or on a station the radio is not tuned to,
            // is the least useful number on the panel.
            let ph = crate::signal::fft::strongest_real_bin(
                &fr.peak_hold,
                Some(crate::signal::fft::centre_radius_bins(fr.peak_hold.len(), fr.sample_rate)),
            ).map(|(_, v)| v);
            lines.push(metric("Peak hold", match ph {
                Some(v) if v.is_finite() => vec![Span::styled(format!("{v:.1} dBFS"), val)],
                _ => vec![dash()],
            }));
        } else {
            for name in ["Channel power", "Peak", "Noise floor", "Occupied BW", "Peak hold"] {
                lines.push(metric(name, vec![dash()]));
            }
        }

        lines.push(Line::raw(""));

        // ── ADJACENT CHANNEL (ACPR) ───────────────────────────────────────────
        lines.push(section("ADJACENT CHANNEL", "ACPR", iw, theme));
        let sig = &state.signal;
        let off = sig.acpr_offset_hz;
        let lo_label = acpr_label('L', off, '\u{2212}');
        let hi_label = acpr_label('R', off, '+');
        // The pair is written together by the FFT worker — both finite, or neither.
        // Testing both says so at the point of use rather than trusting it.
        let measured = !stale && sig.acpr_lower_db.is_finite() && sig.acpr_upper_db.is_finite();
        if measured {
            const LABEL_W: usize = 8;
            for (label, db) in [(&lo_label, sig.acpr_lower_db), (&hi_label, sig.acpr_upper_db)] {
                let value_str = format!("{db:.1} dB");
                // lead(1) + label(8) + gap(1) + bar + gap(1) + value
                let bar_w = iw.saturating_sub(1 + LABEL_W + 1 + 1 + value_str.chars().count()).max(6);
                let mut spans = vec![Span::styled(format!(" {label:<LABEL_W$}"), Style::default().fg(theme.label))];
                spans.extend(acpr_bar(db, bar_w, theme));
                spans.push(Span::styled(format!(" {value_str}"), val));
                lines.push(Line::from(spans));
            }
            // The louder of the two bands is the one worth naming, and the FFT
            // worker picks it the same way: lower wins a tie.
            let adj_freq = frame.map(|fr| {
                if sig.acpr_lower_db >= sig.acpr_upper_db {
                    fr.center_freq_hz.saturating_sub(off as u64)
                } else {
                    fr.center_freq_hz + off as u64
                }
            });
            // A silent adjacent band has no level to name — the sentinel must not
            // reach the screen as a number.
            lines.push(metric("Adj carrier", match (sig.adj_carrier_dbfs.is_finite(), adj_freq) {
                (true, Some(hz)) => vec![
                    Span::styled(format!("{:.1} dBFS", sig.adj_carrier_dbfs), val),
                    Span::styled(format!("   {}", fmt_freq(hz)), dim),
                ],
                (true, None) => vec![Span::styled(format!("{:.1} dBFS", sig.adj_carrier_dbfs), val)],
                (false, _)   => vec![dash()],
            }));
        } else {
            lines.push(metric(&lo_label, vec![dash()]));
            lines.push(metric(&hi_label, vec![dash()]));
            // Same row count either way, so the stack below does not jump when the
            // measurement comes and goes.
            lines.push(metric("Adj carrier", vec![dash()]));
        }

        lines.push(Line::raw(""));

        // ── SPECTRAL SHAPE ─────────────────────────────────────────────────
        lines.push(section("SPECTRAL SHAPE", "60 s", iw, theme));
        if !stale {
            // C/N trend: reuses `snr_history` (already fed ~500 ms by the rx poll
            // task, [`crate::state::SNR_HISTORY_LEN`] = 120 deep → 60 s), the same
            // ring the Command Rail's SNR trace and the micro views read. No new
            // state — C/N ≈ peak/noise = SNR.
            const LABEL: &str = "C/N trend";
            const TREND_ANN_W: usize = 10; // budget for "±NN.N dB"
            let snr_hist: Vec<f32> = sig.snr_history.iter().copied().collect();
            let spark_w = iw.saturating_sub(1 + LABEL.chars().count() + 1 + TREND_ANN_W).max(1);
            let (spark, p2p) = crate::ui::micro_common::spark_minmax(&snr_hist, spark_w);
            if !spark.is_empty() {
                let ann = format!("\u{00b1}{:.1} dB", p2p / 2.0);
                let used = 1 + LABEL.chars().count() + 1 + spark.chars().count() + 1 + ann.chars().count();
                let pad = iw.saturating_sub(used).max(1);
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(LABEL, Style::default().fg(theme.label)),
                    Span::raw(" "),
                    Span::styled(spark, val),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(ann, dim),
                ]));
            } else {
                lines.push(Line::from(vec![Span::raw(" "), Span::styled(LABEL, Style::default().fg(theme.label)), Span::raw("  "), dash()]));
            }

            // Crest / PAPR: reuses the exact ADC-loading model from the Lab RF
            // bench (`rf_calc::adc_loading`) rather than re-deriving peak-minus-rms
            // — full-bandwidth ADC crest factor, the same honest proxy the RF lab
            // already shows for "constant-envelope vs peaky".
            let n: u64 = state.iq.adc_signed_hist.iter().sum();
            let load = crate::ui::rf_calc::adc_loading(
                sig.adc_peak_dbfs as f64, sig.adc_rms_dbfs as f64, sig.adc_clip_events, n);
            lines.push(metric("Crest / PAPR", vec![Span::styled(format!("{:.1} dB", load.crest_db), val)]));
        } else {
            lines.push(metric("C/N trend", vec![dash()]));
            lines.push(metric("Crest / PAPR", vec![dash()]));
        }

        lines.push(Line::raw(""));

        // ── Verdict ──────────────────────────────────────────────────────────
        if let Some(fr) = frame.filter(|_| !stale) {
            let (level, headline, detail) = verdict(
                sig.modulation, fr.peak_to_nf_db, sig.acpr_lower_db, sig.acpr_upper_db, fr.occupied_bw_hz);
            let (mark, col) = match level {
                VerdictLevel::Clean    => ("\u{2713}", theme.status_ok),
                VerdictLevel::Caution  => ("\u{26a0}", theme.status_warn),
                VerdictLevel::NoSignal => ("\u{25cb}", theme.stale),
            };
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{mark} {headline}"), Style::default().fg(col).add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(vec![Span::raw(" "), Span::styled(detail, Style::default().fg(theme.label))]));
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled("[C]", Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD)),
                Span::styled(" snapshot to log", Style::default().fg(theme.label)),
            ]));
        } else {
            lines.push(Line::from(vec![Span::raw(" "), Span::styled("\u{25cb} IDLE \u{2014} RX stopped", dim)]));
        }

        crate::ui::chrome::fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    fn frame(bins: Vec<f32>, center: u64, sr: f64) -> FftFrame {
        FftFrame {
            peak_hold: Arc::new(bins.clone()),
            bins_dbfs: Arc::new(bins),
            noise_floor: -90.0,
            center_freq_hz: center,
            sample_rate: sr,
            timestamp: Instant::now(),
            peak_to_nf_db: 40.0,
            channel_power_dbfs: -22.0,
            occupied_bw_hz: 180_000,
            enbw_hz: 1_000.0,
        }
    }

    #[test]
    fn panel_name_is_stable() {
        assert_eq!(SignalCharacterizationPanel.name(), "signal_characterization");
    }

    /// 101 bins across 400 kHz centred at 100 MHz → span 99.8..100.2 MHz, 3.96 kHz
    /// a bin, centre bin at index 50. At that resolution the ±150 kHz centre radius
    /// is 38 bins, so there is room either side of it to test both edges of the rule.
    fn peaked_at(idx: usize) -> FftFrame {
        let mut bins = vec![-80.0f32; 101];
        bins[idx] = -10.0;
        frame(bins, 100_000_000, 400_000.0)
    }

    #[test]
    fn noise_density_is_the_same_radio_at_any_resolution() {
        // The two readings that started this, measured on 92.8 MHz at 2 and 10 Msps.
        // Per bin they differ by 7.3 dB and describe the sample rate; as densities
        // they agree, and describe the radio.
        let two  = noise_density_dbfs_hz(-81.1, 976.5625 * 1.5).unwrap();
        let ten  = noise_density_dbfs_hz(-73.8, 4882.8125 * 1.5).unwrap();
        assert!((two - (-112.8)).abs() < 0.2, "2 Msps: {two:.1}");
        assert!((ten - (-112.5)).abs() < 0.2, "10 Msps: {ten:.1}");
        assert!((two - ten).abs() < 0.5, "densities must agree: {two:.1} vs {ten:.1}");
    }

    #[test]
    fn noise_density_declines_without_a_resolution_bandwidth() {
        assert!(noise_density_dbfs_hz(-81.0, 0.0).is_none());
        assert!(noise_density_dbfs_hz(-81.0, f64::NAN).is_none());
        assert!(noise_density_dbfs_hz(f32::NEG_INFINITY, 1_465.0).is_none());
    }

    #[test]
    fn peak_bin_maps_index_to_frequency() {
        let (lvl, hz) = peak_bin(&peaked_at(75)).unwrap();
        assert!((lvl + 10.0).abs() < 1e-6, "peak level is the max bin");
        assert_eq!(hz, 100_100_000, "three quarters across the span");
    }

    #[test]
    fn peak_bin_stays_inside_the_tuned_channel() {
        // A louder signal further out is somebody else's station. Naming it here
        // would also put this row at odds with the headline above it, which is
        // scoped the same way.
        let mut bins = vec![-80.0f32; 101];
        bins[75] = -30.0; // inside the radius
        bins[5]  = 0.0;   // far outside it, and much louder
        let (lvl, hz) = peak_bin(&frame(bins, 100_000_000, 400_000.0)).unwrap();
        assert!((lvl + 30.0).abs() < 1e-6, "reported a station out of channel: {lvl}");
        assert_eq!(hz, 100_100_000);
    }

    #[test]
    fn peak_bin_skips_the_dc_artefact() {
        // The whole point of the guard: on a quiet channel the LO leakage is the
        // tallest bin, and naming it would put the tuned frequency in this row as
        // though a station were there. The next real bin is the honest answer.
        let mut bins = vec![-80.0f32; 101];
        bins[50] = 0.0;   // artefact at centre, by far the strongest
        bins[60] = -30.0; // a real, weaker carrier
        let (lvl, hz) = peak_bin(&frame(bins, 100_000_000, 400_000.0)).unwrap();
        assert!((lvl + 30.0).abs() < 1e-6, "reported the artefact: {lvl}");
        assert_eq!(hz, 100_040_000);
    }

    #[test]
    fn peak_bin_empty_frame_is_none() {
        let fr = frame(vec![], 100_000_000, 2_000_000.0);
        assert!(peak_bin(&fr).is_none());
    }

    #[test]
    fn snr_color_thresholds() {
        let t = crate::theme::Theme::sdr();
        assert_eq!(snr_color(25.0, &t), t.status_ok);
        assert_eq!(snr_color(15.0, &t), t.status_warn);
        assert_eq!(snr_color(5.0, &t), t.status_crit);
    }

    #[test]
    fn acpr_bar_width_matches_bar_w() {
        let t = crate::theme::Theme::sdr();
        let spans = acpr_bar(-38.0, 24, &t);
        let w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(w, 24);
    }

    #[test]
    fn acpr_bar_touching_carrier_is_full_red() {
        let t = crate::theme::Theme::sdr();
        let spans = acpr_bar(0.0, 10, &t);
        assert_eq!(spans.last().unwrap().style.fg, Some(t.status_crit));
    }

    #[test]
    fn acpr_bar_below_floor_is_empty() {
        let t = crate::theme::Theme::sdr();
        let spans = acpr_bar(-95.0, 10, &t);
        let s: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(s.chars().all(|c| c == ' '), "below the display floor reads as clean/empty: {s:?}");
    }

    #[test]
    fn fmt_bw_picks_units() {
        assert_eq!(fmt_bw(180_000), "180.0 kHz");
        assert_eq!(fmt_bw(1_500_000), "1.500 MHz");
        assert_eq!(fmt_bw(400), "400 Hz");
    }

    #[test]
    fn verdict_level_short_labels() {
        assert_eq!(VerdictLevel::Clean.short_label(), "Clean");
        assert_eq!(VerdictLevel::Caution.short_label(), "Caution");
        assert_eq!(VerdictLevel::NoSignal.short_label(), "No signal");
    }

    #[test]
    fn verdict_no_signal_below_classify_gate() {
        // Unknown modulation (e.g. weak/no carrier) never gets a guessed verdict.
        let (level, headline, _) = verdict(Modulation::Unknown, 40.0, -40.0, -40.0, 180_000);
        assert_eq!(level, VerdictLevel::NoSignal);
        assert_eq!(headline, "NO SIGNAL");
        // A known modulation with SNR under the gate is still "no signal".
        let (level, ..) = verdict(Modulation::Wfm, 5.0, -40.0, -40.0, 180_000);
        assert_eq!(level, VerdictLevel::NoSignal);
    }

    #[test]
    fn verdict_weak_signal_below_clean_threshold() {
        let (level, headline, detail) = verdict(Modulation::Wfm, 15.0, -40.0, -40.0, 180_000);
        assert_eq!(level, VerdictLevel::Caution);
        assert!(headline.contains("WEAK"));
        assert!(detail.contains("15 dB"));
    }

    #[test]
    fn verdict_flags_adjacent_splatter_despite_clean_snr() {
        let (level, headline, detail) = verdict(Modulation::Wfm, 40.0, -10.0, -40.0, 180_000);
        assert_eq!(level, VerdictLevel::Caution);
        assert!(headline.contains("SPLATTER"));
        assert!(detail.contains("-10 dB"));
    }

    #[test]
    fn verdict_clean_when_strong_and_well_suppressed() {
        let (level, headline, detail) = verdict(Modulation::Wfm, 43.7, -38.0, -41.0, 180_000);
        assert_eq!(level, VerdictLevel::Clean);
        assert!(headline.contains("CLEAN"));
        assert!(detail.contains("180.0 kHz"));
        assert!(detail.contains("ACPR -38/-41 dB"));
    }

    #[test]
    fn verdict_clean_without_acpr_data_omits_the_clause() {
        // No adjacent-channel measurement yet (e.g. band edge) → no fabricated ACPR note.
        let (level, _, detail) = verdict(Modulation::Nfm, 30.0, f32::NEG_INFINITY, f32::NEG_INFINITY, 15_000);
        assert_eq!(level, VerdictLevel::Clean);
        assert!(!detail.contains("ACPR"));
    }

    #[test]
    fn verdict_needs_both_sides_before_it_quotes_a_ratio() {
        // The clause prints the pair, so half a measurement must produce none of it
        // rather than "ACPR -inf/-38 dB".
        for (lo, hi) in [(f32::NEG_INFINITY, -38.0), (-38.0, f32::NEG_INFINITY)] {
            let (level, _, detail) = verdict(Modulation::Wfm, 30.0, lo, hi, 120_000);
            assert_eq!(level, VerdictLevel::Clean);
            assert!(!detail.contains("ACPR"), "half a pair reached the copy: {detail}");
            assert!(!detail.contains("inf"), "sentinel reached the copy: {detail}");
        }
    }

    #[test]
    fn acpr_labels_name_the_offset_that_was_measured() {
        // Hardcoded "-200k" was right for broadcast FM and wrong everywhere else,
        // now that the spacing follows the modulation.
        use crate::state::acpr_offset_hz;
        assert_eq!(acpr_label('L', acpr_offset_hz(Modulation::Wfm), '\u{2212}'), "L \u{2212}200k");
        assert_eq!(acpr_label('R', acpr_offset_hz(Modulation::Nfm), '+'), "R +25k");
        assert_eq!(acpr_label('R', acpr_offset_hz(Modulation::Am),  '+'), "R +9k");
        assert_eq!(acpr_label('L', 1_500_000.0, '\u{2212}'), "L \u{2212}1.5M");
    }

    #[test]
    fn acpr_labels_fit_the_column() {
        // The rows are laid out on a fixed 8-column label field; an overflow would
        // push the bar out of alignment with the row above it.
        use crate::state::acpr_offset_hz;
        for m in [Modulation::Wfm, Modulation::Nfm, Modulation::Am, Modulation::Unknown] {
            for (side, sign) in [('L', '\u{2212}'), ('R', '+')] {
                let l = acpr_label(side, acpr_offset_hz(m), sign);
                assert!(l.chars().count() <= 8, "{m:?} {side}: {l:?} is {} wide", l.chars().count());
            }
        }
    }
}
