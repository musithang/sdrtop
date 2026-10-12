// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The S-meter: an S1..S9+60 bar under the frequency hero.
//!
//! S-units are a radio convention, not a linear dB scale: S1 to S9 is 6 dB per
//! unit, and everything above S9 is quoted as "S9 + n dB". The bar therefore
//! spends 8/14 of its width on S1..S9 and the rest on the +60 overshoot.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

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
    let val_str = format!("~{power_dbfs:.1} dBFS");
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
