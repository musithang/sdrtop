//! `fm_demod` — the right column of the `lab_signal` preset's redesign
//! (DSN-2026-07): the FM MPX · DEMOD instrument.
//!
//! Phase 2 fills the DEVIATION section from the live FM discriminator
//! (`signal::demod`): peak and RMS deviation plus carrier offset, measured about
//! the carrier so a mistuned radio reports tuning error as offset rather than as
//! modulation. The MPX / PILOT / RDS / AUDIO sections stay declared-but-empty
//! nameplates until their phases land; NFM and AM already select their own
//! deviation reference through [`deviation_limit_hz`], and the wider per-mode
//! dispatch slots into the same `match` later.
//!
//! The panel never invents a reading: when the demod is off, the signal is too
//! weak, the modulation is not something an FM discriminator describes, or the
//! last measurement has aged out, it falls back to the neutral idle headline.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::signal::demod::{FILTER_QUALITY_D_LIMIT, MPX_SPAN_HZ, PILOT_HZ};
use crate::state::{deviation_limit_hz, FmMeasure, Modulation, MpxFrame, PilotState, SdrMetrics};
use crate::ui::chrome;
use crate::ui::micro_common::bar_spans;
use crate::ui::panel::Panel;

pub struct FmDemodPanel;

/// One zone of the demod panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sec { Mpx, Pilot, Deviation, Ctcss, Depth, Carrier, Rds, Audio }

/// The sections a given modulation's panel shows.
///
/// This is the single dispatch the panel was designed around. MPX, the stereo
/// pilot and RDS are wide-band FM concepts that simply do not exist for NFM or
/// AM, and FM deviation is meaningless for an amplitude-modulated carrier — so
/// each mode is shown only what it actually has, rather than a fixed grid of
/// sections where most read as permanently empty.
fn sections_for(m: Modulation) -> &'static [Sec] {
    match m {
        Modulation::Nfm => &[Sec::Deviation, Sec::Ctcss, Sec::Audio],
        Modulation::Am  => &[Sec::Depth, Sec::Carrier, Sec::Audio],
        // An unclassified carrier keeps the broadcast shape — the state the panel
        // rests in before anything is tuned.
        Modulation::Wfm | Modulation::Unknown =>
            &[Sec::Mpx, Sec::Pilot, Sec::Deviation, Sec::Rds, Sec::Audio],
    }
}

/// Resample an MPX spectrum onto exactly `points` display columns spanning
/// 0..[`MPX_SPAN_HZ`], in dB.
///
/// Each column takes the **maximum** of the bins it covers, not their mean: the
/// pilot is a single narrow line, and averaging would bury it under the wideband
/// audio around it — the display would then disagree with the pilot readout right
/// beneath it.
fn mpx_profile(frame: &MpxFrame, points: usize) -> Vec<f32> {
    if points == 0 || frame.bin_hz <= 0.0 || frame.mags_hz.is_empty() { return Vec::new(); }
    let last = (MPX_SPAN_HZ / frame.bin_hz).ceil() as usize;
    let last = last.min(frame.mags_hz.len());
    if last == 0 { return Vec::new(); }

    let mut profile: Vec<f32> = (0..points)
        .map(|i| {
            let lo = i * last / points;
            let hi = (((i + 1) * last / points).max(lo + 1)).min(last);
            let peak = frame.mags_hz[lo..hi].iter().copied().fold(0.0f32, f32::max);
            if peak > 0.0 { 20.0 * peak.log10() } else { -120.0 }
        })
        .collect();

    // Clamp to a fixed window below the loudest component. A single braille row
    // has only four vertical levels, so letting the scale stretch down to the
    // noise floor squashes the whole MPX structure into the bottom level — the
    // pilot, the very thing the section is about, becomes invisible. Anchoring the
    // floor a fixed distance below the peak spends those four levels on signal.
    let top = profile.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if top.is_finite() {
        let floor = top - MPX_DISPLAY_RANGE_DB;
        for v in profile.iter_mut() { *v = v.max(floor); }
    }
    profile
}

/// Dynamic range shown in the MPX trace, below the loudest component.
const MPX_DISPLAY_RANGE_DB: f32 = 40.0;

/// Tick row under the MPX trace, marking the pilot, the stereo subcarrier and RDS
/// at their true positions in the span.
fn mpx_ticks(width: usize) -> String {
    let mut row = vec![b' '; width];
    for (hz, label) in [(PILOT_HZ, "19k"), (38_000.0, "38k"), (57_000.0, "57k")] {
        let pos = ((hz / MPX_SPAN_HZ) * width as f64).round() as usize;
        // Centre the label on the tick, keeping it inside the row.
        let start = pos.saturating_sub(label.len() / 2).min(width.saturating_sub(label.len()));
        if start + label.len() <= width {
            row[start..start + label.len()].copy_from_slice(label.as_bytes());
        }
    }
    String::from_utf8(row).unwrap_or_default()
}

/// The idle headline: `(mark, headline, detail)`. Every branch is dim/neutral —
/// an idle demod isn't a fault, the same framing `signal_characterization` uses
/// for its own "IDLE — RX stopped".
fn idle_status(modulation: Modulation, user_on: bool) -> (&'static str, &'static str, &'static str) {
    if !user_on {
        ("\u{25cb}", "DEMOD OFF",
         "Press [Space] in demod focus to start measuring.")
    } else if matches!(modulation, Modulation::Am) {
        ("\u{25cb}", "DEMOD IDLE",
         "AM carrier \u{2014} FM deviation does not apply here.")
    } else if modulation.is_known() {
        ("\u{25cb}", "DEMOD IDLE",
         "Carrier detected \u{2014} waiting for a usable channel.")
    } else {
        ("\u{25cb}", "NO SIGNAL",
         "Tune to a broadcast station and centre it to characterize.")
    }
}

/// Frequency in the most readable unit for a deviation / offset figure. Keeps
/// three significant figures across the WFM (tens of kHz) and NFM (single kHz)
/// ranges without ever padding a number with false precision.
fn fmt_hz(hz: f32) -> String {
    let a = hz.abs();
    if a >= 10_000.0      { format!("{:.1} kHz", hz / 1000.0) }
    else if a >= 1_000.0  { format!("{:.2} kHz", hz / 1000.0) }
    else                  { format!("{:.0} Hz", hz) }
}

/// Signed variant for the carrier offset, where the sign is the point.
fn fmt_offset(hz: f32) -> String {
    let sign = if hz >= 0.0 { "+" } else { "\u{2212}" };
    format!("{}{}", sign, fmt_hz(hz.abs()))
}

/// Colour for a deviation reading against its nominal limit: over the limit is
/// a real transmitter fault, and just under it is worth flagging.
fn deviation_color(ratio: f32, theme: &crate::Theme) -> ratatui::style::Color {
    if ratio >= 1.0      { theme.status_crit }
    else if ratio >= 0.9 { theme.status_warn }
    else                 { theme.status_ok }
}

/// Colour for an AM depth reading, graded on the **negative** peak.
///
/// Positive over-modulation merely runs hot; a negative peak reaching 100 %
/// pinches the carrier off entirely, which clips the envelope and splatters into
/// the adjacent channel. That is the failure worth colouring for.
fn depth_color(negative_pct: f32, theme: &crate::Theme) -> ratatui::style::Color {
    if negative_pct >= 100.0     { theme.status_crit }
    else if negative_pct >= 90.0 { theme.status_warn }
    else                         { theme.status_ok }
}

impl Panel for FmDemodPanel {
    fn name(&self) -> &'static str { "fm_demod" }
    fn min_size(&self) -> (u16, u16) { (28, 12) }

    fn focus_key(&self) -> Option<char> { Some('m') }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("Space", "Demod on/off"), ("←/→", "Channel offset"), ("P", "Snap to carrier"),
          ("0", "Centre"), ("T", "Mode"), ("C", "Snapshot to log")]
    }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        let stale = !state.radio.hw_streaming;
        let name_style = Style::default().fg(theme.label).add_modifier(Modifier::BOLD);
        let mut title = vec![Span::raw(" "), Span::styled("FM MPX \u{00b7} Demod", name_style)];
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
        let dim = Style::default().fg(theme.stale);
        let lbl = Style::default().fg(theme.label);
        let val = Style::default().fg(theme.value);
        let mut lines: Vec<Line> = Vec::new();

        // `live()` already refuses an aged-out reading, so a frozen number can
        // never be mistaken for a current one.
        let modulation = state.demod.effective_modulation(state.signal.modulation);
        let measure = if stale { None } else { state.demod.live() };
        let am = if stale { None } else { state.demod.live_am() };
        // One demodulator runs at a time, so a lock is whichever produced a value.
        let locked = measure.is_some() || am.is_some();

        // ── Status headline ────────────────────────────────────────────────
        if stale {
            lines.push(Line::from(vec![Span::raw(" "), Span::styled("\u{25cb} IDLE \u{2014} RX stopped", dim)]));
        } else if locked {
            // A forced mode is marked, so a reading is never mistaken for the
            // classifier's own conclusion.
            let src = if state.demod.mode_override.is_some() { " \u{2731}" } else { "" };
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("\u{25cf} DEMOD LOCK \u{2014} {}{}", modulation.label(), src),
                             Style::default().fg(theme.status_ok).add_modifier(Modifier::BOLD)),
            ]));
            let d = state.demod.decimation.max(1);
            // The absolute frequency actually being demodulated — with an offset
            // in play this is not the tuned frequency, and must never be implied.
            let demod_hz = state.radio.frequency as i64 + state.demod.offset_hz;
            let off = state.demod.offset_hz;
            let off_str = if off == 0 { "centre".to_string() }
                          else { format!("{:+.0} kHz", off as f64 / 1000.0) };
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{:.3} MHz ", demod_hz as f64 / 1e6),
                             Style::default().fg(theme.value_hi)),
                Span::styled(off_str, lbl),
            ]));
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("{:.0} kHz channel \u{00b7} \u{00f7}{}", state.demod.channel_rate_hz / 1000.0, d),
                    lbl),
            ]));
            // Sitting on the tuned centre means sharing the channel with the
            // front-end's DC offset / LO leakage. Point at the existing fix.
            if state.demod.on_dc_spike(state.iq.cal.correcting()) {
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled("\u{2192} on DC spike \u{2014} [D] in Lab IQ, or offset",
                                 Style::default().fg(theme.status_warn)),
                ]));
            }
            // The channel filter stops sharpening once the decimation factor
            // saturates the tap budget — advise, never coerce, in the house style.
            if d > FILTER_QUALITY_D_LIMIT {
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled("\u{2192} 2\u{2013}2.4 Msps sharpens this", Style::default().fg(theme.status_warn)),
                ]));
            }
        } else {
            let (mark, headline, detail) = idle_status(state.signal.modulation, state.demod.user_on);
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{mark} {headline}"), Style::default().fg(theme.stale).add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(vec![Span::raw(" "), Span::styled(detail, lbl)]));
        }
        lines.push(Line::raw(""));

        // ── Sections, chosen by modulation ─────────────────────────────────
        for sec in sections_for(modulation) {
            match sec {
            Sec::Mpx => {
        lines.push(chrome::section("MPX BASEBAND", "0-60 kHz", iw, theme));
        match state.demod.live_mpx() {
            Some(frame) => {
                let w = iw.saturating_sub(2);
                let profile = mpx_profile(frame, w * 2);
                if w >= 8 && !profile.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw(" "),
                        Span::styled(crate::ui::charts::mini_braille_line(&profile, w),
                                     Style::default().fg(theme.border_accent)),
                    ]));
                    lines.push(Line::from(vec![
                        Span::raw(" "),
                        Span::styled(mpx_ticks(w), lbl),
                    ]));
                }
            }
            None => lines.push(Line::raw("")),
        }
            }
            Sec::Pilot => {
        lines.push(chrome::section("PILOT / STEREO", "19 kHz", iw, theme));
        match state.demod.live_pilot() {
            Some(p) => {
                let (mark, word, color) = match p.state {
                    PilotState::Locked   => ("\u{25cf}", "STEREO",   theme.status_ok),
                    PilotState::Marginal => ("\u{25d0}", "MARGINAL", theme.status_warn),
                    PilotState::Absent   => ("\u{25cb}", "MONO",     theme.label),
                };
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(format!("{mark} {word}"),
                                 Style::default().fg(color).add_modifier(Modifier::BOLD)),
                ]));
                if p.state != PilotState::Absent {
                    lines.push(Line::from(vec![
                        chrome::field("Pilot", 8, theme),
                        Span::styled(fmt_hz(p.deviation_hz), val),
                    ]));
                    lines.push(Line::from(vec![
                        chrome::field("Inject", 8, theme),
                        Span::styled(format!("{:.1}%", p.injection_pct), val),
                    ]));
                }
            }
            None => lines.push(Line::raw("")),
        }
            }
            Sec::Deviation => {
        let limit = deviation_limit_hz(modulation);
        let hint = format!("{:.0} kHz max", limit / 1000.0);
        lines.push(chrome::section("DEVIATION", &hint, iw, theme));
        match measure {
            Some(FmMeasure { peak_dev_hz, rms_dev_hz, carrier_offset_hz }) => {
                let ratio = if limit > 0.0 { peak_dev_hz / limit } else { 0.0 };
                let color = deviation_color(ratio, theme);
                lines.push(Line::from(vec![
                    chrome::field("Peak", 8, theme),
                    Span::styled(fmt_hz(peak_dev_hz), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                ]));
                // Bar width leaves room for the leading space and the trailing "%".
                let bar_w = iw.saturating_sub(9).min(14);
                if bar_w >= 4 {
                    let mut row = vec![Span::raw(" ")];
                    row.extend(bar_spans(ratio.clamp(0.0, 1.0) as f64, bar_w, color, theme));
                    row.push(Span::styled(format!(" {:.0}%", (ratio * 100.0).min(999.0)), lbl));
                    lines.push(Line::from(row));
                }
                lines.push(Line::from(vec![
                    chrome::field("RMS", 8, theme),
                    Span::styled(fmt_hz(rms_dev_hz), val),
                ]));
                lines.push(Line::from(vec![
                    chrome::field("Offset", 8, theme),
                    Span::styled(fmt_offset(carrier_offset_hz), val),
                ]));
            }
            None => lines.push(Line::raw("")),
        }
            }
            Sec::Ctcss => {
                lines.push(chrome::section("CTCSS", "subaudible", iw, theme));
                match state.demod.live_ctcss() {
                    Some(t) => {
                        lines.push(Line::from(vec![
                            Span::raw(" "),
                            Span::styled(format!("\u{25cf} {:.1} Hz", t.tone_hz),
                                         Style::default().fg(theme.status_ok).add_modifier(Modifier::BOLD)),
                        ]));
                        lines.push(Line::from(vec![
                            chrome::field("Dev", 8, theme),
                            Span::styled(fmt_hz(t.deviation_hz), val),
                        ]));
                        lines.push(Line::from(vec![
                            chrome::field("Margin", 8, theme),
                            Span::styled(format!("{:.0} dB", t.margin_db), val),
                        ]));
                    }
                    // "Still filling the window" and "there is no tone" look the
                    // same on screen unless they are said differently — and only
                    // one of them is a finding.
                    None if state.demod.ctcss_searching() => {
                        lines.push(Line::from(vec![
                            Span::raw(" "),
                            Span::styled(format!("\u{25cc} SEARCHING {:.0}%",
                                                 state.demod.ctcss_fill * 100.0), lbl),
                        ]));
                    }
                    None if !stale => {
                        lines.push(Line::from(vec![
                            Span::raw(" "),
                            Span::styled("\u{25cb} NO TONE", lbl),
                        ]));
                    }
                    None => lines.push(Line::raw("")),
                }
            }
            Sec::Depth => {
                lines.push(chrome::section("DEPTH", "100% max", iw, theme));
                match am {
                    Some(a) => {
                        let ratio = a.depth_pct / 100.0;
                        let color = depth_color(a.negative_pct, theme);
                        lines.push(Line::from(vec![
                            chrome::field("Depth", 8, theme),
                            Span::styled(format!("{:.0}%", a.depth_pct),
                                         Style::default().fg(color).add_modifier(Modifier::BOLD)),
                        ]));
                        let bar_w = iw.saturating_sub(9).min(14);
                        if bar_w >= 4 {
                            let mut row = vec![Span::raw(" ")];
                            row.extend(bar_spans(ratio.clamp(0.0, 1.0) as f64, bar_w, color, theme));
                            lines.push(Line::from(row));
                        }
                        // Split out because they fail differently: a negative peak
                        // reaching 100 % pinches the carrier off and splatters.
                        lines.push(Line::from(vec![
                            chrome::field("Pos", 8, theme),
                            Span::styled(format!("{:.0}%", a.positive_pct), val),
                        ]));
                        lines.push(Line::from(vec![
                            chrome::field("Neg", 8, theme),
                            Span::styled(format!("{:.0}%", a.negative_pct),
                                         Style::default().fg(depth_color(a.negative_pct, theme))),
                        ]));
                    }
                    None => lines.push(Line::raw("")),
                }
            }
            Sec::Carrier => {
                lines.push(chrome::section("CARRIER", "", iw, theme));
                match am {
                    Some(a) => lines.push(Line::from(vec![
                        chrome::field("Level", 8, theme),
                        Span::styled(format!("{:.1} dBFS", a.carrier_dbfs), val),
                    ])),
                    None => lines.push(Line::raw("")),
                }
            }
            Sec::Rds   => lines.push(chrome::section("RDS", "57 kHz", iw, theme)),
            Sec::Audio => lines.push(chrome::section("AUDIO", "", iw, theme)),
            }
            lines.push(Line::raw(""));
        }

        chrome::fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_name_is_stable() {
        assert_eq!(FmDemodPanel.name(), "fm_demod");
    }

    #[test]
    fn focus_key_is_unclaimed_by_other_panels() {
        // 'd' belongs to rf_chain; the demod bench takes 'm'.
        assert_eq!(FmDemodPanel.focus_key(), Some('m'));
    }

    #[test]
    fn idle_status_reads_no_signal_when_modulation_unknown() {
        let (_, headline, _) = idle_status(Modulation::Unknown, true);
        assert_eq!(headline, "NO SIGNAL");
    }

    #[test]
    fn idle_status_reads_demod_idle_when_modulation_known() {
        for m in [Modulation::Wfm, Modulation::Nfm, Modulation::Am] {
            let (_, headline, _) = idle_status(m, true);
            assert_eq!(headline, "DEMOD IDLE", "modulation={m:?}");
        }
    }

    #[test]
    fn idle_status_says_off_when_the_user_switched_it_off() {
        // Switched off outranks the classifier — otherwise the panel would blame
        // the signal for a state the user chose.
        let (_, headline, _) = idle_status(Modulation::Wfm, false);
        assert_eq!(headline, "DEMOD OFF");
    }

    #[test]
    fn am_idle_detail_explains_why_fm_deviation_is_absent() {
        let (_, _, detail) = idle_status(Modulation::Am, true);
        assert!(detail.contains("AM"), "detail = {detail}");
    }

    #[test]
    fn fmt_hz_picks_a_readable_unit() {
        assert_eq!(fmt_hz(42_300.0), "42.3 kHz");
        assert_eq!(fmt_hz(4_230.0),  "4.23 kHz");
        assert_eq!(fmt_hz(420.0),    "420 Hz");
    }

    #[test]
    fn fmt_offset_always_carries_a_sign() {
        assert!(fmt_offset(1_200.0).starts_with('+'));
        assert!(fmt_offset(-1_200.0).starts_with('\u{2212}'));
        // Zero reads as a positive zero rather than a bare number.
        assert!(fmt_offset(0.0).starts_with('+'));
    }

    fn frame_with_pilot() -> MpxFrame {
        // 163 Hz bins; a tall line at 19 kHz over a quiet floor.
        let bin_hz = 163.0;
        let mut mags = vec![1.0f32; 512];
        mags[(PILOT_HZ / bin_hz).round() as usize] = 7_500.0;
        MpxFrame { bin_hz, mags_hz: mags }
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
        assert!(top - bottom > 20.0, "pilot should tower over the floor: {top} vs {bottom}");
        // The pilot sits at 19/60 of the span.
        let idx = p.iter().enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).map(|(i, _)| i).unwrap();
        let expect = (PILOT_HZ / MPX_SPAN_HZ * 64.0) as usize;
        assert!((idx as i64 - expect as i64).abs() <= 1, "pilot at column {idx}, expected {expect}");
    }

    #[test]
    fn mpx_profile_floor_is_clamped_to_the_display_range() {
        let f = frame_with_pilot();
        let p = mpx_profile(&f, 64);
        let top = p.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let bottom = p.iter().copied().fold(f32::INFINITY, f32::min);
        assert!((top - bottom - MPX_DISPLAY_RANGE_DB).abs() < 0.01,
                "range should be exactly {MPX_DISPLAY_RANGE_DB} dB, got {}", top - bottom);
    }

    #[test]
    fn mpx_profile_declines_degenerate_frames() {
        let empty = MpxFrame { bin_hz: 163.0, mags_hz: vec![] };
        assert!(mpx_profile(&empty, 32).is_empty());
        let bad_bin = MpxFrame { bin_hz: 0.0, mags_hz: vec![1.0; 100] };
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
        assert!(p19 < p38 && p38 < p57, "ticks out of order: {p19} {p38} {p57}");
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

    #[test]
    fn wfm_shows_the_broadcast_sections() {
        let s = sections_for(Modulation::Wfm);
        for want in [Sec::Mpx, Sec::Pilot, Sec::Deviation, Sec::Rds] {
            assert!(s.contains(&want), "WFM missing {want:?}");
        }
        // CTCSS and AM depth belong to other modes.
        assert!(!s.contains(&Sec::Ctcss));
        assert!(!s.contains(&Sec::Depth));
    }

    #[test]
    fn nfm_swaps_the_broadcast_sections_for_ctcss() {
        let s = sections_for(Modulation::Nfm);
        assert!(s.contains(&Sec::Deviation), "NFM still measures deviation");
        assert!(s.contains(&Sec::Ctcss));
        // These are wide-band FM concepts; showing them for NFM would be a lie.
        for absent in [Sec::Mpx, Sec::Pilot, Sec::Rds] {
            assert!(!s.contains(&absent), "NFM should not show {absent:?}");
        }
    }

    #[test]
    fn am_shows_depth_instead_of_deviation() {
        let s = sections_for(Modulation::Am);
        assert!(s.contains(&Sec::Depth));
        assert!(s.contains(&Sec::Carrier));
        // FM deviation is meaningless for an amplitude-modulated carrier.
        assert!(!s.contains(&Sec::Deviation));
        assert!(!s.contains(&Sec::Ctcss));
    }

    #[test]
    fn unknown_rests_in_the_broadcast_shape() {
        assert_eq!(sections_for(Modulation::Unknown), sections_for(Modulation::Wfm));
    }

    #[test]
    fn depth_color_grades_on_the_negative_peak() {
        let t = crate::Theme::sdr();
        assert_eq!(depth_color(60.0, &t), t.status_ok);
        assert_eq!(depth_color(95.0, &t), t.status_warn);
        // 100 % negative pinches the carrier off — clipping and splatter.
        assert_eq!(depth_color(100.0, &t), t.status_crit);
        assert_eq!(depth_color(130.0, &t), t.status_crit);
    }

    #[test]
    fn deviation_color_escalates_at_the_limit() {
        let t = crate::Theme::sdr();
        assert_eq!(deviation_color(0.5, &t), t.status_ok);
        assert_eq!(deviation_color(0.95, &t), t.status_warn);
        // At and above the nominal limit this is over-deviation — a real fault.
        assert_eq!(deviation_color(1.0, &t), t.status_crit);
        assert_eq!(deviation_color(1.4, &t), t.status_crit);
    }
}
