//! `RDS` — the 57 kHz data subcarrier: the station's name, its identity codes,
//! and whatever RadioText it is sending.
//!
//! The one section here that *accumulates*. Everything else in the panel is a
//! measurement of the last window and goes away on its own; a decoded name will
//! sit on screen looking confident long after reception stopped unless it is
//! made to expire. That is what the age plumbing throughout this module is for.

use std::time::Duration;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::{RdsData, SdrMetrics, PTY_NAMES, RDS_AGED_AFTER, RDS_DROPPED_AFTER};
use crate::ui::chrome;

use super::fmt::val;
use super::stack::Stack;

/// Rows of RadioText the panel will show. Two rows hold the 64-character maximum
/// on any column wide enough to be readable; a third would push the sections below
/// off a short terminal for a field that is usually much shorter than its limit.
const RT_MAX_ROWS: usize = 2;

pub(super) fn lines(stack: &mut Stack<'static>, state: &SdrMetrics, iw: usize, theme: &crate::Theme) {
    stack.heading(chrome::section("RDS", "57 kHz", iw, theme));
    let Some(d) = state.demod.live_rds() else { stack.gap(); return };

    let age = state.demod.rds_age();
    // Past the drop timeout the accumulated text is not about anything currently
    // on air, so none of it is shown — not the name, not the code, not the message.
    let dropped = age.is_none_or(|a| a > RDS_DROPPED_AFTER);
    let (mark, text, ok) = rds_headline(d, state.demod.rds_sync, age);
    let color = if ok { theme.status_ok } else { theme.label };
    stack.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(format!("{mark} {text}"),
                     Style::default().fg(color).add_modifier(Modifier::BOLD)),
    ]));
    if let Some(pi) = d.pi.filter(|_| !dropped) {
        stack.detail(Line::from(vec![
            chrome::field("PI", 8, theme),
            Span::styled(format!("{pi:04X}"), val(theme)),
        ]));
    }
    if let Some(name) = d.pty.filter(|_| !dropped)
        .map(|p| PTY_NAMES[(p & 0x1F) as usize]) {
        stack.detail(Line::from(vec![
            chrome::field("PTY", 8, theme),
            Span::styled(name, val(theme)),
        ]));
    }
    // Traffic flags only earn a row when one of them is set —
    // "TP off, TA off" is the normal case and says nothing.
    if (d.tp || d.ta) && !dropped {
        let mut flags = Vec::new();
        if d.tp { flags.push("TP"); }
        if d.ta { flags.push("TA"); }
        stack.detail(Line::from(vec![
            chrome::field("Traffic", 8, theme),
            Span::styled(flags.join(" "),
                         Style::default().fg(theme.status_warn)),
        ]));
    }
    // A running count of the decoder's own work, not anything
    // the station said — last of the RDS rows to be worth a row.
    //
    // The session total leads because that is what "Groups" is
    // read as. The run since the last resync trails it, and only
    // when the two differ: that is the whole point of the pair —
    // a session total climbing while the run keeps restarting is
    // a host that cannot keep up, not a station that will not
    // decode.
    if d.groups_session > 0 && !dropped {
        let mut row = vec![
            chrome::field("Groups", 8, theme),
            Span::styled(d.groups_session.to_string(), val(theme)),
        ];
        if d.groups_ok != d.groups_session {
            row.push(Span::styled(format!("  +{}", d.groups_ok),
                                  Style::default().fg(theme.stale)));
        }
        stack.minor(Line::from(row));
    }
    // RadioText is a free-text field up to 64 characters, so it
    // is the one thing here that has to wrap to the column.
    if let Some(rt) = d.rt.as_deref().filter(|_| !dropped) {
        for row in chrome::wrap(rt, iw.saturating_sub(1), RT_MAX_ROWS) {
            stack.detail(Line::from(vec![Span::raw(" "), Span::styled(row, val(theme))]));
        }
    }
}

/// The RDS headline: what the section leads with before any of the detail rows.
///
/// The states have to read differently. A confirmed Programme Service name is the
/// answer. Block sync without a name yet means the decoder is working and the name
/// is seconds away. Neither means the station carries no RDS *as far as we can
/// tell* — which is why it is phrased as an absence, not a failure.
///
/// `age` is how long since a whole group last arrived, and it is what stops a name
/// from outliving its station. RDS is the one measurement here that accumulates, so
/// it is the one that can sit on screen looking confident long after reception has
/// stopped: past [`RDS_AGED_AFTER`] the name is marked with how old it is, and past
/// [`RDS_DROPPED_AFTER`] it is not shown at all.
///
/// Returns the marker glyph, the text, and whether it is a positive result.
fn rds_headline(d: &RdsData, sync: bool, age: Option<Duration>)
    -> (&'static str, String, bool)
{
    let dropped = age.is_none_or(|a| a > RDS_DROPPED_AFTER);
    let aged    = age.is_some_and(|a| a > RDS_AGED_AFTER);
    match d.ps.as_deref() {
        Some(ps) if !dropped && aged => {
            let secs = age.map(|a| a.as_secs()).unwrap_or(0);
            ("\u{25cc}", format!("{}   {secs} s ago", ps.trim()), false)
        }
        Some(ps) if !dropped => ("\u{25cf}", ps.trim().to_string(), true),
        // Either no name yet, or one too old to stand: both come down to whether
        // the decoder is currently getting anywhere.
        // The session total, not the current run: a resync zeroes `groups_ok`, and
        // reading that as "no RDS" would blink the headline off every time a block
        // is lost on a station that is decoding perfectly well.
        _ if sync || (d.groups_session > 0 && !dropped) => ("\u{25cc}", "DECODING".into(), false),
        _ => ("\u{25cb}", "NO RDS".into(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rds(ps: Option<&str>, groups: u32) -> RdsData {
        RdsData {
            pi: Some(0xB201), ps: ps.map(str::to_string),
            groups_ok: groups, groups_session: groups,
            ..Default::default()
        }
    }

    /// A group that arrived just now — the normal case for a station on air.
    const FRESH: Option<Duration> = Some(Duration::ZERO);

    #[test]
    fn rds_headline_leads_with_the_station_name() {
        let (mark, text, ok) = rds_headline(&rds(Some("SDRTOP  "), 40), true, FRESH);
        assert_eq!(text, "SDRTOP", "trailing RDS padding must not reach the screen");
        assert!(ok);
        assert_eq!(mark, "\u{25cf}");
    }

    #[test]
    fn rds_headline_separates_decoding_from_absent() {
        // Groups arriving but no confirmed name yet: the decoder is working.
        let (_, text, ok) = rds_headline(&rds(None, 3), false, FRESH);
        assert_eq!(text, "DECODING");
        assert!(!ok);
        // Sync alone counts too — the first groups have not completed yet.
        assert_eq!(rds_headline(&rds(None, 0), true, FRESH).1, "DECODING");
        // Neither: as far as we can tell there is no RDS here.
        assert_eq!(rds_headline(&rds(None, 0), false, None).1, "NO RDS");
    }

    #[test]
    fn rds_headline_marks_a_name_that_has_stopped_arriving() {
        // The station is still named — it may well come back — but the panel says
        // how long ago it last spoke, instead of showing a confident lamp.
        let age = Some(RDS_AGED_AFTER + Duration::from_secs(7));
        let (mark, text, ok) = rds_headline(&rds(Some("RADIO 1"), 40), false, age);
        assert_eq!(mark, "\u{25cc}", "an aged name must not wear the live lamp");
        assert!(text.starts_with("RADIO 1"));
        assert!(text.contains("12 s ago"), "got {text}");
        assert!(!ok);
    }

    #[test]
    fn rds_headline_drops_a_name_that_outlived_its_station() {
        // The bug this guards: nine seconds after retuning from 92.8 to 96.6 the
        // panel still read "● RADIO 1", group counter frozen. Retuning now wipes the
        // decoder outright, but a station simply going off air has to expire too.
        let age = Some(RDS_DROPPED_AFTER + Duration::from_secs(1));
        let (mark, text, ok) = rds_headline(&rds(Some("RADIO 1"), 40), false, age);
        assert_eq!(text, "NO RDS");
        assert_eq!(mark, "\u{25cb}");
        assert!(!ok);
        // Never a group at all is the same answer.
        assert_eq!(rds_headline(&rds(Some("RADIO 1"), 40), false, None).1, "NO RDS");
    }

    #[test]
    fn rds_headline_keeps_decoding_while_sync_holds() {
        // Sync outranks the drop timeout: blocks are arriving even if no whole group
        // has completed for a while, so "NO RDS" would be wrong.
        let age = Some(RDS_DROPPED_AFTER + Duration::from_secs(5));
        assert_eq!(rds_headline(&rds(None, 40), true, age).1, "DECODING");
    }
}
