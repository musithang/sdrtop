//! `fm_demod` — the right column of the `lab_signal` preset's redesign
//! (DSN-2026-07): the FM MPX · DEMOD instrument.
//!
//! The panel dispatches on modulation ([`sections_for`]) rather than showing a
//! fixed grid: broadcast FM gets MPX baseband, the stereo pilot, deviation and
//! RDS; narrow-band FM swaps the broadcast sections for CTCSS; AM shows depth and
//! carrier level instead of deviation. A section a mode does not have is absent,
//! not rendered empty.
//!
//! Deviation is measured about the carrier (`signal::demod`), so a mistuned radio
//! reports its tuning error as offset rather than as modulation, and the bar's
//! reference comes from [`deviation_limit_hz`]. RDS comes from `signal::rds_demod`
//! by way of `signal::rds`.
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

use std::time::Duration;

use crate::signal::demod::{FILTER_QUALITY_D_LIMIT, MPX_SPAN_HZ, PILOT_HZ};
use crate::state::{
    deviation_limit_hz, FmMeasure, Modulation, MpxFrame, PilotState, SdrMetrics,
    PTY_NAMES, RDS_AGED_AFTER, RDS_DROPPED_AFTER,
};
use crate::ui::chrome;
use crate::ui::micro_common::bar_spans;
use crate::ui::panel::Panel;

pub struct FmDemodPanel;

/// One zone of the demod panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sec { Mpx, Pilot, Deviation, Ctcss, Depth, Carrier, Rds }

/// The sections a given modulation's panel shows.
///
/// This is the single dispatch the panel was designed around. MPX, the stereo
/// pilot and RDS are wide-band FM concepts that simply do not exist for NFM or
/// AM, and FM deviation is meaningless for an amplitude-modulated carrier — so
/// each mode is shown only what it actually has, rather than a fixed grid of
/// sections where most read as permanently empty.
///
/// There is no AUDIO section. One existed as a Phase-6 placeholder and appeared in
/// every mode with nothing under it, in every state, forever — which is precisely
/// the failure this dispatch was built to avoid. It comes back when there is
/// something to put in it.
fn sections_for(m: Modulation) -> &'static [Sec] {
    match m {
        Modulation::Nfm => &[Sec::Deviation, Sec::Ctcss],
        Modulation::Am  => &[Sec::Depth, Sec::Carrier],
        // An unclassified carrier keeps the broadcast shape — the state the panel
        // rests in before anything is tuned.
        Modulation::Wfm | Modulation::Unknown =>
            &[Sec::Mpx, Sec::Pilot, Sec::Deviation, Sec::Rds],
    }
}

/// What a row is worth when the stack does not fit the panel.
///
/// `chrome::fit_spacers` can only hand back blank rows, and at 120x38 the WFM stack
/// is still ten rows over once every spacer is gone — so the tail was simply clipped
/// by the paragraph, and the tail is RDS. The panel showed `● DANKO` and lost the PI,
/// the programme type, the group count and the RadioText under it: the payload of the
/// section, cut without a mark. Reordering RDS higher only moves the amputation onto
/// DEVIATION, which is a measurement too.
///
/// So rows say what they are instead, and the panel spends its height in that order.
/// [`Heading`](Prio::Heading) and [`Core`](Prio::Core) are a section's nameplate and
/// its lead reading — losing those makes the section a lie by omission.
/// [`Detail`](Prio::Detail) is a secondary number a reader can do without;
/// [`Minor`](Prio::Minor) is the least of a section's numbers, ranked below its
/// siblings. [`Ornament`](Prio::Ornament) is a *picture of* a number already printed
/// beside it: the deviation bar under `Peak`, the MPX tick row.
///
/// Shedding takes from the **top** of the stack first within each class, which is
/// what protects the foot — the sections that used to vanish whole. `Minor` exists
/// because that top-down order is right *between* sections and wrong *within* one:
/// with a single detail class the pilot's deviation went before its injection level,
/// purely because it is printed first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Prio { Heading, Core, Detail, Minor, Ornament }

/// The panel's line stack, recording each row's [`Prio`] as it is pushed.
///
/// [`push`](Stack::push) is deliberately the un-suffixed one and means `Core`: a row
/// is only sheddable when someone says so, so a row added later without a thought
/// stays on screen rather than quietly becoming the first casualty.
struct Stack<'a> { rows: Vec<(Line<'a>, Prio)> }

/// A blank spacer row — the ones `chrome::fit_spacers` owns.
fn is_gap(l: &Line<'_>) -> bool { l.spans.iter().all(|s| s.content.trim().is_empty()) }

impl<'a> Stack<'a> {
    fn new() -> Self { Stack { rows: Vec::new() } }
    fn push(&mut self, l: Line<'a>)     { self.rows.push((l, Prio::Core)); }
    fn heading(&mut self, l: Line<'a>)  { self.rows.push((l, Prio::Heading)); }
    fn detail(&mut self, l: Line<'a>)   { self.rows.push((l, Prio::Detail)); }
    fn minor(&mut self, l: Line<'a>)    { self.rows.push((l, Prio::Minor)); }
    fn ornament(&mut self, l: Line<'a>) { self.rows.push((l, Prio::Ornament)); }
    fn gap(&mut self)                   { self.rows.push((Line::raw(""), Prio::Core)); }

    /// Shed rows until the stack fits `avail`, cheapest class first, and return what
    /// is left. The caller still runs `chrome::fit_spacers` on the result.
    ///
    /// Spacers are counted as already spent: `collapse_spacers` will take every one
    /// of them back for free afterwards, so they must not buy an ornament's life.
    fn fit(mut self, avail: usize) -> Vec<Line<'a>> {
        for prio in [Prio::Ornament, Prio::Minor, Prio::Detail] {
            while Self::over(&self.rows, avail) {
                let Some(i) = self.rows.iter().position(|(_, p)| *p == prio) else { break };
                self.rows.remove(i);
                // One removal at a time, because emptying a section frees its
                // nameplate too and that may be all the room still needed.
                drop_orphan_headings(&mut self.rows);
            }
        }
        self.rows.into_iter().map(|(l, _)| l).collect()
    }

    fn over(rows: &[(Line<'a>, Prio)], avail: usize) -> bool {
        let gaps = rows.iter().filter(|(l, _)| is_gap(l)).count();
        rows.len() > avail.saturating_add(gaps)
    }
}

/// Remove any section nameplate left with nothing under it.
///
/// Shedding works row by row and can empty a section completely — and a nameplate
/// over blank space is exactly the failure B3 took out of the idle panel. Walked
/// back to front so a run of newly emptied sections collapses in one pass.
fn drop_orphan_headings(rows: &mut Vec<(Line<'_>, Prio)>) {
    let mut i = rows.len();
    while i > 0 {
        i -= 1;
        if rows[i].1 != Prio::Heading { continue; }
        let orphan = rows[i + 1..].iter()
            .find(|(l, _)| !is_gap(l))
            .is_none_or(|(_, p)| *p == Prio::Heading);
        if orphan { rows.remove(i); }
    }
}

/// Rows the MPX trace grows to when the panel has height going spare, and the slack
/// it refuses to spend getting there.
///
/// One braille row is four vertical levels over a 40 dB window — 10 dB a level, on
/// which the stereo pilot (~20 dB under the audio) is level 2 of 4 and indistinct
/// from the hump beside it. Three rows make it 3.3 dB a level. The reserve stops a
/// panel with barely enough slack from trading all its air for trace resolution.
const MPX_TRACE_MAX_ROWS: usize = 3;
const MPX_TRACE_SLACK_RESERVE: usize = 4;

/// Rows a headline advisory may wrap to. Two holds the longest of them at the
/// panel's declared minimum width; a third would cost a section row to say the
/// same thing.
const ADVISORY_MAX_ROWS: usize = 2;

/// Rows of RadioText the panel will show. Two rows hold the 64-character maximum
/// on any column wide enough to be readable; a third would push the sections below
/// off a short terminal for a field that is usually much shorter than its limit.
const RT_MAX_ROWS: usize = 2;

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
fn rds_headline(d: &crate::state::RdsData, sync: bool, age: Option<Duration>)
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

        let h = inner.height as usize;
        let iw = inner.width as usize;

        // The MPX trace is the one block whose height is a free choice, and it
        // cannot be sized until the rest of the stack is known — a taller trace is
        // worth having only out of genuinely spare rows, and how many of those there
        // are depends on the modulation, on whether RDS is decoding, and on which
        // advisories are showing. So the stack is built once at one trace row to
        // measure the slack, then rebuilt to spend it. Building is pure line
        // construction over a snapshot that is already cloned, so the second pass
        // costs nothing worth protecting against.
        let probe = build_stack(state, theme, iw, stale, 1).0.fit(h).len();
        let spare = h.saturating_sub(probe).saturating_sub(MPX_TRACE_SLACK_RESERVE);
        let trace_rows = (1 + spare).min(MPX_TRACE_MAX_ROWS);

        let (stack, headline_only) = build_stack(state, theme, iw, stale, trace_rows);
        let mut lines = stack.fit(h);

        if headline_only {
            // With no sections there are two lines of message in a column thirty
            // rows tall, and left at the top they read as content that failed to
            // render. Centring them says "nothing here" deliberately.
            //
            // Not via `fit_spacers`: it fills by *growing the existing blank rows*
            // proportionally, so leading padding gets amplified along with
            // everything else and the message ends up pinned to the floor. An empty
            // state has no stack to breathe — it just needs placing.
            let pad = h.saturating_sub(lines.len()) / 2;
            for _ in 0..pad { lines.insert(0, Line::raw("")); }
        } else {
            chrome::fit_spacers(&mut lines, h);
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// Build the panel's row stack at a given MPX trace height, and say whether it came
/// out headline-only (no section had anything to show). Pure: no locking, no I/O, no
/// mutation — see the module note on panels rendering from a snapshot.
fn build_stack(
    state: &SdrMetrics, theme: &crate::Theme, iw: usize, stale: bool, trace_rows: usize,
) -> (Stack<'static>, bool) {
        let dim = Style::default().fg(theme.stale);
        let lbl = Style::default().fg(theme.label);
        let val = Style::default().fg(theme.value);
        let mut stack = Stack::new();

        // `live()` already refuses an aged-out reading, so a frozen number can
        // never be mistaken for a current one.
        let modulation = state.demod.effective_modulation(state.signal.modulation);
        let measure = if stale { None } else { state.demod.live() };
        let am = if stale { None } else { state.demod.live_am() };
        // One demodulator runs at a time, so a lock is whichever produced a value.
        let locked = measure.is_some() || am.is_some();

        // ── Status headline ────────────────────────────────────────────────
        if stale {
            stack.push(Line::from(vec![Span::raw(" "), Span::styled("\u{25cb} IDLE \u{2014} RX stopped", dim)]));
        } else if locked {
            // A forced mode is marked, so a reading is never mistaken for the
            // classifier's own conclusion.
            let src = if state.demod.mode_override.is_some() { " \u{2731}" } else { "" };
            stack.push(Line::from(vec![
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
            stack.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{:.3} MHz ", demod_hz as f64 / 1e6),
                             Style::default().fg(theme.value_hi)),
                Span::styled(off_str, lbl),
            ]));
            // The chain's own settings, not a measurement — the first thing the
            // headline can spare.
            stack.minor(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("{:.0} kHz channel \u{00b7} \u{00f7}{}", state.demod.channel_rate_hz / 1000.0, d),
                    lbl),
            ]));
            // Advisories are sentences, so they wrap rather than being clipped
            // mid-word the way the DC-spike line was at 46 columns. Each is
            // `Detail`: useful, and the first thing a short panel can spare.
            let mut advise = |text: String| {
                for row in chrome::wrap(&text, iw.saturating_sub(1), ADVISORY_MAX_ROWS) {
                    stack.detail(Line::from(vec![
                        Span::raw(" "),
                        Span::styled(row, Style::default().fg(theme.status_warn)),
                    ]));
                }
            };
            // Sitting on the tuned centre means sharing the channel with the
            // front-end's DC offset / LO leakage. Point at the existing fix.
            if state.demod.on_dc_spike(state.iq.cal.correcting()) {
                advise("\u{2192} on DC spike \u{2014} [D] in Lab IQ, or offset".to_string());
            }
            // Blocks lost on the way here. RDS and CTCSS both need unbroken runs, so
            // without this line a busy host and a station with no RDS look identical
            // — the panel simply never decodes anything and never says why.
            if let Some(n) = state.demod.dropping() {
                advise(format!("\u{2192} {n} block{} dropped \u{2014} RDS/CTCSS need a clean run",
                               if n == 1 { "" } else { "s" }));
            }
            // The channel filter stops sharpening once the decimation factor
            // saturates the tap budget — advise, never coerce, in the house style.
            if d > FILTER_QUALITY_D_LIMIT {
                advise("\u{2192} 2\u{2013}2.4 Msps sharpens this".to_string());
            }
        } else {
            // The *effective* mode, not the classifier's raw guess: the sections
            // below are chosen by it, and passing the raw one meant forcing AM with
            // [T] on an unclassified carrier printed "Tune to a broadcast station"
            // directly above a DEPTH / CARRIER pair.
            let (mark, headline, detail) = idle_status(modulation, state.demod.user_on);
            stack.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{mark} {headline}"), Style::default().fg(theme.stale).add_modifier(Modifier::BOLD)),
            ]));
            stack.push(Line::from(vec![Span::raw(" "), Span::styled(detail, lbl)]));
        }
        stack.gap();

        // ── Sections, chosen by modulation ─────────────────────────────────
        //
        // Only while there is something to put under them. With the demod off, or
        // before it has locked, every section is empty at once — and a stack of five
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
            Sec::Mpx => {
        stack.heading(chrome::section("MPX BASEBAND", "0-60 kHz", iw, theme));
        match state.demod.live_mpx() {
            Some(frame) => {
                let w = iw.saturating_sub(2);
                let profile = mpx_profile(frame, w * 2);
                if w >= 8 && !profile.is_empty() {
                    // The trace is one block at whatever height it was given, so the
                    // rows go on together: the caller sizes it, the shedding pass
                    // must not take a slice out of the middle of a picture.
                    for row in crate::ui::charts::braille_profile(&profile, w, trace_rows) {
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
                        Span::styled(mpx_ticks(w), lbl),
                    ]));
                }
            }
            None => stack.gap(),
        }
            }
            Sec::Pilot => {
        stack.heading(chrome::section("PILOT / STEREO", "19 kHz", iw, theme));
        match state.demod.live_pilot() {
            Some(p) => {
                let (mark, word, color) = match p.state {
                    PilotState::Locked   => ("\u{25cf}", "STEREO",   theme.status_ok),
                    PilotState::Marginal => ("\u{25d0}", "MARGINAL", theme.status_warn),
                    PilotState::Absent   => ("\u{25cb}", "MONO",     theme.label),
                };
                stack.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(format!("{mark} {word}"),
                                 Style::default().fg(color).add_modifier(Modifier::BOLD)),
                ]));
                if p.state != PilotState::Absent {
                    stack.detail(Line::from(vec![
                        chrome::field("Pilot", 8, theme),
                        Span::styled(fmt_hz(p.deviation_hz), val),
                    ]));
                    // Injection is the deviation restated against 75 kHz, so it is
                    // the one of the pair that can go.
                    stack.minor(Line::from(vec![
                        chrome::field("Inject", 8, theme),
                        Span::styled(format!("{:.1}%", p.injection_pct), val),
                    ]));
                }
            }
            None => stack.gap(),
        }
            }
            Sec::Deviation => {
        let limit = deviation_limit_hz(modulation);
        let hint = format!("{:.0} kHz max", limit / 1000.0);
        stack.heading(chrome::section("DEVIATION", &hint, iw, theme));
        match measure {
            Some(FmMeasure { peak_dev_hz, rms_dev_hz, carrier_offset_hz }) => {
                let ratio = if limit > 0.0 { peak_dev_hz / limit } else { 0.0 };
                let color = deviation_color(ratio, theme);
                stack.push(Line::from(vec![
                    chrome::field("Peak", 8, theme),
                    Span::styled(fmt_hz(peak_dev_hz), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                ]));
                // Bar width leaves room for the leading space and the trailing "%".
                let bar_w = iw.saturating_sub(9).min(14);
                if bar_w >= 4 {
                    let mut row = vec![Span::raw(" ")];
                    row.extend(bar_spans(ratio.clamp(0.0, 1.0) as f64, bar_w, color, theme));
                    row.push(Span::styled(format!(" {:.0}%", (ratio * 100.0).min(999.0)), lbl));
                    stack.ornament(Line::from(row));
                }
                stack.detail(Line::from(vec![
                    chrome::field("RMS", 8, theme),
                    Span::styled(fmt_hz(rms_dev_hz), val),
                ]));
                // Not "Offset": the headline already uses that word for the
                // channel's offset from the tuned centre, and this is the opposite
                // measurement with the opposite sign — where the carrier sits inside
                // the channel, i.e. the tuning error the demodulator is seeing.
                stack.detail(Line::from(vec![
                    chrome::field("Carrier", 8, theme),
                    Span::styled(fmt_offset(carrier_offset_hz), val),
                ]));
            }
            None => stack.gap(),
        }
            }
            Sec::Ctcss => {
                stack.heading(chrome::section("CTCSS", "subaudible", iw, theme));
                match state.demod.live_ctcss() {
                    Some(t) => {
                        stack.push(Line::from(vec![
                            Span::raw(" "),
                            Span::styled(format!("\u{25cf} {:.1} Hz", t.tone_hz),
                                         Style::default().fg(theme.status_ok).add_modifier(Modifier::BOLD)),
                        ]));
                        stack.detail(Line::from(vec![
                            chrome::field("Dev", 8, theme),
                            Span::styled(fmt_hz(t.deviation_hz), val),
                        ]));
                        stack.minor(Line::from(vec![
                            chrome::field("Margin", 8, theme),
                            Span::styled(format!("{:.0} dB", t.margin_db), val),
                        ]));
                    }
                    // "Still filling the window" and "there is no tone" look the
                    // same on screen unless they are said differently — and only
                    // one of them is a finding.
                    None if state.demod.ctcss_searching() => {
                        stack.push(Line::from(vec![
                            Span::raw(" "),
                            Span::styled(format!("\u{25cc} SEARCHING {:.0}%",
                                                 state.demod.ctcss_fill * 100.0), lbl),
                        ]));
                    }
                    None if !stale => {
                        stack.push(Line::from(vec![
                            Span::raw(" "),
                            Span::styled("\u{25cb} NO TONE", lbl),
                        ]));
                    }
                    None => stack.gap(),
                }
            }
            Sec::Depth => {
                stack.heading(chrome::section("DEPTH", "100% max", iw, theme));
                match am {
                    Some(a) => {
                        let ratio = a.depth_pct / 100.0;
                        let color = depth_color(a.negative_pct, theme);
                        stack.push(Line::from(vec![
                            chrome::field("Depth", 8, theme),
                            Span::styled(format!("{:.0}%", a.depth_pct),
                                         Style::default().fg(color).add_modifier(Modifier::BOLD)),
                        ]));
                        let bar_w = iw.saturating_sub(9).min(14);
                        if bar_w >= 4 {
                            let mut row = vec![Span::raw(" ")];
                            row.extend(bar_spans(ratio.clamp(0.0, 1.0) as f64, bar_w, color, theme));
                            stack.ornament(Line::from(row));
                        }
                        // Split out because they fail differently: a negative peak
                        // reaching 100 % pinches the carrier off and splatters.
                        stack.detail(Line::from(vec![
                            chrome::field("Pos", 8, theme),
                            Span::styled(format!("{:.0}%", a.positive_pct), val),
                        ]));
                        stack.detail(Line::from(vec![
                            chrome::field("Neg", 8, theme),
                            Span::styled(format!("{:.0}%", a.negative_pct),
                                         Style::default().fg(depth_color(a.negative_pct, theme))),
                        ]));
                    }
                    None => stack.gap(),
                }
            }
            Sec::Carrier => {
                stack.heading(chrome::section("CARRIER", "", iw, theme));
                match am {
                    Some(a) => stack.push(Line::from(vec![
                        chrome::field("Level", 8, theme),
                        Span::styled(format!("{:.1} dBFS", a.carrier_dbfs), val),
                    ])),
                    None => stack.gap(),
                }
            }
            Sec::Rds => {
                stack.heading(chrome::section("RDS", "57 kHz", iw, theme));
                match state.demod.live_rds() {
                    Some(d) => {
                        let age = state.demod.rds_age();
                        // Past the drop timeout the accumulated text is not about
                        // anything currently on air, so none of it is shown — not
                        // the name, not the code, not the message.
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
                                Span::styled(format!("{pi:04X}"), val),
                            ]));
                        }
                        if let Some(name) = d.pty.filter(|_| !dropped)
                            .map(|p| PTY_NAMES[(p & 0x1F) as usize]) {
                            stack.detail(Line::from(vec![
                                chrome::field("PTY", 8, theme),
                                Span::styled(name, val),
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
                                Span::styled(d.groups_session.to_string(), val),
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
                                stack.detail(Line::from(vec![Span::raw(" "), Span::styled(row, val)]));
                            }
                        }
                    }
                    None => stack.gap(),
                }
            }
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
    fn no_mode_carries_a_section_with_nothing_behind_it() {
        // The AUDIO placeholder appeared in every mode, always empty, for as long as
        // it existed. Each set is now exactly the sections that mode can fill.
        assert_eq!(sections_for(Modulation::Wfm),
                   &[Sec::Mpx, Sec::Pilot, Sec::Deviation, Sec::Rds]);
        assert_eq!(sections_for(Modulation::Nfm), &[Sec::Deviation, Sec::Ctcss]);
        assert_eq!(sections_for(Modulation::Am),  &[Sec::Depth, Sec::Carrier]);
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
    fn rds(ps: Option<&str>, groups: u32) -> crate::state::RdsData {
        crate::state::RdsData {
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
        let (mark, text, ok) = rds_headline(&rds(Some("DANKO"), 40), false, age);
        assert_eq!(mark, "\u{25cc}", "an aged name must not wear the live lamp");
        assert!(text.starts_with("DANKO"));
        assert!(text.contains("12 s ago"), "got {text}");
        assert!(!ok);
    }

    #[test]
    fn rds_headline_drops_a_name_that_outlived_its_station() {
        // The bug this guards: nine seconds after retuning from 92.8 to 96.6 the
        // panel still read "● DANKO", group counter frozen. Retuning now wipes the
        // decoder outright, but a station simply going off air has to expire too.
        let age = Some(RDS_DROPPED_AFTER + Duration::from_secs(1));
        let (mark, text, ok) = rds_headline(&rds(Some("DANKO"), 40), false, age);
        assert_eq!(text, "NO RDS");
        assert_eq!(mark, "\u{25cb}");
        assert!(!ok);
        // Never a group at all is the same answer.
        assert_eq!(rds_headline(&rds(Some("DANKO"), 40), false, None).1, "NO RDS");
    }

    #[test]
    fn rds_headline_keeps_decoding_while_sync_holds() {
        // Sync outranks the drop timeout: blocks are arriving even if no whole group
        // has completed for a while, so "NO RDS" would be wrong.
        let age = Some(RDS_DROPPED_AFTER + Duration::from_secs(5));
        assert_eq!(rds_headline(&rds(None, 40), true, age).1, "DECODING");
    }

    /// A stand-in for the WFM stack's shape: four sections, each a nameplate plus a
    /// core reading, with detail and ornament hung off them. Content is the row's own
    /// label so a fitted stack can be read back as a list of names.
    fn wfm_stack() -> Stack<'static> {
        let mut s = Stack::new();
        s.push(Line::raw("lock"));
        s.push(Line::raw("freq"));
        s.minor(Line::raw("channel"));
        s.gap();
        s.heading(Line::raw("MPX"));
        s.push(Line::raw("trace"));
        s.ornament(Line::raw("ticks"));
        s.gap();
        s.heading(Line::raw("PILOT"));
        s.push(Line::raw("stereo"));
        s.detail(Line::raw("pilot"));
        s.minor(Line::raw("inject"));
        s.gap();
        s.heading(Line::raw("RDS"));
        s.push(Line::raw("name"));
        s.detail(Line::raw("pi"));
        s.minor(Line::raw("groups"));
        s.detail(Line::raw("rt"));
        s.gap();
        s
    }

    fn names(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().filter(|l| !is_gap(l))
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn fit_keeps_everything_when_there_is_room() {
        let s = wfm_stack();
        let n = s.rows.len();
        assert_eq!(s.fit(n).len(), n, "a stack that fits is left alone");
    }

    #[test]
    fn fit_spends_gaps_before_it_sheds_a_row() {
        // Five spacers are five free rows; `collapse_spacers` reclaims them after
        // this returns, so an overflow smaller than that must cost no content.
        let s = wfm_stack();
        let content = s.rows.iter().filter(|(l, _)| !is_gap(l)).count();
        let before = names(&wfm_stack().fit(usize::MAX));
        assert_eq!(names(&s.fit(content + 1)), before, "spacers paid for it, not the rows");
    }

    #[test]
    fn fit_sheds_ornament_then_minor_then_detail() {
        let order = ["ticks", "channel", "inject", "groups", "pilot", "pi", "rt"];
        let full = wfm_stack().rows.iter().filter(|(l, _)| !is_gap(l)).count();
        let mut gone: Vec<&str> = Vec::new();
        for drop in 1..=order.len() {
            let kept = names(&wfm_stack().fit(full - drop));
            gone = order.iter().copied().filter(|n| !kept.iter().any(|k| k == n)).collect();
            assert_eq!(gone.len(), drop, "shed {drop}, got {gone:?}");
        }
        // Ornament first, then the minor rows top-down, then the details top-down.
        assert_eq!(gone, order);
    }

    #[test]
    fn fit_protects_the_foot_of_the_stack() {
        // B5 itself: the RDS section is last and used to be the part that got cut.
        // Under pressure it must still be the part that survives.
        let full = wfm_stack().rows.iter().filter(|(l, _)| !is_gap(l)).count();
        let kept = names(&wfm_stack().fit(full - 4));
        assert!(kept.contains(&"RDS".to_string()) && kept.contains(&"name".to_string()));
        assert!(kept.contains(&"pi".to_string()) && kept.contains(&"rt".to_string()),
                "RDS lost its payload again: {kept:?}");
    }

    #[test]
    fn fit_takes_an_emptied_sections_nameplate_with_it() {
        // A heading over nothing is what B3 removed from the idle panel; the
        // shedding pass must not put one back.
        let mut s = Stack::new();
        s.push(Line::raw("lock"));
        s.gap();
        s.heading(Line::raw("MPX"));
        s.ornament(Line::raw("ticks"));
        s.gap();
        s.heading(Line::raw("PILOT"));
        s.push(Line::raw("stereo"));
        let kept = names(&s.fit(3));
        assert_eq!(kept, vec!["lock", "PILOT", "stereo"], "orphaned nameplate survived: {kept:?}");
    }

    #[test]
    fn fit_clips_rather_than_shedding_a_core_reading() {
        // Nothing left to shed: the remaining rows are all readings, and losing one
        // silently would be the original bug. The paragraph clips instead, which is
        // the honest floor of what this can do without scrolling.
        let mut s = Stack::new();
        for n in ["a", "b", "c", "d"] { s.push(Line::raw(n)); }
        assert_eq!(names(&s.fit(2)), vec!["a", "b", "c", "d"]);
    }
}
