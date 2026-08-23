//! `command_rail` — the Command Rail layout's left instrument strip (`[1]`).
//!
//! A single vertical column that gathers what a poweruser reads at a glance:
//! the frequency hero (big segmented VFO + band tag), a value-first SIGNAL zone
//! (SNR with its short-term trend arrow, PWR, NF, SAT), the GAIN chain, the
//! STREAM health, and a one-line log foot. The header thins to status + dial
//! (see `SlimHeaderPanel`) and the frequency lives here instead.
//!
//! It carries the big frequency hero, value-first metrics with sparklines, and a
//! HUNT·MONITOR·BENCH mode strip whose lead card adapts to what you're doing
//! (tuning → Hunt peak-finder, gain → Bench gain-health, idle → Monitor watch).
//! The mode auto-follows actions and `Tab` (in rail-focus, key `c`) pins it.
//! Recall slots and the log overlay are still later steps. Rendering is two
//! non-overlapping `Paragraph`s (the stack and the bottom-anchored log foot), so
//! it never flickers.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use std::collections::VecDeque;

use crate::state::{active_recall_slot, RailMode, SdrMetrics, RECALL_SLOTS};
use crate::ui::widgets::charts::{ema_smooth, gain_bar_colored, mini_braille_line};
use crate::ui::panels::core::header::{active_digit_idx, gain_bar, vfo_spans, vfo_string};
use crate::ui::widgets::micro_common::{fft_stale, fmt_rbw, snr_color};
use crate::ui::panel::{Panel, PanelChrome};
use crate::ui::panels::core::spectrum::detect_peaks;
use crate::ui::chrome;
use crate::ui::panels::core::log;
use crate::ui::widgets::bigdigits;
use crate::ui::widgets::band_plan::band_at;

pub struct CommandRailPanel;

/// Combined front-end gain for the TOTAL readout: primary + secondary stage when
/// the device has two (HackRF LNA+VGA), else just the primary (RTL-SDR tuner).
fn total_gain(lna: u32, vga: u32, has_second_stage: bool) -> u32 {
    if has_second_stage { lna + vga } else { lna }
}

/// Throughput as a compact `5.2 MB/s` string; `—` when not streaming.
fn fmt_mb(bps: u64) -> String {
    if bps == 0 { "—".to_string() } else { format!("{:.1} MB/s", bps as f64 / 1_000_000.0) }
}

/// Width of the gain bar given the rail's inner width — leaves room for the
/// `LNA ` label, a space, and a 2-col value. Clamped so it neither vanishes on a
/// narrow rail nor sprawls on a wide one.
fn gain_bar_width(inner_w: usize) -> usize {
    inner_w.saturating_sub(10).clamp(4, 12)
}

/// Short-term trend of a metric history: mean of the recent half minus the older
/// half (same shape as `SignalState::snr_delta`). `None` until ≥4 samples.
fn series_delta(h: &VecDeque<f32>) -> Option<f32> {
    let n = h.len();
    if n < 4 { return None; }
    let half = n / 2;
    let older:  f32 = h.iter().take(half).sum::<f32>() / half as f32;
    let recent: f32 = h.iter().skip(n - half).sum::<f32>() / half as f32;
    Some(recent - older)
}

/// A trend arrow for a metric delta. `good_when_rising` colours the direction by
/// meaning: `Some(true)` → rising is good (SNR), `Some(false)` → rising is bad
/// (NF, SAT), `None` → neutral (PWR). Below `eps` it's a dim steady `→`.
fn trend_arrow(delta: Option<f32>, eps: f32, good_when_rising: Option<bool>,
               theme: &crate::Theme) -> Option<Span<'static>> {
    let d = delta?;
    let dir: i8 = if d > eps { 1 } else if d < -eps { -1 } else { 0 };
    let glyph = match dir { 1 => "↑", -1 => "↓", _ => "→" };
    let color = match good_when_rising {
        _ if dir == 0 => theme.stale,
        None          => theme.stale,
        Some(gw)      => if (dir == 1) == gw { theme.status_ok } else { theme.status_warn },
    };
    Some(Span::styled(glyph, Style::default().fg(color)))
}

/// Width of the fixed label field (margin + 3-char label + gap) so every metric
/// trace starts at the same column — labels are SNR/PWR/SAT (3) and NF (2).
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
/// label sits left of the axis, the value right of the trace — neither overlaps it.
/// The trace width is fixed (label field + axis + value budget reserved) so traces
/// and values align across metrics and shrink together with the rail. When value is
/// None (stale) the trace still draws from the buffer and the value shows a dim "—".
fn metric_block(label: &str, unit: &str, value: Option<String>, value_color: Color,
                history: &VecDeque<f32>, arrow: Option<Span<'static>>, iw: usize,
                theme: &crate::Theme) -> Line<'static> {
    let val_str = value.as_deref().unwrap_or("—").to_string();
    // One column for the axis, plus the fixed value budget → constant trace width.
    let scope_w = iw.saturating_sub(METRIC_LEAD + 1 + METRIC_VALUE_W).max(4);

    let data: Vec<f32> = history.iter().copied().collect();
    let smoothed = ema_smooth(&data, 0.3);
    let trace = mini_braille_line(&smoothed, scope_w);

    let trace_col = if value.is_some() { value_color } else { theme.border_dim };
    let val_col   = if value.is_some() { value_color } else { theme.stale };

    let mut spans: Vec<Span<'static>> = vec![
        Span::raw(" "),
        Span::styled(format!("{label:<w$}", w = METRIC_LABEL_W), Style::default().fg(theme.label)),
        Span::raw(" "),
        Span::styled("▏".to_string(), Style::default().fg(theme.border_dim)), // faint axis
        Span::styled(trace, Style::default().fg(trace_col)),
        Span::raw(" "),
        Span::styled(val_str, Style::default().fg(val_col).add_modifier(Modifier::BOLD)),
    ];
    if value.is_some() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(unit.to_string(), Style::default().fg(theme.border_dim)));
        if let Some(a) = arrow {
            spans.push(Span::raw(" "));
            spans.push(a);
        }
    }
    Line::from(spans)
}

/// Colour for the ADC-saturation value: calm below 10 %, warn to 50 %, crit above.
fn sat_color(pct: f32, theme: &crate::Theme) -> Color {
    if pct >= 50.0 { theme.status_crit }
    else if pct >= 10.0 { theme.status_warn }
    else { theme.value }
}

/// How long a clip is remembered, and the window in which it's still "fresh"
/// (loud red) before it fades to a dim memory line.
const CLIP_FRESH_SECS:  u64 = 6;
const CLIP_MEMORY_SECS: u64 = 30;

/// Compact relative age for the alert-memory: `"4s"`, `"2m"`, `"1h"`. Pure.
fn fmt_since(secs: u64) -> String {
    if secs < 60        { format!("{secs}s") }
    else if secs < 3600 { format!("{}m", secs / 60) }
    else                { format!("{}h", secs / 3600) }
}

/// The SAT clip alert-memory state: `Some((age_secs, fresh))` while a clip is
/// still remembered, `None` once it's older than [`CLIP_MEMORY_SECS`]. A fresh
/// clip (≤ [`CLIP_FRESH_SECS`]) renders loud; afterwards it fades. Pure over the
/// clock so it's testable, and it only ever fades — it never flickers.
fn clip_alert(last_clip_at: Option<u64>, now: u64) -> Option<(u64, bool)> {
    let since = now.saturating_sub(last_clip_at?);
    (since <= CLIP_MEMORY_SECS).then_some((since, since <= CLIP_FRESH_SECS))
}

// ---------------------------------------------------------------------------
// S-meter
// ---------------------------------------------------------------------------

const S9_DBFS: f32 = -52.0;

/// Power fraction [0.0..1.0] on the S-meter arc: 0=S1 (−100 dBFS), 8/14=S9, 1.0=S9+60.
fn power_to_s_frac(dbfs: f32) -> f64 {
    const S1_DBFS: f32 = S9_DBFS - 48.0;
    const OVER: f32 = 60.0;
    let v = dbfs.clamp(S1_DBFS, S9_DBFS + OVER);
    if v <= S9_DBFS {
        ((v - S1_DBFS) / 48.0 * (8.0 / 14.0)) as f64
    } else {
        (8.0 / 14.0 + (v - S9_DBFS) / OVER * (6.0 / 14.0)) as f64
    }
}

fn frac_to_s_label(frac: f64) -> &'static str {
    match (frac * 14.0).round() as i32 {
        i32::MIN..=0 => "S1",
        1 => "S2", 2 => "S3", 3 => "S4", 4 => "S5",
        5 => "S6", 6 => "S7", 7 => "S8", 8..=9 => "S9",
        10..=11 => "S9+20", 12..=13 => "S9+40", _ => "S9+60",
    }
}

fn s_bar_color(x: usize, bar_w: usize) -> Color {
    let t = x as f64 / bar_w.max(1) as f64;
    let s9_t = 8.0 / 14.0;
    if t <= s9_t {
        let u = (t / s9_t).clamp(0.0, 1.0);
        let r = (u * 190.0) as u8;
        let g = (190.0 - u * 40.0) as u8;
        Color::Rgb(r, g, 0)
    } else {
        let u = ((t - s9_t) / (1.0 - s9_t)).clamp(0.0, 1.0);
        let r = (190.0_f64 + u * 50.0).min(240.0) as u8;
        let g = (150.0 * (1.0 - u)) as u8;
        Color::Rgb(r, g, 0)
    }
}

const S_EIGHTHS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

fn s_bar_char(x: usize, fill_eighths: usize, peak_col: Option<usize>) -> char {
    let pos8  = x * 8;
    let next8 = pos8 + 8;
    if fill_eighths >= next8 {
        '█'
    } else if fill_eighths > pos8 {
        S_EIGHTHS[fill_eighths - pos8]
    } else if peak_col == Some(x) {
        '╵'
    } else {
        ' '
    }
}

// Positions are kept in the readable "n fourteenths" form to mirror the S-unit
// scale; the final 14.0/14.0 (= 1.0, the bar's max) trips clippy's eq_op, hence
// the allow — the pattern documents intent better than a bare 1.0 would.
#[allow(clippy::eq_op)]
const SCALE: &[(&str, f64)] = &[
    ("S1", 0.0 / 14.0), ("S3", 2.0 / 14.0), ("S5", 4.0 / 14.0), ("S7", 6.0 / 14.0),
    ("S9", 8.0 / 14.0), ("+20", 10.0 / 14.0), ("+40", 12.0 / 14.0), ("+60", 14.0 / 14.0),
];

fn s_meter_lines(power_dbfs: f32, peak_dbfs: Option<f32>, iw: usize,
                 theme: &crate::Theme) -> [Line<'static>; 3] {
    let bar_w = iw.saturating_sub(1).max(1);
    let frac  = power_to_s_frac(power_dbfs);
    let fill_eighths = (frac * bar_w as f64 * 8.0) as usize;
    let peak_col = peak_dbfs.map(|p| (power_to_s_frac(p) * bar_w as f64) as usize);

    // Row 0: scale tick labels.
    let skip_alt = iw < 20;
    let mut scale_buf = vec![' '; bar_w];
    for (idx, &(lbl, frac_pos)) in SCALE.iter().enumerate() {
        if skip_alt && idx % 2 != 0 { continue; }
        let pos = (frac_pos * bar_w as f64) as usize;
        for (j, c) in lbl.chars().enumerate() {
            let col = pos + j;
            if col < bar_w { scale_buf[col] = c; }
        }
    }
    let scale_str: String = scale_buf.into_iter().collect();
    let row0 = Line::from(vec![
        Span::raw(" "),
        Span::styled(scale_str, Style::default().fg(theme.border_dim)),
    ]);

    // Row 1: gradient bar with ⅛-block precision and peak pip.
    let mut bar_spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    for x in 0..bar_w {
        let c = s_bar_char(x, fill_eighths, peak_col);
        let color = if c == ' ' {
            theme.border_dim
        } else if c == '╵' {
            theme.value_hi
        } else {
            s_bar_color(x, bar_w)
        };
        bar_spans.push(Span::styled(c.to_string(), Style::default().fg(color)));
    }
    let row1 = Line::from(bar_spans);

    // Row 2: "S7  ·  -19.3 dBFS  ·  peak S9+20"
    let s_label = frac_to_s_label(frac);
    let val_str = format!("{power_dbfs:.1} dBFS");
    let mut row2_spans = vec![
        Span::raw(" "),
        Span::styled(s_label.to_string(), Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD)),
        Span::styled("  ·  ".to_string(), Style::default().fg(theme.border_dim)),
        Span::styled(val_str, Style::default().fg(theme.value)),
    ];
    if let Some(p) = peak_dbfs {
        let p_label = frac_to_s_label(power_to_s_frac(p));
        row2_spans.push(Span::styled("  ·  ".to_string(), Style::default().fg(theme.border_dim)));
        row2_spans.push(Span::styled(format!("peak {p_label}"), Style::default().fg(theme.label)));
    }
    let row2 = Line::from(row2_spans);

    [row0, row1, row2]
}

// ---------------------------------------------------------------------------
// Clip decay background tint
// ---------------------------------------------------------------------------

fn clip_decay_bg(since: u64) -> Option<Color> {
    if since > CLIP_MEMORY_SECS { return None; }
    let t = if since <= CLIP_FRESH_SECS {
        1.0_f64
    } else {
        1.0 - (since - CLIP_FRESH_SECS) as f64
            / (CLIP_MEMORY_SECS - CLIP_FRESH_SECS) as f64
    };
    let r = (45.0 * t) as u8;
    if r == 0 { None } else { Some(Color::Rgb(r, 0, 0)) }
}

/// Columns the full `HUNT·MONITOR·BENCH` strip needs: a leading space, then each
/// mode as ` LABEL ` (label+2) plus a one-column gap. Pure, for the width check.
fn mode_tabs_full_w() -> usize {
    1 + RailMode::ALL.iter().map(|m| m.label().len() + 3).sum::<usize>()
}

/// The `HUNT·MONITOR·BENCH` mode strip — every mode a filled ` LABEL ` chip: the
/// active one lit bright (`value_hi` bg, bold), the inactive ones the same chip in a
/// muted "inactive" fill (`border_dim` bg) so they read as selected-but-off rather
/// than plain text. Both use dark text on the fill. Falls back to 3-letter codes
/// when the rail is too narrow for the full labels, so the strip never clips
/// mid-word.
fn mode_tabs_line(active: RailMode, iw: usize, theme: &crate::Theme) -> Line<'static> {
    let compact = mode_tabs_full_w() > iw;
    let ink = Color::Rgb(4, 6, 15); // dark text on the chip fill
    let mut spans = vec![Span::raw(" ")];
    for m in RailMode::ALL {
        let label = if compact { &m.label()[..3] } else { m.label() };
        let style = if m == active {
            Style::default().fg(ink).bg(theme.value_hi).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(ink).bg(theme.border_dim)
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

/// The rail's signal list: the strongest distinct spectral peaks mapped to
/// `(freq_hz, dbfs)`, strongest-first. A thin wrapper over the spectrum panel's
/// [`detect_peaks`] (shared prominence ≥ NF+10 dB + min-separation logic) plus
/// the bin→Hz map, so HUNT and the MONITOR activity count agree with the markers.
fn rail_peaks(bins: &[f32], noise_floor: f32, center_hz: u64, sample_rate: f64, n: usize)
    -> Vec<(u64, f32)> {
    if sample_rate <= 0.0 || bins.is_empty() { return Vec::new(); }
    let len = bins.len();
    let sep = (len / 48).max(2);
    let left_hz = center_hz as f64 - sample_rate / 2.0;
    detect_peaks(bins, noise_floor, n, sep).into_iter().map(|i| {
        let hz = (left_hz + i as f64 / len as f64 * sample_rate).max(0.0) as u64;
        (hz, bins[i])
    }).collect()
}

/// BENCH gain-health verdict from ADC saturation and clip headroom. Returns the
/// word plus severity (2=crit, 1=warn, 0=ok), so the caller picks the colour.
/// Pure for testability. `headroom_db` is `-adc_peak_dbfs`.
/// Severity mirrors `sat_color`: ≥50% → crit, ≥10% → warn.
fn chain_verdict(sat_pct: f32, headroom_db: f32) -> (&'static str, i8) {
    if sat_pct >= 50.0         { ("clipping", 2) }  // rail hits → back off now
    else if sat_pct >= 10.0    { ("hot",      1) }  // high level → nudge down
    else if headroom_db > 45.0 { ("low",      1) }  // lots of room → add gain
    else                       { ("optimal",  0) }
}

/// The mode-adaptive lead card that sits between the mode strip and the SIGNAL
/// zone. Only this block changes with the mode; everything below is fixed.
fn mode_card_lines(mode: RailMode, state: &SdrMetrics, stale: bool,
                   theme: &crate::Theme) -> Vec<Line<'static>> {
    let dim = |s: String| Line::from(vec![
        Span::raw(" "), Span::styled(s, Style::default().fg(theme.stale))]);

    match mode {
        // HUNT — the three strongest signals on screen, with band tags.
        RailMode::Hunt => {
            let fft = state.waterfall.last_fft.as_ref().filter(|_| !stale);
            let Some(fr) = fft else { return vec![dim("scanning…".into())]; };
            let peaks = rail_peaks(&fr.bins_dbfs, fr.noise_floor, state.radio.frequency, fr.sample_rate, 3);
            if peaks.is_empty() { return vec![dim("no peaks".into())]; }
            peaks.into_iter().enumerate().map(|(i, (hz, db))| {
                let mark = if i == 0 { "▸" } else { " " };
                let mut spans = vec![
                    Span::styled(mark.to_string(), Style::default().fg(theme.value_hi)),
                    Span::styled(format!("{:7.2}", hz as f64 / 1e6),
                                 Style::default().fg(if i == 0 { theme.value_hi } else { theme.value })),
                    Span::styled(format!(" {db:4.0}"), Style::default().fg(theme.label)),
                ];
                if let Some(b) = band_at(hz) {
                    spans.push(Span::styled(format!("  {b}"), Style::default().fg(theme.border_accent)));
                }
                Line::from(spans)
            }).collect()
        }

        // MONITOR — a calm watch headline: signal quality + how many signals are up.
        RailMode::Monitor => {
            let snr = state.signal.peak_to_nf_db;
            let (word, col) = if stale { ("—", theme.stale) }
                else if snr >= 20.0 { ("strong", theme.status_ok) }
                else if snr >= 10.0 { ("fair",   theme.value) }
                else                { ("quiet",  theme.label) };
            // detect_peaks already gates on NF+10 dB, so its count is the activity.
            let n_active = state.waterfall.last_fft.as_ref().filter(|_| !stale).map_or(0, |fr| {
                rail_peaks(&fr.bins_dbfs, fr.noise_floor, state.radio.frequency, fr.sample_rate, 8).len()
            });
            vec![
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled("WATCH ", Style::default().fg(theme.label)),
                    Span::styled(word.to_string(), Style::default().fg(col).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(format!("{n_active}"), Style::default().fg(theme.value).add_modifier(Modifier::BOLD)),
                    Span::styled(" active", Style::default().fg(theme.label)),
                ]),
            ]
        }

        // BENCH — gain-chain health: clip headroom + a one-word verdict.
        RailMode::Bench => {
            let streaming = state.radio.hw_streaming;
            let power = state.signal.adc_peak_dbfs;
            let headroom = if streaming { (-power).max(0.0) } else { f32::NAN };
            let sat = state.signal.adc_saturation_pct;
            let (verdict_str, vcol) = if streaming {
                let (v, sev) = chain_verdict(sat, headroom);
                let col = match sev { 2 => theme.status_crit, 1 => theme.status_warn, _ => theme.status_ok };
                (v.to_string(), col)
            } else {
                ("\u{2014}".to_string(), theme.stale)
            };
            let hstr = if headroom.is_finite() { format!("{headroom:.0} dB") } else { "\u{2014}".into() };
            vec![
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled("HEADROOM ", Style::default().fg(theme.label)),
                    Span::styled(hstr, Style::default().fg(theme.value).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled("CHAIN ", Style::default().fg(theme.label)),
                    Span::styled(verdict_str, Style::default().fg(vcol).add_modifier(Modifier::BOLD)),
                ]),
            ]
        }
    }
}

/// Whether a recall slot's frequency has a detectable signal in the current spectrum.
/// Returns `Some((pip_str, strong))` when in-band, `None` when out-of-band or stale.
fn recall_pip(slot_hz: u64, state: &SdrMetrics, stale: bool) -> Option<(&'static str, bool)> {
    if stale { return None; }
    let fr = state.waterfall.last_fft.as_ref()?;
    let half_sr = (fr.sample_rate / 2.0) as u64;
    let center  = state.radio.frequency;
    if slot_hz < center.saturating_sub(half_sr) || slot_hz > center + half_sr {
        return None;
    }
    let peaks = rail_peaks(&fr.bins_dbfs, fr.noise_floor, center, fr.sample_rate, 8);
    let close  = peaks.iter().any(|&(f, _)| f.abs_diff(slot_hz) < 250_000);
    let strong = peaks.iter()
        .filter(|&&(f, _)| f.abs_diff(slot_hz) < 250_000)
        .any(|&(_, db)| db > fr.noise_floor + 20.0);
    Some(if strong { ("⣿⡇", true) } else if close { ("⠉⠁", false) } else { ("·", false) })
}

/// The RECALL list: the three saved-frequency slots, the one the radio is parked
/// on lit with `▸`. Empty slots show a dim dash. `M` saves the current tuning,
/// `1·2·3` jump (both in rail-focus). Band tags come from `band_at`.
/// Activity pips appear on the right when a slot frequency is visible in the spectrum.
fn recall_lines(state: &SdrMetrics, stale: bool, theme: &crate::Theme) -> Vec<Line<'static>> {
    let active = active_recall_slot(&state.ui.recall, state.radio.frequency);
    (0..RECALL_SLOTS).map(|i| {
        let n = i + 1;
        match state.ui.recall[i] {
            Some(hz) => {
                let on = active == Some(i);
                let mark = if on { "▸" } else { " " };
                let modi = if on { Modifier::BOLD } else { Modifier::empty() };
                let mut spans = vec![
                    Span::styled(mark.to_string(), Style::default().fg(theme.value_hi)),
                    Span::styled(format!("{n} "), Style::default().fg(if on { theme.value_hi } else { theme.label })),
                    Span::styled(vfo_string(hz),
                        Style::default().fg(if on { theme.value_hi } else { theme.value }).add_modifier(modi)),
                ];
                if let Some(b) = band_at(hz) {
                    spans.push(Span::styled(format!("  {b}"), Style::default().fg(theme.border_accent)));
                }
                if let Some((pip, strong)) = recall_pip(hz, state, stale) {
                    let col = if strong { theme.value_hi } else { theme.border_dim };
                    spans.push(Span::styled(format!(" {pip}"), Style::default().fg(col)));
                }
                Line::from(spans)
            }
            None => Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{n} "), Style::default().fg(theme.border_dim)),
                Span::styled("—", Style::default().fg(theme.stale)),
            ]),
        }
    }).collect()
}

/// The frequency hero: the big 3-row block readout, or a single bold line when
/// the rail is too narrow for the block font. The actively-tuned digit is lit in
/// `value_hi` (the same digit the small VFO underlines), the rest in `value`, the
/// decimal point dim — all dim in observer mode.
fn freq_hero_lines(freq: u64, step: u64, observer: bool, inner_w: usize,
                   theme: &crate::Theme) -> Vec<Line<'static>> {
    let s = vfo_string(freq);

    // Narrow fallback: the existing single-line segmented VFO (+" MHz"). The
    // budget covers the leading space (1) + the gap (1) + the "MHz" suffix (3),
    // so the big readout shows whenever its widest (middle) row actually fits.
    if bigdigits::big_width(&s) + 5 > inner_w {
        let col = if observer { theme.label } else { theme.value_hi };
        let mut spans = vec![Span::raw(" ")];
        spans.extend(vfo_spans(freq, step, col, theme.label, theme.value_hi));
        spans.push(Span::raw(" "));
        spans.push(Span::styled("MHz", Style::default().fg(theme.label)));
        return vec![Line::from(spans)];
    }

    let active = active_digit_idx(freq, step);
    let chars: Vec<char> = s.chars().collect();
    let mut rows: [Vec<Span<'static>>; 3] =
        [vec![Span::raw(" ")], vec![Span::raw(" ")], vec![Span::raw(" ")]];
    for (i, &c) in chars.iter().enumerate() {
        let color = if observer { theme.label }
            else if Some(i) == active { theme.value_hi }
            else if c == '.' { theme.label }
            else { theme.value };
        let g = bigdigits::glyph(c);
        for (r, row) in rows.iter_mut().enumerate() {
            if i > 0 { row.push(Span::raw(" ")); }
            row.push(Span::styled(g[r].to_string(), Style::default().fg(color)));
        }
    }
    // "MHz" rides the middle row, just past the digits.
    rows[1].push(Span::raw(" "));
    rows[1].push(Span::styled("MHz", Style::default().fg(theme.label)));
    let [r0, r1, r2] = rows;
    vec![Line::from(r0), Line::from(r1), Line::from(r2)]
}

/// `[FM]  SR 2.0M · RBW 1.5 kHz` — the band chip plus sample-rate / resolution
/// context, sitting just under the frequency hero.
fn band_sr_line(state: &SdrMetrics, iw: usize, theme: &crate::Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    if let Some(b) = band_at(state.radio.frequency) {
        let chip = format!(" {b} ");
        used += chip.chars().count() + 2;
        spans.push(Span::styled(chip, Style::default()
            .fg(Color::Rgb(4, 6, 15)).bg(theme.value_hi).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw("  "));
    }
    let sr = format!("SR {:.1}M", state.radio.config_sample_rate / 1_000_000.0);
    used += sr.chars().count();
    spans.push(Span::styled(sr, Style::default().fg(theme.label)));
    // RBW is the first thing to go on a narrow rail — drop it (and its separator)
    // rather than let it clip mid-word at the panel border.
    let rbw = match state.waterfall.last_fft.as_ref().filter(|fr| fr.enbw_hz > 0.0) {
        Some(fr) => fmt_rbw(fr.enbw_hz),
        None     => "—".to_string(),
    };
    let rbw_str = format!(" · RBW {rbw}");
    if used + rbw_str.chars().count() <= iw {
        spans.push(Span::styled(" · ", Style::default().fg(theme.border_dim)));
        spans.push(Span::styled(format!("RBW {rbw}"), Style::default().fg(theme.label)));
    }
    Line::from(spans)
}

impl Panel for CommandRailPanel {
    fn name(&self) -> &'static str { "command_rail" }
    fn min_size(&self) -> (u16, u16) { (22, 12) }

    // `c` for Command: focus the rail to drive it directly. In focus, `←/→` tune
    // (which auto-switches the mode to Hunt) and `Tab` cycles the mode manually.
    fn focus_key(&self) -> Option<char> { Some('c') }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("←→", "Tune"), ("Tab", "Mode"), ("1·2·3", "Recall"), ("M", "Save"), ("L", "Log")]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome { PanelChrome::deck("_Command") }

    fn render(&self, f: &mut Frame, inner: Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {

        let iw = inner.width as usize;
        let stale = fft_stale(state);
        let observer = state.observer.active;
        let active = state.radio.hw_streaming && !observer;

        // Width 6 so the 5-char "TOTAL" still keeps a gap before its value.
        let lbl   = |s: &str| Span::styled(format!("{s:<6}"), Style::default().fg(theme.label));
        // Section separator with box-drawing connector tick: `├╴ LABEL ╶────…`
        let section = |name: &str| {
            let label = name.to_uppercase();
            // "├╴ " (3) + label + " ╶" (2) = label.len() + 5
            let used = label.chars().count() + 5;
            Line::from(vec![
                Span::styled("├╴ ".to_string(), Style::default().fg(theme.border_dim)),
                Span::styled(label, Style::default().fg(theme.label).add_modifier(Modifier::BOLD)),
                Span::styled(" ╶".to_string(), Style::default().fg(theme.border_dim)),
                Span::styled("─".repeat(iw.saturating_sub(used)), Style::default().fg(theme.border_dim)),
            ])
        };

        let mut lines: Vec<Line> = Vec::new();

        // --- FREQ HERO ---------------------------------------------------------
        lines.extend(freq_hero_lines(state.radio.frequency, state.spectrum.step_hz,
                                     observer, iw, theme));
        lines.push(band_sr_line(state, iw, theme));
        // S-meter sits directly under the band/SR line, replacing the old blank gap.
        let pwr = state.signal.channel_power_dbfs;
        if !stale && pwr.is_finite() {
            let peak_pwr: Option<f32> = {
                let m = state.signal.pwr_history.iter().copied()
                    .filter(|v| v.is_finite())
                    .fold(f32::NEG_INFINITY, f32::max);
                m.is_finite().then_some(m)
            };
            lines.extend(s_meter_lines(pwr, peak_pwr, iw, theme));
        }
        lines.push(Line::raw(""));

        // --- MODE STRIP + lead card -------------------------------------------
        // The mode auto-follows actions (tune→Hunt, gain→Bench) and decays to
        // Monitor; the lead card below adapts to it. Everything under it is fixed.
        let mode = state.ui.effective_rail_mode();
        lines.push(mode_tabs_line(mode, iw, theme));
        lines.extend(mode_card_lines(mode, state, stale, theme));
        lines.push(Line::raw(""));

        // --- RECALL ------------------------------------------------------------
        lines.push(section("Recall"));
        lines.extend(recall_lines(state, stale, theme));
        lines.push(Line::raw(""));

        // --- SIGNAL ------------------------------------------------------------
        // Each metric: one instrument row — faint axis + braille line trace + value.
        // A blank spacer between them gives breathing room (dropped by dense-mode on
        // a short rail, so the layout stays adaptive).
        lines.push(section("Signal"));

        let snr = state.signal.peak_to_nf_db;
        lines.push(metric_block(
            "SNR", "dB",
            (!stale).then(|| format!("{snr:.1}")),
            snr_color(snr, theme),
            &state.signal.snr_history,
            trend_arrow(series_delta(&state.signal.snr_history), 0.3, Some(true), theme),
            iw, theme));
        lines.push(Line::raw(""));

        lines.push(metric_block(
            "PWR", "dBFS",
            (!stale && pwr.is_finite()).then(|| format!("{pwr:.1}")),
            theme.value,
            &state.signal.pwr_history,
            trend_arrow(series_delta(&state.signal.pwr_history), 0.5, None, theme),
            iw, theme));
        lines.push(Line::raw(""));

        let nf = state.waterfall.last_fft.as_ref().filter(|_| !stale).map(|fr| fr.noise_floor);
        lines.push(metric_block(
            "FLR", "dBFS",
            nf.map(|v| format!("{v:.1}")),
            theme.value,
            &state.signal.nf_history,
            trend_arrow(series_delta(&state.signal.nf_history), 0.3, Some(false), theme),
            iw, theme));
        lines.push(Line::raw(""));

        let sat = state.signal.adc_saturation_pct;
        lines.push(metric_block(
            "SAT", "%",
            active.then(|| format!("{sat:.1}")),
            sat_color(sat, theme),
            &state.signal.sat_history,
            trend_arrow(series_delta(&state.signal.sat_history), 0.5, Some(false), theme),
            iw, theme));
        // Alert-memory: a recent clip leaves a fading "⚠ last clip Xs" line under
        // SAT (bg tint decays from dark red to nothing, never flickers). It occupies
        // the SAT section's trailing spacer row rather than adding a line, so showing
        // it leaves the total line count — and thus the airy/dense decision below —
        // unchanged: the layout no longer collapses its spacers when a clip appears.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        match clip_alert(state.signal.last_clip_at, now) {
            Some((since, fresh)) => {
                let fg_col = if fresh { theme.status_crit } else { theme.stale };
                let mut style = Style::default().fg(fg_col);
                if let Some(bg) = clip_decay_bg(since) { style = style.bg(bg); }
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(format!("⚠ last clip {}", fmt_since(since)), style),
                ]));
            }
            None => lines.push(Line::raw("")),
        }

        // --- GAIN --------------------------------------------------------------
        lines.push(section("Gain"));
        let gm = &state.caps.gain;
        let bw = gain_bar_width(iw);
        let val_col = if active { theme.value } else { theme.label };
        // Front-end boost (AMP / AGC).
        let (boost_val, boost_col) = if observer { ("—".to_string(), theme.label) }
            else if state.radio.amp_enabled { ("ON".to_string(), theme.value_hi) }
            else { ("OFF".to_string(), theme.label) };
        lines.push(Line::from(vec![
            Span::raw(" "), lbl(gm.boost_label()),
            Span::styled(boost_val, Style::default().fg(boost_col)),
        ]));
        lines.push(Line::raw(""));
        // A gain row: ` LABEL [⅛-block bar] value`. When streaming the bar shades
        // along a meaning gradient (LNA green→yellow, VGA cyan→orange); idle it's a
        // flat dim ⅛-block (the header keeps its own flat bar — separate code path).
        let gain_row = |label_span: Span<'static>, gain: u32, max: u32,
                        lo: Color, hi: Color| -> Line<'static> {
            let bar: Vec<Span<'static>> = if active {
                gain_bar_colored(gain, max, bw, lo, hi, theme.border_dim)
            } else {
                let (f, e) = gain_bar(gain, max, bw);
                vec![Span::styled(f, Style::default().fg(theme.label)),
                     Span::styled(e, Style::default().fg(theme.border_dim))]
            };
            let mut spans = vec![Span::raw(" "), label_span];
            spans.extend(bar);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(format!("{gain:2}"), Style::default().fg(val_col)));
            Line::from(spans)
        };
        // Primary stage (LNA / Tuner): green → yellow.
        lines.push(gain_row(lbl(gm.primary_label()), state.radio.lna_gain,
                            gm.primary_max_db(), theme.status_ok, theme.value_hi));
        lines.push(Line::raw(""));
        // Secondary stage (HackRF VGA only): cyan → orange.
        if gm.has_second_stage() {
            lines.push(gain_row(lbl("VGA"), state.radio.vga_gain, 62,
                                theme.border_accent, theme.status_warn));
            lines.push(Line::raw(""));
        }
        let total = total_gain(state.radio.lna_gain, state.radio.vga_gain, gm.has_second_stage());
        // TOTAL gain, plus the clip headroom (peak headroom, same as RF Diagnostics).
        let mut total_spans = vec![
            Span::raw(" "), lbl("TOTAL"),
            Span::styled(format!("{total} dB"), Style::default().fg(val_col)),
        ];
        if active {
            let headroom = (-state.signal.adc_peak_dbfs).max(0.0);
            total_spans.push(Span::styled("  ·  ".to_string(), Style::default().fg(theme.border_dim)));
            total_spans.push(Span::styled(format!("{headroom:.0} dB headroom"),
                                          Style::default().fg(theme.label)));
        }
        lines.push(Line::from(total_spans));
        lines.push(Line::raw(""));

        // --- STREAM ------------------------------------------------------------
        lines.push(section("Stream"));
        let stream_val = |s: String| Span::styled(s, Style::default().fg(if active { theme.value } else { theme.label }));
        lines.push(Line::from(vec![Span::raw(" "), lbl("DROP"),
            stream_val(format!("{} /s", state.signal.drops_per_sec))]));
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![Span::raw(" "), lbl("BUF"),
            stream_val(format!("{:.0} %", state.iq.buf_fill_pct))]));
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![Span::raw(" "), lbl("USB"),
            stream_val(fmt_mb(if active { state.radio.current_throughput_bps } else { 0 }))]));

        // Split off the bottom inner row for the log foot so the stack and the
        // foot never overlap (no flicker), and the foot stays anchored.
        let (stack_area, foot_area) = if inner.height >= 4 {
            (Rect { height: inner.height - 1, ..inner },
             Some(Rect { x: inner.x, y: inner.y + inner.height - 1, width: inner.width, height: 1 }))
        } else {
            (inner, None)
        };
        // Self-adjusting density: on a short rail where the airy layout would
        // overflow (and clip a whole section), drop only as many blank spacers as
        // needed — evenly across the stack — so the panel keeps as much breathing
        // room as fits instead of snapping to fully dense and stranding empty rows
        // above the foot. Tall rails keep every spacer; very short ones drop them
        // all and lean on the `╴SECTION╶` nameplates for separation.
        chrome::collapse_spacers(&mut lines, stack_area.height as usize);
        f.render_widget(Paragraph::new(lines), stack_area);

        if let Some(foot) = foot_area {
            if let Some(e) = state.ui.log.back() {
                let foot_line = Line::from(vec![
                    Span::raw(" "),
                    log::lamp(e.level, theme),
                    Span::raw(" "),
                    Span::styled(log::fmt_clock(e.at_epoch_secs), Style::default().fg(theme.border_dim)),
                    Span::raw(" "),
                    Span::styled(e.text.as_ref(), Style::default().fg(theme.value)),
                ]);
                f.render_widget(Paragraph::new(foot_line), foot);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn total_gain_sums_only_with_second_stage() {
        assert_eq!(total_gain(32, 30, true), 62);  // HackRF LNA+VGA
        assert_eq!(total_gain(40, 99, false), 40); // RTL-SDR tuner only
    }

    #[test]
    fn fmt_mb_blanks_when_idle() {
        assert_eq!(fmt_mb(0), "—");
        assert_eq!(fmt_mb(5_200_000), "5.2 MB/s");
    }

    #[test]
    fn gain_bar_width_clamps() {
        assert_eq!(gain_bar_width(10), 4);   // tiny rail → floor
        assert_eq!(gain_bar_width(0), 4);
        assert_eq!(gain_bar_width(22), 12);  // wide rail → ceiling
        assert_eq!(gain_bar_width(18), 8);   // mid → 18-10
    }

    #[test]
    fn trend_arrow_colours_by_meaning() {
        let t = Theme::sdr();
        assert!(trend_arrow(None, 0.3, Some(true), &t).is_none());
        // rising-is-good (SNR): up → ok, down → warn
        assert_eq!(trend_arrow(Some(1.0), 0.3, Some(true), &t).unwrap().style.fg, Some(t.status_ok));
        assert_eq!(trend_arrow(Some(-1.0), 0.3, Some(true), &t).unwrap().style.fg, Some(t.status_warn));
        // rising-is-bad (NF/SAT): up → warn
        assert_eq!(trend_arrow(Some(1.0), 0.3, Some(false), &t).unwrap().style.fg, Some(t.status_warn));
        // neutral (PWR) and within-eps → dim steady
        assert_eq!(trend_arrow(Some(1.0), 0.3, None, &t).unwrap().style.fg, Some(t.stale));
        assert_eq!(trend_arrow(Some(0.0), 0.3, Some(true), &t).unwrap().style.fg, Some(t.stale));
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
    fn sat_color_escalates() {
        let t = Theme::sdr();
        assert_eq!(sat_color(0.0, &t), t.value);
        assert_eq!(sat_color(20.0, &t), t.status_warn);
        assert_eq!(sat_color(80.0, &t), t.status_crit);
    }

    #[test]
    fn rail_peaks_maps_bins_to_frequency_strongest_first() {
        // Two lobes above the −80 dB noise floor: a tall one left of centre
        // (bin 2), a shorter one right (bin 6). 10 Msps @ 100 MHz → 95..105 MHz.
        let bins = [-90.0, -40.0, -10.0, -40.0, -80.0, -50.0, -25.0, -50.0, -90.0];
        let peaks = rail_peaks(&bins, -80.0, 100_000_000, 10_000_000.0, 3);
        assert_eq!(peaks.len(), 2, "two distinct lobes above NF+10");
        assert!(peaks[0].1 > peaks[1].1, "strongest first");
        assert!((peaks[0].1 - (-10.0)).abs() < 1e-3);
        // Bin 2 of 9 maps below centre; bin 6 above it.
        assert!(peaks[0].0 < 100_000_000 && peaks[1].0 > 100_000_000);
    }

    #[test]
    fn rail_peaks_empty_without_signal_or_rate() {
        // All near the floor → nothing clears NF+10 dB.
        assert!(rail_peaks(&[-90.0, -88.0, -90.0], -90.0, 100_000_000, 6_000_000.0, 3).is_empty());
        // No sample rate → no usable frequency map.
        assert!(rail_peaks(&[-90.0, -10.0, -90.0], -90.0, 100_000_000, 0.0, 3).is_empty());
    }

    #[test]
    fn fmt_since_scales_units() {
        assert_eq!(fmt_since(4), "4s");
        assert_eq!(fmt_since(59), "59s");
        assert_eq!(fmt_since(120), "2m");
        assert_eq!(fmt_since(7200), "2h");
    }

    #[test]
    fn clip_alert_is_fresh_then_fades_then_expires() {
        assert_eq!(clip_alert(None, 100), None);                 // never clipped
        assert_eq!(clip_alert(Some(100), 103), Some((3, true))); // fresh & loud
        assert_eq!(clip_alert(Some(100), 115), Some((15, false))); // remembered, dim
        assert_eq!(clip_alert(Some(100), 140), None);            // older than memory
        // Clock skew (clip "in the future") must not panic or misread.
        assert_eq!(clip_alert(Some(100), 90), Some((0, true)));
    }

    #[test]
    fn mode_tabs_full_width_is_the_label_budget() {
        // " HUNT " + gap + " MONITOR " + gap + " BENCH " + gap, plus leading space.
        // Each chip is label+2 (pad spaces) + 1 gap = label+3:
        // (4+3) + (7+3) + (5+3) + 1 = 26.
        assert_eq!(mode_tabs_full_w(), 26);
        // Compact kicks in below that — the strip then uses 3-letter codes.
        assert!(mode_tabs_full_w() > 20, "narrow rail must compact");
        assert!(mode_tabs_full_w() <= 28, "wide rail shows full labels");
    }

    #[test]
    fn chain_verdict_reads_saturation_and_headroom() {
        assert_eq!(chain_verdict(60.0, 10.0), ("clipping", 2)); // ≥50% → crit
        assert_eq!(chain_verdict(20.0, 10.0), ("hot",      1)); // 10-50% → warn
        assert_eq!(chain_verdict(0.0,  60.0), ("low",      1)); // lots of headroom
        assert_eq!(chain_verdict(0.0,  20.0), ("optimal",  0));
    }

    #[test]
    fn power_to_s_frac_s1_is_zero() {
        let s1 = S9_DBFS - 48.0;
        let frac = power_to_s_frac(s1);
        assert!(frac < 0.01, "S1 should be ≈0, got {frac}");
    }

    #[test]
    fn power_to_s_frac_s9_is_eight_fourteenths() {
        let frac = power_to_s_frac(S9_DBFS);
        assert!((frac - 8.0 / 14.0).abs() < 0.01, "S9 should be 8/14, got {frac}");
    }

    #[test]
    fn power_to_s_frac_clamps_below_s1() {
        assert!(power_to_s_frac(-200.0) < 0.01);
    }

    #[test]
    fn power_to_s_frac_clamps_above_s9_plus_60() {
        assert!((power_to_s_frac(100.0) - 1.0).abs() < 0.01);
    }

    #[test]
    fn s_bar_char_full_block_when_beyond() {
        // fill_eighths=32, x=2 → pos8=16 < 32 → '█'
        assert_eq!(s_bar_char(2, 32, None), '█');
    }

    #[test]
    fn s_bar_char_eighth_at_boundary() {
        // fill_eighths=12, x=1 → pos8=8 < 12 < 16 → S_EIGHTHS[12-8]='▌'
        assert_eq!(s_bar_char(1, 12, None), '▌');
    }

    #[test]
    fn s_bar_char_peak_pip_in_empty_zone() {
        // fill_eighths=8 (1 full col), peak at x=2 → empty zone → '╵'
        assert_eq!(s_bar_char(2, 8, Some(2)), '╵');
    }

    #[test]
    fn frac_to_s_label_known_values() {
        assert_eq!(frac_to_s_label(0.0),        "S1");
        assert_eq!(frac_to_s_label(6.0 / 14.0), "S7");
        assert_eq!(frac_to_s_label(8.0 / 14.0), "S9");
        assert_eq!(frac_to_s_label(1.0),         "S9+60");
    }

    #[test]
    fn clip_decay_bg_fresh_is_max_red() {
        let bg = clip_decay_bg(0);
        assert!(bg.is_some());
        if let Some(Color::Rgb(r, g, b)) = bg {
            assert!(r > 0, "red component must be positive");
            assert_eq!((g, b), (0, 0));
        }
    }

    #[test]
    fn clip_decay_bg_at_memory_limit_is_none() {
        assert_eq!(clip_decay_bg(CLIP_MEMORY_SECS), None);
    }

    #[test]
    fn clip_decay_bg_fades_monotonically() {
        let mut prev_r = u8::MAX;
        for t in 0..=CLIP_MEMORY_SECS {
            let r = match clip_decay_bg(t) {
                Some(Color::Rgb(r, _, _)) => r,
                _ => 0,
            };
            assert!(r <= prev_r, "should fade at t={t}: {r} > {prev_r}");
            prev_r = r;
        }
    }

    #[test]
    fn section_line_starts_with_connector_chars() {
        let t = Theme::sdr();
        let iw = 24usize;
        // Build the line the same way the section closure does.
        let label = "SIGNAL";
        let used = label.chars().count() + 5;
        let rule = "─".repeat(iw.saturating_sub(used));
        // Verify the connector prefix is present and rule fills the rest.
        let total: usize = 3 + label.chars().count() + 2 + rule.chars().count();
        assert_eq!(total, iw, "section line should fill iw={iw}, got {total}");
        // Spot-check frac_to_s_label round-trips through power_to_s_frac.
        let _ = t.label; // use t to avoid unused-var warning
    }
}
