//! `SPECTRAL SHAPE` - how the signal has behaved over the last minute, and how
//! peaky it is.
//!
//! Neither reading is new state: the C/N trend is `snr_history`, the same ring
//! the Command Rail's SNR trace and the micro views read, and the crest figure is
//! the Lab RF bench's own ADC-loading model. Re-deriving either here would give
//! the reader two numbers for one thing.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::chrome::section;

use super::row::{dash, dim, metric, val};

/// Label on the trend row, and the budget kept clear at the end of it for the
/// `±NN.N dB` annotation.
const LABEL: &str = "C/N trend";
const TREND_ANN_W: usize = 10;

pub(super) fn lines(
    out: &mut Vec<Line<'static>>,
    state: &SdrMetrics,
    stale: bool,
    iw: usize,
    theme: &crate::Theme,
) {
    out.push(section("SPECTRAL SHAPE", "60 s", iw, theme));
    if stale {
        out.push(metric("C/N trend", vec![dash(theme)], theme));
        out.push(metric("Crest / PAPR", vec![dash(theme)], theme));
        return;
    }
    let sig = &state.signal;

    // C/N trend: reuses `snr_history` (already fed ~500 ms by the rx poll
    // task, [`crate::state::SNR_HISTORY_LEN`] = 120 deep → 60 s), the same
    // ring the Command Rail's SNR trace and the micro views read. No new
    // state - C/N ≈ peak/noise = SNR.
    let snr_hist: Vec<f32> = sig.snr_history.iter().copied().collect();
    let spark_w = iw
        .saturating_sub(1 + LABEL.chars().count() + 1 + TREND_ANN_W)
        .max(1);
    let (spark, p2p) = crate::ui::widgets::micro_common::spark_minmax(&snr_hist, spark_w);
    if !spark.is_empty() {
        let ann = format!("\u{00b1}{:.1} dB", p2p / 2.0);
        let used = 1 + LABEL.chars().count() + 1 + spark.chars().count() + 1 + ann.chars().count();
        let pad = iw.saturating_sub(used).max(1);
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(LABEL, Style::default().fg(theme.label)),
            Span::raw(" "),
            Span::styled(spark, val(theme)),
            Span::raw(" ".repeat(pad)),
            Span::styled(ann, dim(theme)),
        ]));
    } else {
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(LABEL, Style::default().fg(theme.label)),
            Span::raw("  "),
            dash(theme),
        ]));
    }

    // Crest / PAPR: reuses the exact ADC-loading model from the Lab RF
    // bench (`rf_calc::adc_loading`) rather than re-deriving peak-minus-rms:
    // full-bandwidth ADC crest factor, the same honest proxy the RF lab
    // already shows for "constant-envelope vs peaky".
    let n: u64 = state.iq.adc_signed_hist.iter().sum();
    let load = crate::ui::rf_calc::adc_loading(
        sig.adc_peak_dbfs as f64,
        sig.adc_rms_dbfs as f64,
        sig.adc_clip_events,
        n,
        state.caps.sample_geometry.bits(),
    );
    out.push(metric(
        "Crest / PAPR",
        vec![Span::styled(format!("{:.1} dB", load.crest_db), val(theme))],
        theme,
    ));
}
