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

    (0..points)
        .map(|i| {
            let lo = i * last / points;
            let hi = (((i + 1) * last / points).max(lo + 1)).min(last);
            let peak = frame.mags_hz[lo..hi].iter().copied().fold(0.0f32, f32::max);
            if peak > 0.0 { 20.0 * peak.log10() } else { -120.0 }
        })
        .collect()
}

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

impl Panel for FmDemodPanel {
    fn name(&self) -> &'static str { "fm_demod" }
    fn min_size(&self) -> (u16, u16) { (28, 12) }

    fn focus_key(&self) -> Option<char> { Some('m') }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("Space", "Demod on/off"), ("←/→", "Channel offset"), ("P", "Snap to carrier"),
          ("0", "Centre"), ("C", "Snapshot to log")]
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
        let measure = if stale { None } else { state.demod.live() };

        // ── Status headline ────────────────────────────────────────────────
        if stale {
            lines.push(Line::from(vec![Span::raw(" "), Span::styled("\u{25cb} IDLE \u{2014} RX stopped", dim)]));
        } else if measure.is_some() {
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("\u{25cf} DEMOD LOCK \u{2014} {}", state.signal.modulation.label()),
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

        // ── MPX BASEBAND — live in Phase 3 ─────────────────────────────────
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
        lines.push(Line::raw(""));

        // ── PILOT / STEREO — live in Phase 3 ───────────────────────────────
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
        lines.push(Line::raw(""));

        // ── DEVIATION — live in Phase 2 ────────────────────────────────────
        let limit = deviation_limit_hz(state.signal.modulation);
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
        lines.push(Line::raw(""));

        // ── RDS / AUDIO: still declared-empty (Phases 4-5) ─────────────────
        for (name, hint) in [("RDS", "57 kHz"), ("AUDIO", "")] {
            lines.push(chrome::section(name, hint, iw, theme));
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
