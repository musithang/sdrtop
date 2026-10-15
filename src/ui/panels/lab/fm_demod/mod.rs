// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `fm_demod` - the right column of the `lab_signal` preset: the FM MPX ·
//! DEMOD instrument.
//!
//! The panel dispatches on modulation ([`sections_for`]) rather than showing a
//! fixed grid: broadcast FM gets MPX baseband, the stereo pilot, deviation and
//! RDS; narrow-band FM swaps the broadcast sections for CTCSS; AM shows depth and
//! carrier level instead of deviation. A section a mode does not have is absent,
//! not rendered empty.
//!
//! Deviation is measured about the carrier (`signal::demod`), so a mistuned radio
//! reports its tuning error as offset rather than as modulation, and the bar's
//! reference comes from `deviation_limit_hz`. RDS comes from `signal::rds_demod`
//! by way of `signal::rds`.
//!
//! The panel never invents a reading: when the demod is off, the signal is too
//! weak, the modulation is not something an FM discriminator describes, or the
//! last measurement has aged out, it falls back to the neutral idle headline.
//!
//! Split one module per zone of the panel, because that dispatch is the design
//! and the zones are what a reader is looking for:
//!
//! - [`headline`]: the lock / idle status rows above everything, and the
//!   advisories that hang off a working lock.
//! - [`mpx`], [`pilot`], [`deviation`], [`ctcss`], [`am`], [`rds`]: one per
//!   section, in the order [`sections_for`] can emit them. `am` holds both AM
//!   sections because they read one measurement and appear together.
//! - [`stack`]: the row stack and the shedding rule - the only place that
//!   decides what a short panel gives up.
//! - [`fmt`]: the value vocabulary the sections share.
//!
//! This module owns the dispatch and the height arithmetic; the sections know
//! nothing about each other.

mod am;
mod ctcss;
mod deviation;
mod fmt;
mod headline;
mod mpx;
mod pilot;
mod rds;
mod stack;

use ratatui::{layout::Rect, text::Line, widgets::Paragraph, Frame};

use crate::state::{Modulation, SdrMetrics};
use crate::ui::chrome;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

use stack::Stack;

pub struct FmDemodPanel;

/// One zone of the demod panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sec {
    Mpx,
    Pilot,
    Deviation,
    Ctcss,
    Depth,
    Carrier,
    Rds,
}

/// The sections a given modulation's panel shows.
///
/// This is the single dispatch the panel was designed around. MPX, the stereo
/// pilot and RDS are wide-band FM concepts that simply do not exist for NFM or
/// AM, and FM deviation is meaningless for an amplitude-modulated carrier - so
/// each mode is shown only what it actually has, rather than a fixed grid of
/// sections where most read as permanently empty.
///
/// There is no AUDIO section. One existed as a Phase-6 placeholder and appeared in
/// every mode with nothing under it, in every state, forever - which is precisely
/// the failure this dispatch was built to avoid. It comes back when there is
/// something to put in it.
fn sections_for(m: Modulation) -> &'static [Sec] {
    match m {
        Modulation::Nfm => &[Sec::Deviation, Sec::Ctcss],
        Modulation::Am => &[Sec::Depth, Sec::Carrier],
        // An unclassified carrier keeps the broadcast shape - the state the panel
        // rests in before anything is tuned.
        Modulation::Wfm | Modulation::Unknown => &[Sec::Mpx, Sec::Pilot, Sec::Deviation, Sec::Rds],
    }
}

impl Panel for FmDemodPanel {
    fn name(&self) -> &'static str {
        "fm_demod"
    }
    fn min_size(&self) -> (u16, u16) {
        (28, 12)
    }

    fn focus_key(&self) -> Option<char> {
        Some('m')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("Space", "Demod on/off"),
            ("←/→", "Channel offset"),
            ("P", "Snap to carrier"),
            ("0", "Centre"),
            ("T", "Mode"),
            ("C", "Snapshot to log"),
        ]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        // The marker sits on the M of MPX, not the M of FM: it is the start of a
        // word, so the eye lands on it instead of hunting inside an initialism.
        PanelChrome::new("FM _MPX \u{00b7} Demod").stale_when(Staleness::NotStreaming)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let stale = !state.radio.hw_streaming;
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let h = inner.height as usize;
        let iw = inner.width as usize;

        // The MPX trace is the one block whose height is a free choice, and it
        // cannot be sized until the rest of the stack is known - a taller trace is
        // worth having only out of genuinely spare rows. So the stack is built once
        // at one trace row to measure the slack, then rebuilt to spend it. The
        // probe leaves the advisories out: every section keeps one shape with or
        // without its reading, so without them the slack depends only on the
        // height and the mode, and the trace does not change size when an
        // advisory comes or goes. Building is pure line construction over a
        // snapshot that is already cloned, so the second pass costs nothing worth
        // protecting against.
        let probe = build_stack(state, theme, iw, stale, 1, false)
            .0
            .fit(h)
            .len();
        let spare = h
            .saturating_sub(probe)
            .saturating_sub(mpx::MPX_TRACE_SLACK_RESERVE);
        let trace_rows = (1 + spare).min(mpx::MPX_TRACE_MAX_ROWS);

        let (stack, headline_only) = build_stack(state, theme, iw, stale, trace_rows, true);
        let mut lines = stack.fit(h);

        if headline_only {
            // With no sections there are two lines of message in a column thirty
            // rows tall, and left at the top they read as content that failed to
            // render. Centring them says "nothing here" deliberately.
            //
            // Not via `fit_spacers`: it fills by *growing the existing blank rows*
            // proportionally, so leading padding gets amplified along with
            // everything else and the message ends up pinned to the floor. An empty
            // state has no stack to breathe - it just needs placing.
            let pad = h.saturating_sub(lines.len()) / 2;
            for _ in 0..pad {
                lines.insert(0, Line::raw(""));
            }
        } else {
            chrome::fit_spacers(&mut lines, h);
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// Build the panel's row stack at a given MPX trace height, and say whether it came
/// out headline-only (no section had anything to show). Pure: no locking, no I/O, no
/// mutation - see the module note on panels rendering from a snapshot.
fn build_stack(
    state: &SdrMetrics,
    theme: &crate::Theme,
    iw: usize,
    stale: bool,
    trace_rows: usize,
    advise: bool,
) -> (Stack<'static>, bool) {
    let mut stack = Stack::new();

    // `live()` already refuses an aged-out reading, so a frozen number can
    // never be mistaken for a current one.
    let modulation = state
        .demod
        .effective_modulation(state.signal.modulation_settled);
    let measure = if stale { None } else { state.demod.live() };
    let am_measure = if stale { None } else { state.demod.live_am() };
    // One demodulator runs at a time, so a lock is whichever produced a value.
    let locked = measure.is_some() || am_measure.is_some();

    headline::lines(
        &mut stack, state, modulation, locked, stale, advise, iw, theme,
    );
    stack.gap();

    // ── Sections, chosen by modulation ─────────────────────────────────
    //
    // Only while there is something to put under them. With the demod off, or
    // before it has locked, every section is empty at once - and a stack of five
    // headings over blank space reads as a broken panel rather than an idle one.
    // The headline above already says why nothing is measured; repeating that in
    // scaffolding adds no information.
    let sections: &[Sec] = if state.demod.has_measurement() {
        sections_for(modulation)
    } else {
        &[]
    };
    for sec in sections {
        match sec {
            Sec::Mpx => mpx::lines(&mut stack, state, iw, trace_rows, theme),
            Sec::Pilot => pilot::lines(&mut stack, state, iw, theme),
            Sec::Deviation => deviation::lines(&mut stack, measure, modulation, iw, theme),
            Sec::Ctcss => ctcss::lines(&mut stack, state, stale, iw, theme),
            Sec::Depth => am::depth_lines(&mut stack, am_measure, iw, theme),
            Sec::Carrier => am::carrier_lines(&mut stack, am_measure, iw, theme),
            Sec::Rds => rds::lines(&mut stack, state, iw, theme),
        }
        stack.gap();
    }

    (stack, sections.is_empty())
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

    /// A live FM lock on a streaming radio: the classifier's raw class is
    /// `raw`, the class it has held for a second is `settled`.
    pub(super) fn locked(raw: Modulation, settled: Modulation) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.signal.modulation = raw;
        m.signal.modulation_settled = settled;
        m.demod.fm = Some(crate::state::FmMeasure {
            peak_dev_hz: 60_000.0,
            rms_dev_hz: 20_000.0,
            carrier_offset_hz: 150.0,
        });
        m.demod.channel_rate_hz = 240_000.0;
        m.demod.decimation = 8;
        m.demod.last_update = Some(std::time::Instant::now());
        m
    }

    /// **The sections follow the class that has held, not the latest
    /// guess.** On a bursty broadcast station the classifier says NFM for a
    /// frame; the panel keeps its broadcast sections.
    #[test]
    fn the_sections_follow_the_settled_class() {
        let text = crate::state::fixture::draw(
            FmDemodPanel,
            40,
            34,
            &locked(Modulation::Nfm, Modulation::Wfm),
        )
        .join("\n");
        assert!(
            text.contains("DEVIATION") && text.contains("PILOT"),
            "{text}"
        );
        assert!(!text.contains("CTCSS"), "{text}");
    }

    /// The row each label first appears on, `None` where it is absent.
    fn rows_of(lines: &[String], labels: &[&str]) -> Vec<Option<usize>> {
        labels
            .iter()
            .map(|l| lines.iter().position(|r| r.contains(l)))
            .collect()
    }

    fn wfm_full() -> SdrMetrics {
        use crate::signal::demod::{PilotMeasure, PilotState};
        let mut m = locked(Modulation::Wfm, Modulation::Wfm);
        let mut mags = vec![1.0f32; 512];
        mags[117] = 7_500.0;
        m.demod.mpx = Some(std::sync::Arc::new(crate::state::MpxFrame {
            bin_hz: 163.0,
            mags_hz: mags,
        }));
        m.demod.pilot = Some(PilotMeasure {
            state: PilotState::Locked,
            deviation_hz: 6_800.0,
            injection_pct: 9.1,
        });
        m.demod.rds = Some(std::sync::Arc::new(crate::signal::rds::RdsData {
            pi: Some(0xB201),
            pty: Some(10),
            tp: true,
            ps: Some("SDRTOP  ".into()),
            rt: Some("A RadioText long enough to need both of its rows here".into()),
            groups_ok: 40,
            groups_session: 40,
            ..Default::default()
        }));
        m.demod.rds_last_group = Some(std::time::Instant::now());
        m
    }

    const WFM_LABELS: [&str; 4] = ["MPX BASEBAND", "PILOT / STEREO", "DEVIATION", "RDS"];

    /// **A burst does not move the panel.** Each section keeps its rows
    /// whether or not its reading is there: a pilot that drops from stereo
    /// to mono, an RDS decoder that has not named the station yet and an
    /// MPX frame missing from a short block leave every heading where it was.
    #[test]
    fn a_missing_reading_keeps_its_rows() {
        let full = crate::state::fixture::draw(FmDemodPanel, 40, 44, &wfm_full());
        let mut bare = wfm_full();
        bare.demod.mpx = None;
        bare.demod.pilot = None;
        bare.demod.rds = None;
        let bare = crate::state::fixture::draw(FmDemodPanel, 40, 44, &bare);
        let mut mono = wfm_full();
        if let Some(p) = mono.demod.pilot.as_mut() {
            p.state = crate::signal::demod::PilotState::Absent;
        }
        let mono = crate::state::fixture::draw(FmDemodPanel, 40, 44, &mono);
        let want = rows_of(&full, &WFM_LABELS);
        assert!(want.iter().all(Option::is_some), "{}", full.join("\n"));
        assert_eq!(rows_of(&bare, &WFM_LABELS), want, "{}", bare.join("\n"));
        assert_eq!(rows_of(&mono, &WFM_LABELS), want, "{}", mono.join("\n"));
    }

    /// The same for narrow-band FM: a tone, a search still filling its
    /// window and a channel with no tone are one shape.
    #[test]
    fn the_ctcss_states_are_one_shape() {
        let mut tone = locked(Modulation::Nfm, Modulation::Nfm);
        tone.demod.ctcss = Some(crate::state::CtcssMeasure {
            tone_hz: 88.5,
            deviation_hz: 450.0,
            margin_db: 18.0,
        });
        tone.demod.ctcss_fill = 1.0;
        let mut searching = tone.clone();
        searching.demod.ctcss = None;
        searching.demod.ctcss_fill = 0.4;
        let mut none = searching.clone();
        none.demod.ctcss_fill = 1.0;
        let shape = |m: &SdrMetrics| {
            let lines = crate::state::fixture::draw(FmDemodPanel, 40, 30, m);
            let filled: Vec<bool> = lines
                .iter()
                .map(|l| !l.trim_matches(|c| c == '\u{2502}' || c == ' ').is_empty())
                .collect();
            (rows_of(&lines, &["DEVIATION", "CTCSS"]), filled)
        };
        assert_eq!(shape(&searching), shape(&tone));
        assert_eq!(shape(&none), shape(&tone));
    }

    /// **The MPX trace keeps its height** when an advisory comes and goes,
    /// rather than being resized from whatever rows happen to be spare.
    #[test]
    fn the_trace_height_does_not_follow_the_advisories() {
        let trace = |m: &SdrMetrics, h: u16| {
            let lines = crate::state::fixture::draw(FmDemodPanel, 40, h, m);
            lines
                .iter()
                .filter(|l| l.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)))
                .count()
        };
        let quiet = wfm_full();
        let mut dropping = wfm_full();
        dropping.demod.blocks_dropped = 3;
        dropping.demod.last_drop = Some(std::time::Instant::now());
        for h in 24..=44 {
            assert_eq!(trace(&dropping, h), trace(&quiet, h), "at {h} rows");
        }
        assert!(trace(&quiet, 44) > 2, "the trace had room to grow");
    }

    /// **A held reading says so.** Under the presence gate the last reading
    /// stays until it ages out; the headline stops calling it a lock.
    #[test]
    fn a_held_reading_is_not_called_a_lock() {
        let fresh = locked(Modulation::Wfm, Modulation::Wfm);
        let text = crate::state::fixture::draw(FmDemodPanel, 40, 34, &fresh).join("\n");
        assert!(text.contains("DEMOD LOCK"), "{text}");
        let mut held = fresh;
        held.demod.last_update =
            std::time::Instant::now().checked_sub(std::time::Duration::from_millis(900));
        let text = crate::state::fixture::draw(FmDemodPanel, 40, 34, &held).join("\n");
        assert!(
            text.contains("DEMOD HELD") && !text.contains("DEMOD LOCK"),
            "{text}"
        );
        assert!(
            text.contains("DEVIATION"),
            "the sections stay while held: {text}"
        );
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
        assert_eq!(
            sections_for(Modulation::Unknown),
            sections_for(Modulation::Wfm)
        );
    }

    #[test]
    fn no_mode_carries_a_section_with_nothing_behind_it() {
        // The AUDIO placeholder appeared in every mode, always empty, for as long as
        // it existed. Each set is now exactly the sections that mode can fill.
        assert_eq!(
            sections_for(Modulation::Wfm),
            &[Sec::Mpx, Sec::Pilot, Sec::Deviation, Sec::Rds]
        );
        assert_eq!(sections_for(Modulation::Nfm), &[Sec::Deviation, Sec::Ctcss]);
        assert_eq!(sections_for(Modulation::Am), &[Sec::Depth, Sec::Carrier]);
    }
}
