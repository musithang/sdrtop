//! The S-meter: an S1..S9+60 bar under the frequency hero, plus the clip
//! alert-memory line that fades under the SAT metric.
//!
//! S-units are a radio convention, not a linear dB scale: S1 to S9 is 6 dB per
//! unit, and everything above S9 is quoted as "S9 + n dB". The bar therefore
//! spends 8/14 of its width on S1..S9 and the rest on the +60 overshoot.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// How long a clip is remembered, and the window in which it's still "fresh"
/// (loud red) before it fades to a dim memory line.
const CLIP_FRESH_SECS: u64 = 6;

const CLIP_MEMORY_SECS: u64 = 30;

/// Compact relative age for the alert-memory: `"4s"`, `"2m"`, `"1h"`. Pure.
pub(super) fn fmt_since(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// The SAT clip alert-memory state: `Some((age_secs, fresh))` while a clip is
/// still remembered, `None` once it's older than [`CLIP_MEMORY_SECS`]. A fresh
/// clip (≤ [`CLIP_FRESH_SECS`]) renders loud; afterwards it fades. Pure over the
/// clock so it's testable, and it only ever fades — it never flickers.
pub(super) fn clip_alert(last_clip_at: Option<u64>, now: u64) -> Option<(u64, bool)> {
    let since = now.saturating_sub(last_clip_at?);
    (since <= CLIP_MEMORY_SECS).then_some((since, since <= CLIP_FRESH_SECS))
}

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
        1 => "S2",
        2 => "S3",
        3 => "S4",
        4 => "S5",
        5 => "S6",
        6 => "S7",
        7 => "S8",
        8..=9 => "S9",
        10..=11 => "S9+20",
        12..=13 => "S9+40",
        _ => "S9+60",
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
    let pos8 = x * 8;
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

#[allow(clippy::eq_op)]
const SCALE: &[(&str, f64)] = &[
    ("S1", 0.0 / 14.0),
    ("S3", 2.0 / 14.0),
    ("S5", 4.0 / 14.0),
    ("S7", 6.0 / 14.0),
    ("S9", 8.0 / 14.0),
    ("+20", 10.0 / 14.0),
    ("+40", 12.0 / 14.0),
    ("+60", 14.0 / 14.0),
];

pub(super) fn s_meter_lines(
    power_dbfs: f32,
    peak_dbfs: Option<f32>,
    iw: usize,
    theme: &crate::Theme,
) -> [Line<'static>; 3] {
    let bar_w = iw.saturating_sub(1).max(1);
    let frac = power_to_s_frac(power_dbfs);
    let fill_eighths = (frac * bar_w as f64 * 8.0) as usize;
    let peak_col = peak_dbfs.map(|p| (power_to_s_frac(p) * bar_w as f64) as usize);

    // Row 0: scale tick labels.
    let skip_alt = iw < 20;
    let mut scale_buf = vec![' '; bar_w];
    for (idx, &(lbl, frac_pos)) in SCALE.iter().enumerate() {
        if skip_alt && idx % 2 != 0 {
            continue;
        }
        let pos = (frac_pos * bar_w as f64) as usize;
        for (j, c) in lbl.chars().enumerate() {
            let col = pos + j;
            if col < bar_w {
                scale_buf[col] = c;
            }
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
        Span::styled(
            s_label.to_string(),
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ".to_string(), Style::default().fg(theme.border_dim)),
        Span::styled(val_str, Style::default().fg(theme.value)),
    ];
    if let Some(p) = peak_dbfs {
        let p_label = frac_to_s_label(power_to_s_frac(p));
        row2_spans.push(Span::styled(
            "  ·  ".to_string(),
            Style::default().fg(theme.border_dim),
        ));
        row2_spans.push(Span::styled(
            format!("peak {p_label}"),
            Style::default().fg(theme.label),
        ));
    }
    let row2 = Line::from(row2_spans);

    [row0, row1, row2]
}

pub(super) fn clip_decay_bg(since: u64) -> Option<Color> {
    if since > CLIP_MEMORY_SECS {
        return None;
    }
    let t = if since <= CLIP_FRESH_SECS {
        1.0_f64
    } else {
        1.0 - (since - CLIP_FRESH_SECS) as f64 / (CLIP_MEMORY_SECS - CLIP_FRESH_SECS) as f64
    };
    let r = (45.0 * t) as u8;
    if r == 0 {
        None
    } else {
        Some(Color::Rgb(r, 0, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_since_scales_units() {
        assert_eq!(fmt_since(4), "4s");
        assert_eq!(fmt_since(59), "59s");
        assert_eq!(fmt_since(120), "2m");
        assert_eq!(fmt_since(7200), "2h");
    }

    #[test]
    fn clip_alert_is_fresh_then_fades_then_expires() {
        assert_eq!(clip_alert(None, 100), None); // never clipped
        assert_eq!(clip_alert(Some(100), 103), Some((3, true))); // fresh & loud
        assert_eq!(clip_alert(Some(100), 115), Some((15, false))); // remembered, dim
        assert_eq!(clip_alert(Some(100), 140), None); // older than memory
                                                      // Clock skew (clip "in the future") must not panic or misread.
        assert_eq!(clip_alert(Some(100), 90), Some((0, true)));
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
        assert!(
            (frac - 8.0 / 14.0).abs() < 0.01,
            "S9 should be 8/14, got {frac}"
        );
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
        assert_eq!(frac_to_s_label(0.0), "S1");
        assert_eq!(frac_to_s_label(6.0 / 14.0), "S7");
        assert_eq!(frac_to_s_label(8.0 / 14.0), "S9");
        assert_eq!(frac_to_s_label(1.0), "S9+60");
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
}
