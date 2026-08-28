//! `MPX BASEBAND` - the 0-60 kHz composite the discriminator recovers, drawn as
//! a braille trace with a tick row naming the three carriers inside it.
//!
//! This is the one section whose height is a free choice, so the constants the
//! panel sizes it with live here too.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::signal::demod::{MPX_SPAN_HZ, PILOT_HZ};
use crate::state::{MpxFrame, SdrMetrics};
use crate::ui::chrome;

use super::fmt::lbl;
use super::stack::Stack;

/// Rows the MPX trace grows to when the panel has height going spare, and the slack
/// it refuses to spend getting there.
///
/// One braille row is four vertical levels over a 40 dB window - 10 dB a level, on
/// which the stereo pilot (~20 dB under the audio) is level 2 of 4 and indistinct
/// from the hump beside it. Three rows make it 3.3 dB a level. The reserve stops a
/// panel with barely enough slack from trading all its air for trace resolution.
pub(super) const MPX_TRACE_MAX_ROWS: usize = 3;
pub(super) const MPX_TRACE_SLACK_RESERVE: usize = 4;

/// Dynamic range shown in the MPX trace, below the loudest component.
const MPX_DISPLAY_RANGE_DB: f32 = 40.0;

/// Push the section: nameplate, then the trace at the height the caller sized,
/// then the tick row.
pub(super) fn lines(
    stack: &mut Stack<'static>,
    state: &SdrMetrics,
    iw: usize,
    trace_rows: usize,
    theme: &crate::Theme,
) {
    stack.heading(chrome::section("MPX BASEBAND", "0-60 kHz", iw, theme));
    match state.demod.live_mpx() {
        Some(frame) => {
            let w = iw.saturating_sub(2);
            let profile = mpx_profile(frame, w * 2);
            if w >= 8 && !profile.is_empty() {
                // The trace is one block at whatever height it was given, so the
                // rows go on together: the caller sizes it, the shedding pass
                // must not take a slice out of the middle of a picture.
                for row in crate::ui::widgets::charts::braille_profile(&profile, w, trace_rows) {
                    stack.push(Line::from(vec![
                        Span::raw(" "),
                        Span::styled(row, Style::default().fg(theme.border_accent)),
                    ]));
                }
                // The tick row is the first thing to go: a trace without its
                // 19k/38k/57k marks is still a shape, marks without a trace are
                // nothing at all.
                stack.ornament(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(mpx_ticks(w), lbl(theme)),
                ]));
            }
        }
        None => stack.gap(),
    }
}

/// Resample an MPX spectrum onto exactly `points` display columns spanning
/// 0..[`MPX_SPAN_HZ`], in dB.
///
/// Each column takes the **maximum** of the bins it covers, not their mean: the
/// pilot is a single narrow line, and averaging would bury it under the wideband
/// audio around it - the display would then disagree with the pilot readout right
/// beneath it.
fn mpx_profile(frame: &MpxFrame, points: usize) -> Vec<f32> {
    if points == 0 || frame.bin_hz <= 0.0 || frame.mags_hz.is_empty() {
        return Vec::new();
    }
    let last = (MPX_SPAN_HZ / frame.bin_hz).ceil() as usize;
    let last = last.min(frame.mags_hz.len());
    if last == 0 {
        return Vec::new();
    }

    let mut profile: Vec<f32> = (0..points)
        .map(|i| {
            let lo = i * last / points;
            let hi = (((i + 1) * last / points).max(lo + 1)).min(last);
            let peak = frame.mags_hz[lo..hi].iter().copied().fold(0.0f32, f32::max);
            if peak > 0.0 {
                20.0 * peak.log10()
            } else {
                -120.0
            }
        })
        .collect();

    // Clamp to a fixed window below the loudest component. A single braille row
    // has only four vertical levels, so letting the scale stretch down to the
    // noise floor squashes the whole MPX structure into the bottom level - the
    // pilot, the very thing the section is about, becomes invisible. Anchoring the
    // floor a fixed distance below the peak spends those four levels on signal.
    let top = profile.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if top.is_finite() {
        let floor = top - MPX_DISPLAY_RANGE_DB;
        for v in profile.iter_mut() {
            *v = v.max(floor);
        }
    }
    profile
}

/// Tick row under the MPX trace, marking the pilot, the stereo subcarrier and RDS
/// at their true positions in the span.
fn mpx_ticks(width: usize) -> String {
    let mut row = vec![b' '; width];
    for (hz, label) in [(PILOT_HZ, "19k"), (38_000.0, "38k"), (57_000.0, "57k")] {
        let pos = ((hz / MPX_SPAN_HZ) * width as f64).round() as usize;
        // Centre the label on the tick, keeping it inside the row.
        let start = pos
            .saturating_sub(label.len() / 2)
            .min(width.saturating_sub(label.len()));
        if start + label.len() <= width {
            row[start..start + label.len()].copy_from_slice(label.as_bytes());
        }
    }
    String::from_utf8(row).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_with_pilot() -> MpxFrame {
        // 163 Hz bins; a tall line at 19 kHz over a quiet floor.
        let bin_hz = 163.0;
        let mut mags = vec![1.0f32; 512];
        mags[(PILOT_HZ / bin_hz).round() as usize] = 7_500.0;
        MpxFrame {
            bin_hz,
            mags_hz: mags,
        }
    }

    #[test]
    fn mpx_profile_keeps_the_pilot_line_visible() {
        let f = frame_with_pilot();
        let p = mpx_profile(&f, 64);
        assert_eq!(p.len(), 64);
        // The pilot column must stand clear of the floor: taking the max (not the
        // mean) of each column's bins is what preserves a one-bin line.
        let top = p.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let bottom = p.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            top - bottom > 20.0,
            "pilot should tower over the floor: {top} vs {bottom}"
        );
        // The pilot sits at 19/60 of the span.
        let idx = p
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        let expect = (PILOT_HZ / MPX_SPAN_HZ * 64.0) as usize;
        assert!(
            (idx as i64 - expect as i64).abs() <= 1,
            "pilot at column {idx}, expected {expect}"
        );
    }

    #[test]
    fn mpx_profile_floor_is_clamped_to_the_display_range() {
        let f = frame_with_pilot();
        let p = mpx_profile(&f, 64);
        let top = p.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let bottom = p.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            (top - bottom - MPX_DISPLAY_RANGE_DB).abs() < 0.01,
            "range should be exactly {MPX_DISPLAY_RANGE_DB} dB, got {}",
            top - bottom
        );
    }

    #[test]
    fn mpx_profile_declines_degenerate_frames() {
        let empty = MpxFrame {
            bin_hz: 163.0,
            mags_hz: vec![],
        };
        assert!(mpx_profile(&empty, 32).is_empty());
        let bad_bin = MpxFrame {
            bin_hz: 0.0,
            mags_hz: vec![1.0; 100],
        };
        assert!(mpx_profile(&bad_bin, 32).is_empty());
        assert!(mpx_profile(&frame_with_pilot(), 0).is_empty());
    }

    #[test]
    fn mpx_ticks_place_labels_in_span_order() {
        let row = mpx_ticks(48);
        assert_eq!(row.chars().count(), 48);
        let p19 = row.find("19k").expect("19k tick");
        let p38 = row.find("38k").expect("38k tick");
        let p57 = row.find("57k").expect("57k tick");
        assert!(
            p19 < p38 && p38 < p57,
            "ticks out of order: {p19} {p38} {p57}"
        );
        // 19 kHz of a 60 kHz span sits near a third across.
        assert!((p19 as f64 - 48.0 * 19.0 / 60.0).abs() < 3.0);
    }

    #[test]
    fn mpx_ticks_survive_a_narrow_panel() {
        // Labels must never overflow the row, however little width there is.
        for w in [0usize, 1, 3, 8, 20] {
            assert_eq!(mpx_ticks(w).chars().count(), w, "width {w}");
        }
    }
}
