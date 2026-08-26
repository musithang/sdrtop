//! Micro ecosystem view state machine, cycled with the `[0]` key.
//!
//! Lives in the state layer (not `app`) so input handlers can advance it through
//! the shared `SdrMetrics` they already hold, and the footer can read the
//! current position without the engine. The `[0]` handler in `app::input` owns
//! the entry/advance policy; this type only defines the cycle.

/// The micro ecosystem views. `Sweep` is only part of the cycle while a
/// frequency sweep is active (a future capability); until then the cycle is
/// Main → Signal → Gain → Health → Main.
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
    /// The cycle order. `Sweep` is appended only when a sweep is active.
    fn order(sweep_active: bool) -> &'static [MicroView] {
        use MicroView::*;
        if sweep_active {
            &[Main, Signal, Gain, Health, Sweep]
        } else {
            &[Main, Signal, Gain, Health]
        }
    }

    /// The next view in the cycle, wrapping around.
    pub fn next(self, sweep_active: bool) -> Self {
        let order = Self::order(sweep_active);
        let idx = order.iter().position(|&v| v == self).unwrap_or(0);
        order[(idx + 1) % order.len()]
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

    /// Short label for the footer (`main`, `signal`, …).
    pub fn label(self) -> &'static str {
        match self {
            MicroView::Main => "main",
            MicroView::Signal => "signal",
            MicroView::Gain => "gain",
            MicroView::Health => "health",
            MicroView::Sweep => "sweep",
        }
    }

    /// 1-based position in the cycle (Main = 1 … Sweep = 5). Stable regardless
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

    /// Number of views currently in the cycle (4, or 5 while sweeping).
    pub fn total(sweep_active: bool) -> usize {
        Self::order(sweep_active).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_cycles_without_sweep() {
        assert_eq!(MicroView::Main.next(false), MicroView::Signal);
        assert_eq!(MicroView::Signal.next(false), MicroView::Gain);
        assert_eq!(MicroView::Gain.next(false), MicroView::Health);
        // Wraps back to Main, skipping Sweep when not active.
        assert_eq!(MicroView::Health.next(false), MicroView::Main);
    }

    #[test]
    fn next_includes_sweep_when_active() {
        assert_eq!(MicroView::Health.next(true), MicroView::Sweep);
        assert_eq!(MicroView::Sweep.next(true), MicroView::Main);
    }

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
}
