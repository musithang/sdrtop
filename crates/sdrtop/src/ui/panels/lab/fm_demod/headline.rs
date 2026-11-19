// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The status headline: the two or three rows above the first section, saying
//! whether anything is being demodulated and, if so, what and where.
//!
//! Three states, and they have to read differently: RX stopped, locked, and idle.
//! A lock whose readings are held after a fade, waiting to age out, says so on
//! its own lamp rather than being a fourth state. Only the locked one carries
//! advisories - sentences about something the reader
//! can act on, which is why they wrap rather than clip.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::signal::demod::FILTER_QUALITY_D_LIMIT;
use crate::state::{Modulation, SdrMetrics};
use crate::ui::chrome;

use super::fmt::lbl;
use super::stack::Stack;

/// Rows a headline advisory may wrap to. Two holds the longest of them at the
/// panel's declared minimum width; a third would cost a section row to say the
/// same thing.
const ADVISORY_MAX_ROWS: usize = 2;

/// Push the headline rows. `locked` means some demodulator has a reading that
/// has not aged out; `modulation` is the *effective* mode, the one the sections
/// below are chosen by. `advise` leaves the advisories out, for the probe that
/// sizes the MPX trace: an advisory coming and going must not resize it.
#[allow(clippy::too_many_arguments)]
pub(super) fn lines(
    stack: &mut Stack<'static>,
    state: &SdrMetrics,
    modulation: Modulation,
    locked: bool,
    stale: bool,
    advise: bool,
    iw: usize,
    theme: &crate::Theme,
) {
    let dim = Style::default().fg(theme.stale);

    if stale {
        stack.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("\u{25cb} IDLE \u{2014} RX stopped", dim),
        ]));
    } else if locked {
        // A forced mode is marked, so a reading is never mistaken for the
        // classifier's own conclusion.
        let src = if state.demod.mode_override.is_some() {
            " \u{2731}"
        } else {
            ""
        };
        // Under the presence gate the last readings are held until they age
        // out, and the lamp says so: they are no longer a lock, and how long
        // they have been held is part of what they are.
        let lamp = match state.demod.held_for() {
            None => Span::styled(
                format!("\u{25cf} DEMOD LOCK \u{2014} {}{}", modulation.label(), src),
                Style::default()
                    .fg(theme.status_ok)
                    .add_modifier(Modifier::BOLD),
            ),
            Some(age) => Span::styled(
                format!(
                    "\u{25cc} DEMOD HELD \u{2014} {}{} {:.1} s",
                    modulation.label(),
                    src,
                    age.as_secs_f32()
                ),
                Style::default()
                    .fg(theme.stale)
                    .add_modifier(Modifier::BOLD),
            ),
        };
        stack.push(Line::from(vec![Span::raw(" "), lamp]));
        let d = state.demod.decimation.max(1);
        // The absolute frequency actually being demodulated - with an offset
        // in play this is not the tuned frequency, and must never be implied.
        let demod_hz = state.radio.frequency as i64 + state.demod.offset_hz;
        let off = state.demod.offset_hz;
        let off_str = if off == 0 {
            "centre".to_string()
        } else {
            format!("{:+.0} kHz", off as f64 / 1000.0)
        };
        stack.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("{:.3} MHz ", demod_hz as f64 / 1e6),
                Style::default().fg(theme.value_hi),
            ),
            Span::styled(off_str, lbl(theme)),
        ]));
        // The chain's own settings, not a measurement - the first thing the
        // headline can spare.
        stack.minor(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!(
                    "{:.0} kHz channel \u{00b7} \u{00f7}{}",
                    state.demod.channel_rate_hz / 1000.0,
                    d
                ),
                lbl(theme),
            ),
        ]));
        if advise {
            advisories(stack, state, d, iw, theme);
        }
    } else {
        // The *effective* mode, not the classifier's raw guess: the sections
        // below are chosen by it, and passing the raw one meant forcing AM with
        // [T] on an unclassified carrier printed "Tune to a broadcast station"
        // directly above a DEPTH / CARRIER pair.
        let (mark, headline, detail) = idle_status(modulation, state.demod.user_on);
        stack.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("{mark} {headline}"),
                Style::default()
                    .fg(theme.stale)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        stack.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(detail, lbl(theme)),
        ]));
    }
}

/// The things worth telling the reader about a lock that is working but could
/// work better. Advisories are sentences, so they wrap rather than being clipped
/// mid-word the way the DC-spike line was at 46 columns. Each is `Detail`:
/// useful, and the first thing a short panel can spare.
fn advisories(
    stack: &mut Stack<'static>,
    state: &SdrMetrics,
    d: usize,
    iw: usize,
    theme: &crate::Theme,
) {
    let mut advise = |text: String| {
        for row in chrome::wrap(&text, iw.saturating_sub(1), ADVISORY_MAX_ROWS) {
            stack.detail(Line::from(vec![
                Span::raw(" "),
                Span::styled(row, Style::default().fg(theme.status_warn)),
            ]));
        }
    };
    // Sitting on the LO means sharing the channel with the front end's DC
    // offset / LO leakage. Point at the fix that works there: the DC block
    // where the LO is the centre, an offset where it is not (the DC block
    // removes the 0 Hz tone, not a leak beside it).
    let lo = state.radio.lo_offset_hz;
    if state.demod.on_dc_spike(state.iq.cal.correcting(), lo) {
        advise(if lo == 0 {
            "\u{2192} on DC spike \u{2014} [D] in Lab IQ, or offset".to_string()
        } else {
            format!(
                "\u{2192} on the LO's leak at {:+.1} kHz \u{2014} offset the channel",
                lo as f64 / 1e3
            )
        });
    }
    // Blocks lost on the way here. RDS and CTCSS both need unbroken runs, so
    // without this line a busy host and a station with no RDS look identical:
    // the panel simply never decodes anything and never says why.
    if let Some(n) = state.demod.dropping() {
        advise(format!(
            "\u{2192} {n} block{} dropped \u{2014} RDS/CTCSS need a clean run",
            if n == 1 { "" } else { "s" }
        ));
    }
    // The channel filter stops sharpening once the decimation factor
    // saturates the tap budget - advise, never coerce, in the house style.
    if d > FILTER_QUALITY_D_LIMIT {
        advise("\u{2192} 2\u{2013}2.4 Msps sharpens this".to_string());
    }
}

/// The idle headline: `(mark, headline, detail)`. Every branch is dim/neutral -
/// an idle demod isn't a fault, the same framing `signal_characterization` uses
/// for its own "IDLE — RX stopped".
fn idle_status(
    modulation: Modulation,
    user_on: bool,
) -> (&'static str, &'static str, &'static str) {
    if !user_on {
        (
            "\u{25cb}",
            "DEMOD OFF",
            "Press [Space] in demod focus to start measuring.",
        )
    } else if matches!(modulation, Modulation::Am) {
        (
            "\u{25cb}",
            "DEMOD IDLE",
            "AM carrier \u{2014} FM deviation does not apply here.",
        )
    } else if modulation.is_known() {
        (
            "\u{25cb}",
            "DEMOD IDLE",
            "Carrier detected \u{2014} waiting for a usable channel.",
        )
    } else {
        (
            "\u{25cb}",
            "NO SIGNAL",
            "Tune to a broadcast station and centre it to characterize.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Switched off outranks the classifier - otherwise the panel would blame
        // the signal for a state the user chose.
        let (_, headline, _) = idle_status(Modulation::Wfm, false);
        assert_eq!(headline, "DEMOD OFF");
    }

    #[test]
    fn am_idle_detail_explains_why_fm_deviation_is_absent() {
        let (_, _, detail) = idle_status(Modulation::Am, true);
        assert!(detail.contains("AM"), "detail = {detail}");
    }
}
