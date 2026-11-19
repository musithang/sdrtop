// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What a reading says about itself: when it was measured, whether it is
//! still live, whether the feed lost samples inside it, or why there is no
//! reading at all.
//!
//! POLICY's rules about a number on screen (all testimony is dated, what
//! cannot be asked is refused, one witness one account) hold for a number
//! sent anywhere else too. So the verdicts are decided here, once, from the
//! engine's state, and the terminal and every other client read the same
//! verdict instead of each working it out from the ingredients.

use serde::Serialize;

use crate::state::SdrMetrics;

/// What makes a reading go stale: on screen, when the frame tags a panel's
/// title `[STALE]` and cools its border; over the API, `stale` in its
/// [`Testimony`].
///
/// Declaring this instead of computing it per reading is what makes the
/// project's "a frozen number must never read as live" rule enforceable in
/// one place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Staleness {
    /// Stale whenever the radio is not streaming. For anything read from
    /// hardware counters: timing, drops, gain staging, IQ balance.
    NotStreaming,
    /// Stale when the newest FFT frame exceeds the device's trace-age limit, or
    /// there is no frame yet. For anything derived from the spectrum.
    FftAge,
    /// Never stale. For panels that show configuration rather than measurement.
    Never,
}

impl Staleness {
    /// Resolve the rule against a metrics snapshot.
    pub fn resolve(self, state: &SdrMetrics) -> bool {
        let age = state
            .waterfall
            .last_fft
            .as_ref()
            .map(|frame| frame.timestamp.elapsed().as_millis());
        let stale = self.decide(state.radio.hw_streaming, age, state.caps.trace_stale_ms);
        stale
            || (self == Staleness::FftAge
                && state.caps.acquisition == crate::hardware::AcquisitionKind::PowerTrace
                && !state.radio.hw_streaming)
    }

    /// The rule itself, on plain inputs: `fft_age_ms` is `None` when no frame has
    /// arrived yet. Split out from [`resolve`](Self::resolve) so the decision can
    /// be tested without building a whole metrics snapshot.
    fn decide(self, streaming: bool, fft_age_ms: Option<u128>, trace_stale_ms: u128) -> bool {
        match self {
            Staleness::NotStreaming => !streaming,
            Staleness::FftAge => fft_age_ms
                .map(|milliseconds| milliseconds > trace_stale_ms)
                .unwrap_or(true),
            Staleness::Never => false,
        }
    }
}

/// How far back into the sample feed a reading reaches, declared so the
/// engine can say, in one place, whether the feed lost anything inside that
/// span.
///
/// Every count in the NET section is a lower bound once the feed has dropped
/// samples, because the three ways a block goes missing are invisible
/// downstream. But "the feed lost something" is only a caveat on the numbers
/// that span the loss: a census accumulated over the session is undercounted
/// by a drop ten minutes ago, the duty cycle of the dwell just finished is
/// not. So a reading declares the span its numbers cover, and the engine
/// caveats it (on screen, the `[FEED LOSS]` tag) only when the last loss
/// falls inside it, never computed by whoever shows it, the same rule as
/// [`Staleness`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FeedSpan {
    /// Accumulated for as long as the feed's own account runs: totals, the
    /// census, per-channel counts.
    Session,
    /// Measured over the most recent stretch of this length only: a dwell, a
    /// scrolling window.
    Window(std::time::Duration),
}

impl FeedSpan {
    /// Resolve against a metrics snapshot: did the feed lose anything inside
    /// this span?
    pub fn resolve(self, state: &SdrMetrics) -> bool {
        self.decide(state.net.health.last_loss.map(|t| t.elapsed()))
    }

    /// The rule on plain inputs: `since_loss` is how long ago the feed last
    /// lost anything, `None` if it never has.
    fn decide(self, since_loss: Option<std::time::Duration>) -> bool {
        match (self, since_loss) {
            (_, None) => false,
            (FeedSpan::Session, Some(_)) => true,
            (FeedSpan::Window(span), Some(age)) => age <= span,
        }
    }
}

/// What a reading says about itself. See the module comment.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Testimony {
    /// When it was measured, in milliseconds since the Unix epoch: for a
    /// reader on another machine. Reported as stamped; never used to decide
    /// staleness, which runs on the monotonic clock.
    pub at_unix_ms: Option<u64>,
    /// Whether it has stopped being live, by its [`Staleness`] rule.
    pub stale: bool,
    /// Whether the feed lost samples inside the span it covers, by its
    /// [`FeedSpan`]: its counts are then lower bounds.
    pub feed_loss: bool,
    /// Why there is no value, where there is none.
    pub absent: Option<String>,
}

impl Testimony {
    /// The verdicts for a reading that goes stale by `staleness` and covers
    /// `feed` of the sample feed (`None`: it counts nothing from the feed).
    pub fn resolve(
        state: &SdrMetrics,
        staleness: Staleness,
        feed: Option<FeedSpan>,
        at_unix_ms: Option<u64>,
    ) -> Testimony {
        Testimony {
            at_unix_ms,
            stale: staleness.resolve(state),
            feed_loss: feed.is_some_and(|span| span.resolve(state)),
            absent: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SdrMetrics;
    use std::time::{Duration, Instant};

    /// A loss is a caveat only on the numbers whose span it falls inside.
    #[test]
    fn a_feed_loss_caveats_only_the_spans_it_falls_inside() {
        let ten_min_ago = Some(Duration::from_secs(600));
        let just_now = Some(Duration::from_millis(300));
        let window = FeedSpan::Window(Duration::from_secs(20));

        // Never lost anything: nothing is caveated, whatever the span.
        assert!(!FeedSpan::Session.decide(None));
        assert!(!window.decide(None));

        // A session total spans every loss there has been.
        assert!(FeedSpan::Session.decide(ten_min_ago));
        assert!(FeedSpan::Session.decide(just_now));

        // A window spans only the recent ones: an old drop says nothing about
        // what was measured since.
        assert!(!window.decide(ten_min_ago));
        assert!(window.decide(just_now));
        assert!(
            window.decide(Some(Duration::from_secs(20))),
            "the edge is inside"
        );
    }

    #[test]
    fn staleness_rules_are_independent_of_each_other() {
        // A dead radio staleness NotStreaming, and nothing else.
        let stale_ms = crate::hardware::IQ_TRACE_STALE_MS;
        assert!(Staleness::NotStreaming.decide(false, Some(0), stale_ms));
        assert!(
            !Staleness::FftAge.decide(false, Some(0), stale_ms),
            "fresh frame, dead radio → live"
        );
        assert!(
            !Staleness::Never.decide(false, None, stale_ms),
            "Never means never"
        );

        // A streaming radio whose FFT has dried up staleness only FftAge.
        assert!(!Staleness::NotStreaming.decide(true, None, stale_ms));
        assert!(
            Staleness::FftAge.decide(true, None, stale_ms),
            "no frame yet → stale"
        );
        assert!(Staleness::FftAge.decide(true, Some(stale_ms + 1), stale_ms));
        assert!(
            !Staleness::FftAge.decide(true, Some(stale_ms), stale_ms),
            "the threshold itself is live"
        );
    }

    #[test]
    fn power_trace_staleness_uses_the_device_limit_and_rx_state() {
        let mut state = SdrMetrics::fixture().streaming().with_carrier(0.0, 20.0);
        let mut caps = (*state.caps).clone();
        caps.acquisition = crate::hardware::AcquisitionKind::PowerTrace;
        caps.trace_stale_ms = 100;
        state.caps = std::sync::Arc::new(caps);
        state.waterfall.last_fft.as_mut().unwrap().timestamp =
            Instant::now() - Duration::from_millis(50);

        assert!(!Staleness::FftAge.resolve(&state));
        state.caps = {
            let mut caps = (*state.caps).clone();
            caps.trace_stale_ms = 10;
            std::sync::Arc::new(caps)
        };
        assert!(Staleness::FftAge.resolve(&state));

        state.waterfall.last_fft.as_mut().unwrap().timestamp = Instant::now();
        state.radio.hw_streaming = false;
        assert!(Staleness::FftAge.resolve(&state));
    }

    #[test]
    fn trace_staleness_uses_the_device_limit() {
        let mut state = SdrMetrics::fixture().streaming().with_carrier(0.0, 20.0);
        std::sync::Arc::make_mut(&mut state.caps).trace_stale_ms = 60_000;
        state.waterfall.last_fft.as_mut().unwrap().timestamp =
            Instant::now() - Duration::from_secs(1);
        assert!(!Staleness::FftAge.resolve(&state));

        std::sync::Arc::make_mut(&mut state.caps).trace_stale_ms = 100;
        assert!(Staleness::FftAge.resolve(&state));

        state.waterfall.last_fft = None;
        assert!(Staleness::FftAge.resolve(&state));
    }

    #[test]
    fn trace_age_threshold_is_inclusive_for_each_device() {
        for limit in [0, 100, 500, 60_000] {
            assert!(!Staleness::FftAge.decide(false, Some(limit), limit));
            assert!(Staleness::FftAge.decide(true, Some(limit + 1), limit));
        }
    }

    /// **Staleness is decided on the monotonic clock.** A server that has just
    /// booted can have its wall clock stepped by minutes; a frame measured a
    /// moment ago is live whatever the wall clock said when it was stamped,
    /// and the stamp is reported as it was, not corrected.
    #[test]
    fn staleness_ignores_a_wall_clock_jump() {
        let state = SdrMetrics::fixture().streaming().with_carrier(0.0, 20.0);
        let said = Testimony::resolve(&state, Staleness::FftAge, None, Some(0));
        assert!(
            !said.stale,
            "a fresh frame read as stale off the wall clock"
        );
        assert_eq!(said.at_unix_ms, Some(0));
    }

    /// One testimony carries both verdicts, each from its own rule, and no
    /// span declared means no feed-loss caveat however lossy the feed.
    #[test]
    fn testimony_carries_staleness_and_feed_loss_each_from_its_rule() {
        let mut state = SdrMetrics::fixture();
        state.net.health.last_loss = Some(Instant::now());

        let said = Testimony::resolve(
            &state,
            Staleness::NotStreaming,
            Some(FeedSpan::Session),
            None,
        );
        assert!(said.stale, "the fixture's radio is not streaming");
        assert!(said.feed_loss);
        assert_eq!(said.absent, None);

        let undeclared = Testimony::resolve(&state, Staleness::Never, None, None);
        assert!(!undeclared.stale && !undeclared.feed_loss);
    }
}
