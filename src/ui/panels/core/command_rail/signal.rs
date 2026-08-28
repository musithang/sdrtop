//! The SIGNAL section of the rail: four metric rows, each a faint axis with a
//! braille trace over it and the current value at the end.

use std::collections::VecDeque;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::widgets::charts::{ema_smooth, mini_braille_line};
use crate::ui::widgets::micro_common::{sat_color, snr_color};

use super::smeter::{clip_alert, clip_decay_bg, fmt_since};

/// Short-term trend of a metric history: mean of the recent half minus the older
/// half (same shape as `SignalState::snr_delta`). `None` until ≥4 samples.
fn series_delta(h: &VecDeque<f32>) -> Option<f32> {
    let n = h.len();
    if n < 4 {
        return None;
    }
    let half = n / 2;
    let older: f32 = h.iter().take(half).sum::<f32>() / half as f32;
    let recent: f32 = h.iter().skip(n - half).sum::<f32>() / half as f32;
    Some(recent - older)
}

/// A trend arrow for a metric delta. `good_when_rising` colours the direction by
/// meaning: `Some(true)` → rising is good (SNR), `Some(false)` → rising is bad
/// (NF, SAT), `None` → neutral (PWR). Below `eps` it's a dim steady `→`.
fn trend_arrow(
    delta: Option<f32>,
    eps: f32,
    good_when_rising: Option<bool>,
    theme: &crate::Theme,
) -> Option<Span<'static>> {
    let d = delta?;
    let dir: i8 = if d > eps {
        1
    } else if d < -eps {
        -1
    } else {
        0
    };
    let glyph = match dir {
        1 => "↑",
        -1 => "↓",
        _ => "→",
    };
    let color = match good_when_rising {
        _ if dir == 0 => theme.stale,
        None => theme.stale,
        Some(gw) => {
            if (dir == 1) == gw {
                theme.status_ok
            } else {
                theme.status_warn
            }
        }
    };
    Some(Span::styled(glyph, Style::default().fg(color)))
}

/// Width of the fixed label field (margin + 3-char label + gap) so every metric
/// trace starts at the same column - labels are SNR/PWR/SAT (3) and NF (2).
const METRIC_LABEL_W: usize = 3;

const METRIC_LEAD: usize = 1 + METRIC_LABEL_W + 1;

/// Fixed right-column budget reserved for the value, so every trace is the same
/// width and the values line up. Sized for the widest reading, `" -120.0 dBFS ↘"`
/// (space + value + space + unit + space + arrow = 14).
const METRIC_VALUE_W: usize = 14;

/// One metric as a single instrument row:
/// ```text
///  SNR ▏⣀⣠⡔⡒⡉⡒⡢⡄⣀      43.7 dB ↗
/// ```
/// A faint `▏` left axis anchors an oscilloscope-style braille line trace; the
/// label sits left of the axis, the value right of the trace - neither overlaps it.
/// The trace width is fixed (label field + axis + value budget reserved) so traces
/// and values align across metrics and shrink together with the rail. When value is
/// None (stale) the trace still draws from the buffer and the value shows a dim "—".
struct Metric<'a> {
    /// Three-column name: `SNR`, `PWR`, `FLR`, `SAT`.
    label: &'a str,
    /// Printed after the value, and only when there is one.
    unit: &'a str,
    /// `None` when the reading is stale, which dims the row rather than hiding it.
    value: Option<String>,
    color: Color,
    history: &'a VecDeque<f32>,
    /// Trend arrow, already coloured by meaning.
    arrow: Option<Span<'static>>,
}

fn metric_block(m: Metric<'_>, iw: usize, theme: &crate::Theme) -> Line<'static> {
    let Metric {
        label,
        unit,
        value,
        color: value_color,
        history,
        arrow,
    } = m;
    let val_str = value.as_deref().unwrap_or("—").to_string();
    // One column for the axis, plus the fixed value budget → constant trace width.
    let scope_w = iw.saturating_sub(METRIC_LEAD + 1 + METRIC_VALUE_W).max(4);

    let data: Vec<f32> = history.iter().copied().collect();
    let smoothed = ema_smooth(&data, 0.3);
    let trace = mini_braille_line(&smoothed, scope_w);

    let trace_col = if value.is_some() {
        value_color
    } else {
        theme.border_dim
    };
    let val_col = if value.is_some() {
        value_color
    } else {
        theme.stale
    };

    let mut spans: Vec<Span<'static>> = vec![
        Span::raw(" "),
        Span::styled(
            format!("{label:<w$}", w = METRIC_LABEL_W),
            Style::default().fg(theme.label),
        ),
        Span::raw(" "),
        Span::styled("▏".to_string(), Style::default().fg(theme.border_dim)), // faint axis
        Span::styled(trace, Style::default().fg(trace_col)),
        Span::raw(" "),
        Span::styled(
            val_str,
            Style::default().fg(val_col).add_modifier(Modifier::BOLD),
        ),
    ];
    if value.is_some() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            unit.to_string(),
            Style::default().fg(theme.border_dim),
        ));
        if let Some(a) = arrow {
            spans.push(Span::raw(" "));
            spans.push(a);
        }
    }
    Line::from(spans)
}

/// The SIGNAL section rows: SNR, PWR, FLR and SAT, each a metric block, with a
/// blank spacer between them for breathing room.
///
/// The spacers are droppable: on a short rail `chrome::collapse_spacers` removes
/// only as many as it must, so the rail stays airy for as long as it fits.
pub(super) fn lines(
    state: &SdrMetrics,
    stale: bool,
    active: bool,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let sig = &state.signal;
    let pwr = sig.channel_power_dbfs;
    let nf = state
        .waterfall
        .last_fft
        .as_ref()
        .filter(|_| !stale)
        .map(|fr| fr.noise_floor);
    let snr = sig.peak_to_nf_db;
    let sat = sig.adc_saturation_pct;

    // `good_when_rising` is what makes the trend arrow mean something: rising SNR
    // is good, a rising noise floor or saturation is not, and power is neither -
    // it is just the level the user asked for.
    let mut out = vec![
        metric_block(
            Metric {
                label: "SNR",
                unit: "dB",
                value: (!stale).then(|| format!("{snr:.1}")),
                color: snr_color(snr, theme),
                history: &sig.snr_history,
                arrow: trend_arrow(series_delta(&sig.snr_history), 0.3, Some(true), theme),
            },
            iw,
            theme,
        ),
        Line::raw(""),
        metric_block(
            Metric {
                label: "PWR",
                unit: "dBFS",
                value: (!stale && pwr.is_finite()).then(|| format!("{pwr:.1}")),
                color: theme.value,
                history: &sig.pwr_history,
                arrow: trend_arrow(series_delta(&sig.pwr_history), 0.5, None, theme),
            },
            iw,
            theme,
        ),
        Line::raw(""),
        metric_block(
            Metric {
                label: "FLR",
                unit: "dBFS",
                value: nf.map(|v| format!("{v:.1}")),
                color: theme.value,
                history: &sig.nf_history,
                arrow: trend_arrow(series_delta(&sig.nf_history), 0.3, Some(false), theme),
            },
            iw,
            theme,
        ),
        Line::raw(""),
        metric_block(
            Metric {
                label: "SAT",
                unit: "%",
                value: active.then(|| format!("{sat:.1}")),
                color: sat_color(sat, theme),
                history: &sig.sat_history,
                arrow: trend_arrow(series_delta(&sig.sat_history), 0.5, Some(false), theme),
            },
            iw,
            theme,
        ),
    ];

    // Alert-memory: a recent clip leaves a fading `⚠ last clip Xs` line under
    // SAT. It occupies the SAT section's trailing spacer row rather than adding
    // one, so the total line count - and with it the airy/dense decision - does
    // not change when a clip appears. Before that, the rail collapsed its
    // spacers the moment anything clipped.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    out.push(match clip_alert(sig.last_clip_at, now) {
        Some((since, fresh)) => {
            let mut style = Style::default().fg(if fresh {
                theme.status_crit
            } else {
                theme.stale
            });
            if let Some(bg) = clip_decay_bg(since) {
                style = style.bg(bg);
            }
            Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("\u{26a0} last clip {}", fmt_since(since)), style),
            ])
        }
        None => Line::raw(""),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn trend_arrow_colours_by_meaning() {
        let t = Theme::sdr();
        assert!(trend_arrow(None, 0.3, Some(true), &t).is_none());
        // rising-is-good (SNR): up → ok, down → warn
        assert_eq!(
            trend_arrow(Some(1.0), 0.3, Some(true), &t)
                .unwrap()
                .style
                .fg,
            Some(t.status_ok)
        );
        assert_eq!(
            trend_arrow(Some(-1.0), 0.3, Some(true), &t)
                .unwrap()
                .style
                .fg,
            Some(t.status_warn)
        );
        // rising-is-bad (NF/SAT): up → warn
        assert_eq!(
            trend_arrow(Some(1.0), 0.3, Some(false), &t)
                .unwrap()
                .style
                .fg,
            Some(t.status_warn)
        );
        // neutral (PWR) and within-eps → dim steady
        assert_eq!(
            trend_arrow(Some(1.0), 0.3, None, &t).unwrap().style.fg,
            Some(t.stale)
        );
        assert_eq!(
            trend_arrow(Some(0.0), 0.3, Some(true), &t)
                .unwrap()
                .style
                .fg,
            Some(t.stale)
        );
    }

    #[test]
    fn series_delta_needs_four_samples() {
        let mut h: VecDeque<f32> = VecDeque::new();
        h.extend([10.0, 10.0, 20.0]);
        assert_eq!(series_delta(&h), None);
        h.push_back(20.0); // older half [10,10]=10, recent half [20,20]=20 → +10
        assert!((series_delta(&h).unwrap() - 10.0).abs() < 1e-6);
    }

    #[test]
    /// The rail reads the shared scale, not one of its own.
    ///
    /// It used to escalate at 10 % and 50 %, so a SAT of 20 % was calm here and
    /// red two screens away. Pinned against `crate::state`'s thresholds rather
    /// than literals, so moving the scale moves this test with it.
    fn sat_color_is_the_shared_scale() {
        let t = Theme::sdr();
        assert_eq!(sat_color(0.0, &t), t.status_ok);
        assert_eq!(sat_color(crate::state::SAT_WARN_PCT, &t), t.status_warn);
        assert_eq!(sat_color(crate::state::SAT_CRIT_PCT, &t), t.status_crit);
        assert_eq!(
            sat_color(20.0, &t),
            t.status_crit,
            "20 % is not calm anywhere"
        );
    }
}
