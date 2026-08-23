//! The line stack and its shedding rule: what the panel gives up first when the
//! sections it wants to show are taller than the column it has.
//!
//! Every other module in this panel only ever pushes rows in here, saying what
//! each row is worth. This module is the only one that decides what survives.

use ratatui::text::Line;

/// What a row is worth when the stack does not fit the panel.
///
/// `chrome::fit_spacers` can only hand back blank rows, and at 120x38 the WFM stack
/// is still ten rows over once every spacer is gone — so the tail was simply clipped
/// by the paragraph, and the tail is RDS. The panel showed `● RADIO 1` and lost the PI,
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
pub(super) enum Prio { Heading, Core, Detail, Minor, Ornament }

/// The panel's line stack, recording each row's [`Prio`] as it is pushed.
///
/// [`push`](Stack::push) is deliberately the un-suffixed one and means `Core`: a row
/// is only sheddable when someone says so, so a row added later without a thought
/// stays on screen rather than quietly becoming the first casualty.
pub(super) struct Stack<'a> { rows: Vec<(Line<'a>, Prio)> }

/// A blank spacer row — the ones `chrome::fit_spacers` owns.
fn is_gap(l: &Line<'_>) -> bool { l.spans.iter().all(|s| s.content.trim().is_empty()) }

impl<'a> Stack<'a> {
    pub(super) fn new() -> Self { Stack { rows: Vec::new() } }
    pub(super) fn push(&mut self, l: Line<'a>)     { self.rows.push((l, Prio::Core)); }
    pub(super) fn heading(&mut self, l: Line<'a>)  { self.rows.push((l, Prio::Heading)); }
    pub(super) fn detail(&mut self, l: Line<'a>)   { self.rows.push((l, Prio::Detail)); }
    pub(super) fn minor(&mut self, l: Line<'a>)    { self.rows.push((l, Prio::Minor)); }
    pub(super) fn ornament(&mut self, l: Line<'a>) { self.rows.push((l, Prio::Ornament)); }
    pub(super) fn gap(&mut self)                   { self.rows.push((Line::raw(""), Prio::Core)); }

    /// Shed rows until the stack fits `avail`, cheapest class first, and return what
    /// is left. The caller still runs `chrome::fit_spacers` on the result.
    ///
    /// Spacers are counted as already spent: `collapse_spacers` will take every one
    /// of them back for free afterwards, so they must not buy an ornament's life.
    pub(super) fn fit(mut self, avail: usize) -> Vec<Line<'a>> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
