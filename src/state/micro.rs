//! The micro ecosystem views, and the map between them and their layouts.
//!
//! This used to be a state machine: `[0]` walked Main, Signal, Gain, Health and
//! back, and the position was a field. The views now have number keys inside the
//! Micro section like every other layout, so what is left is the naming, and
//! [`MicroView::from_preset`] is what lets the footer read the current view off
//! the active preset instead of keeping a second copy of it.

/// The micro ecosystem views. `Sweep` counts toward the family size only while a
/// frequency sweep is active.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MicroView {
    #[default]
    Main,
    Signal,
    Gain,
    Health,
    Sweep,
}

impl MicroView {
    /// The family, in display order. `Sweep` is appended only when a sweep is
    /// active, which is what makes the footer's `N/M` read 4 or 5.
    fn order(sweep_active: bool) -> &'static [MicroView] {
        use MicroView::*;
        if sweep_active {
            &[Main, Signal, Gain, Health, Sweep]
        } else {
            &[Main, Signal, Gain, Health]
        }
    }

    /// The layout preset name this view switches to.
    pub fn preset_name(self) -> &'static str {
        match self {
            MicroView::Main => "micro_main",
            MicroView::Signal => "micro_signal",
            MicroView::Gain => "micro_gain",
            MicroView::Health => "micro_health",
            MicroView::Sweep => "micro_sweep",
        }
    }

    /// The view a preset name names, if it names one.
    ///
    /// The inverse of [`MicroView::preset_name`], and the reason the view needs
    /// no field of its own: the active preset already knows, so a second copy
    /// could only ever disagree with it.
    pub fn from_preset(name: &str) -> Option<Self> {
        [
            MicroView::Main,
            MicroView::Signal,
            MicroView::Gain,
            MicroView::Health,
            MicroView::Sweep,
        ]
        .into_iter()
        .find(|v| v.preset_name() == name)
    }

    /// 1-based position in the family (Main = 1 … Sweep = 5). Stable regardless
    /// of whether sweep is active, since Sweep is always last.
    pub fn position(self) -> usize {
        match self {
            MicroView::Main => 1,
            MicroView::Signal => 2,
            MicroView::Gain => 3,
            MicroView::Health => 4,
            MicroView::Sweep => 5,
        }
    }

    /// Number of views currently in the family (4, or 5 while sweeping).
    pub fn total(sweep_active: bool) -> usize {
        Self::order(sweep_active).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_reflects_sweep() {
        assert_eq!(MicroView::total(false), 4);
        assert_eq!(MicroView::total(true), 5);
    }

    #[test]
    fn position_is_one_based_and_stable() {
        assert_eq!(MicroView::Main.position(), 1);
        assert_eq!(MicroView::Health.position(), 4);
        assert_eq!(MicroView::Sweep.position(), 5);
    }

    #[test]
    fn default_is_main() {
        assert_eq!(MicroView::default(), MicroView::Main);
    }

    #[test]
    fn preset_names_match_views() {
        assert_eq!(MicroView::Main.preset_name(), "micro_main");
        assert_eq!(MicroView::Signal.preset_name(), "micro_signal");
        assert_eq!(MicroView::Health.preset_name(), "micro_health");
    }

    /// The active preset is the single source of truth for which micro view is
    /// on screen.
    ///
    /// Two copies of that fact was one too many: the old `[0]` cycle set both, and
    /// every other route into a micro preset set only the preset, so the footer
    /// could advertise the keys of a view you were not looking at.
    #[test]
    fn every_view_round_trips_through_its_preset_name() {
        for view in [
            MicroView::Main,
            MicroView::Signal,
            MicroView::Gain,
            MicroView::Health,
            MicroView::Sweep,
        ] {
            assert_eq!(MicroView::from_preset(view.preset_name()), Some(view));
        }
    }

    /// A layout that is not a micro view names none, rather than defaulting to
    /// one and quietly claiming the deck is in micro mode.
    #[test]
    fn a_non_micro_preset_names_no_view() {
        for name in ["lab_iq", "spectrum", "command_rail", "observer", ""] {
            assert_eq!(MicroView::from_preset(name), None, "{name}");
        }
    }
}
