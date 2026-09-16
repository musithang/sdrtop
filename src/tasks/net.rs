// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `net_survey_task` - walks the 2.4 GHz band while the NET section is open and
//! the mode is survey.
//!
//! The same shape as [`super::sweep`] and for the same reasons: retune, settle,
//! dwell, advance, and steer nothing else. It runs no DSP of its own - the
//! worker in `signal::net` is already measuring whatever the radio is pointed
//! at, so this task's whole job is deciding where that is.
//!
//! **The plan is not here.** Where to point and for how long is
//! [`crate::signal::net::survey::Plan`], which is plain arithmetic over plain
//! data and is asserted with no radio. What is left in this file is the part
//! that cannot be tested that way: the device calls, the sleeps, and putting the
//! tuning back afterwards.
//!
//! It cannot fight the frequency sweep over the tuner, because the two are in
//! different sections and only one section is on screen at a time. It can fight
//! the user, who may retune while a pass is running; the pass wins until the
//! mode is switched to lock, which is what lock is for.
//!
//! **B11: one preset gets a different plan, not a different mode.** On the
//! `net_ble` preset, survey means rotating `signal::ble::channel::
//! advertising_channels_hz`'s three fixed channels rather than covering the
//! wideband occupancy grid `signal::net::survey::Plan` computes - design
//! section 13.1's survey-versus-lock claim still applies unchanged, it is
//! only the positions that differ.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::hardware::SdrDevice;
use crate::signal::net::survey::{Plan, DWELL, SETTLE};
use crate::state::{NetMode, SdrMetrics};

/// How often to look again when there is nothing to do.
const IDLE_POLL: Duration = Duration::from_millis(100);

pub fn spawn_net_survey_task(state: Arc<Mutex<SdrMetrics>>, device: Arc<dyn SdrDevice>) {
    tokio::spawn(async move {
        let mut surveying = false;
        // Alternate passes walk one cell along, so no megahertz stays in a
        // position's DC shadow. See `Plan::DODGE_HZ`.
        let mut pass = 0u64;
        // Said once, not once a poll.
        let mut refused = false;
        // Tracked separately from `surveying`, which only says whether *some*
        // survey is running: switching presets mid-survey, from the wideband
        // one to BLE's rotation or back, needs its own announcement even
        // though a survey was already under way either side of the switch.
        let mut was_ble = false;

        loop {
            let (active, span_hz, rate_hz, tuned, is_ble) = {
                let m = state.lock().unwrap_or_else(|e| e.into_inner());
                let span = if m.radio.bb_filter_hz > 0 {
                    (m.radio.bb_filter_hz as f64).min(m.radio.config_sample_rate)
                } else {
                    m.radio.config_sample_rate
                };
                (
                    m.ui.is_net_section() && m.net.mode == NetMode::Survey && m.radio.hw_streaming,
                    span,
                    m.radio.config_sample_rate,
                    m.radio.frequency,
                    m.ui.active_preset == "net_ble",
                )
            };

            if !active {
                // Where the radio belongs is `NetState::end`'s answer, not this
                // task's: locking means stay here, leaving the section means go
                // back where the survey found you, and quitting mid-pass has to
                // reach the same answer without another iteration of this loop.
                if surveying {
                    surveying = false;
                    let exit = {
                        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                        m.net.end(tuned)
                    };
                    let _ = device.set_frequency(exit.tune_hz);
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.radio.frequency = exit.tune_hz;
                    m.push_log(if exit.locked {
                        format!("NET locked to {:.3} MHz", exit.tune_hz as f64 / 1e6)
                    } else {
                        format!(
                            "NET survey stopped, back to {:.3} MHz",
                            exit.tune_hz as f64 / 1e6
                        )
                    });
                }
                tokio::time::sleep(IDLE_POLL).await;
                continue;
            }

            // **B11's rotation is a different plan under the same mode, not a
            // different mode.** Design section 13.1 makes survey-or-lock part
            // of what a reading claims, and that claim is unaffected by
            // *which* positions a survey visits - a BLE preset asking Survey
            // to rotate the three advertising channels instead of the whole
            // band is still "hop across the band, dwell, gather statistics";
            // it just has a different band to cover. Kept out of `Plan`
            // itself, whose own `covered()` answers a cell-occupancy question
            // this rotation has no matching answer for.
            if is_ble {
                if !was_ble {
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    if !surveying {
                        m.net.pre_survey_hz = Some(tuned);
                    }
                    m.net.survey_refused = None;
                    let hops = crate::signal::ble::channel::advertising_channels_hz();
                    m.push_log(format!(
                        "NET survey: BLE advertising rotation, {} channels (37/38/39), \
                         {} ms a pass, 1/{} dwell each",
                        hops.len(),
                        ((SETTLE + DWELL) * hops.len() as u32).as_millis(),
                        hops.len()
                    ));
                }
                surveying = true;
                was_ble = true;
                for hz in crate::signal::ble::channel::advertising_channels_hz() {
                    {
                        let m = state.lock().unwrap_or_else(|e| e.into_inner());
                        if !m.ui.is_net_section()
                            || m.net.mode != NetMode::Survey
                            || m.ui.active_preset != "net_ble"
                        {
                            break;
                        }
                    }
                    let _ = device.set_frequency(hz);
                    {
                        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                        m.radio.frequency = hz;
                    }
                    tokio::time::sleep(SETTLE + DWELL).await;
                }
                pass = pass.wrapping_add(1);
                continue;
            }
            was_ble = false;

            let plan = Plan::for_span(span_hz, pass);
            let bins = crate::signal::net::scan::bins_for(rate_hz);
            let refusal = refusal(&plan, rate_hz, bins, span_hz);
            {
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                refused = apply_refusal(&mut m, &refusal, refused);
            }
            if refusal.is_some() {
                tokio::time::sleep(IDLE_POLL).await;
                continue;
            }
            if !surveying {
                surveying = true;
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.net.pre_survey_hz = Some(tuned);
                // The coverage is logged rather than assumed. A plan is only a
                // plan if its positions between them see the whole band, and a
                // radio whose usable span left a gap would otherwise report that
                // stretch as unobserved for ever with nothing saying why.
                let covered = plan.covered(rate_hz, bins).iter().filter(|c| **c).count();
                m.push_log(format!(
                    "NET survey: {} positions of {:.1} MHz, {} ms a pass, {covered} of {} MHz covered",
                    plan.hops.len(),
                    span_hz / 1e6,
                    plan.cycle().as_millis(),
                    crate::signal::net::occupancy::CELLS
                ));
            }

            for hz in &plan.hops {
                {
                    let m = state.lock().unwrap_or_else(|e| e.into_inner());
                    if !m.ui.is_net_section() || m.net.mode != NetMode::Survey {
                        break;
                    }
                }
                let _ = device.set_frequency(*hz);
                {
                    // The worker reads `radio.frequency` to know which cells it
                    // is looking at, so this has to be the position and not the
                    // user's last tuning, and it has to be set before the dwell
                    // rather than after it.
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.radio.frequency = *hz;
                }
                tokio::time::sleep(SETTLE + DWELL).await;
            }
            pass = pass.wrapping_add(1);
        }
    });
}

/// Put the decision on the state, and say it in the log the first time.
///
/// **The state is assigned, not set-and-cleared.** The first version set it on
/// refusal and cleared it further down, which is one edit away from leaving a
/// stale refusal on screen after the survey restarted - and no test would have
/// noticed, because the task's side of this had none. Assigning from the
/// decision makes the stale case unrepresentable.
///
/// Returns whether the refusal has now been logged, so the caller says it once
/// rather than ten times a second.
fn apply_refusal(
    m: &mut crate::state::SdrMetrics,
    refusal: &Option<String>,
    already_logged: bool,
) -> bool {
    m.net.survey_refused = refusal.clone();
    match refusal {
        Some(why) if !already_logged => {
            m.push_log(format!("NET survey: {why}; lock to a channel instead"));
            true
        }
        Some(_) => true,
        None => false,
    }
}

/// Why this plan cannot be surveyed, or `None` when it can.
///
/// **A receiver too narrow to see past its own oscillator has positions to visit
/// and nothing to learn at any of them.** The gate asked whether the radio can
/// receive the cheapest mode; whether it can survey a band is a different
/// question, and this is where it is answered.
///
/// Pure, and lifted out of the task, because the task is sleeps and device calls
/// while this is the decision a user sees the consequence of - and the task's
/// side of it had no test at all until a deliberate break walked through it.
fn refusal(plan: &Plan, rate_hz: f64, bins: usize, span_hz: f64) -> Option<String> {
    if plan.hops.is_empty() || !plan.covers_anything(rate_hz, bins) {
        return Some(format!(
            "{:.1} MHz of view is too narrow to hold a whole megahertz clear of the \
             local oscillator",
            span_hz / 1e6
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::net::scan::bins_for;

    /// The case the user hit: a radio set to two megasamples.
    ///
    /// Its baseband filter is narrower still, so the usable view is 1.8 MHz, and
    /// a whole megahertz clear of the oscillator does not fit in it.
    #[test]
    fn a_two_megasample_receiver_is_refused_and_the_sentence_says_why() {
        let span = 1_800_000.0;
        let rate = 2_000_000.0;
        let why = refusal(&Plan::for_span(span, 0), rate, bins_for(rate), span)
            .expect("1.8 MHz cannot be surveyed");
        // The figure the user can act on, in the units the header shows it in.
        assert!(why.contains("1.8 MHz"), "{why}");
        assert!(why.contains("oscillator"), "{why}");
    }

    /// And a receiver that can survey is not refused.
    #[test]
    fn a_wide_receiver_is_not_refused() {
        for (span, rate) in [(18_000_000.0, 20_000_000.0), (4_000_000.0, 4_400_000.0)] {
            assert_eq!(
                refusal(&Plan::for_span(span, 0), rate, bins_for(rate), span),
                None,
                "{span} Hz of view should survey"
            );
        }
    }

    /// **A refusal that ends leaves nothing behind.**
    ///
    /// The failure this prevents is the quiet one: the survey starts working
    /// again and the panel goes on explaining why it cannot. Asserted as
    /// behaviour rather than as the shape of the code, because the shape can be
    /// kept while the behaviour is lost.
    #[test]
    fn a_refusal_that_ends_clears_the_sentence_with_it() {
        let mut m = crate::state::SdrMetrics::fixture();
        let why = Some("1.8 MHz of view is too narrow".to_string());

        let logged = apply_refusal(&mut m, &why, false);
        assert_eq!(m.net.survey_refused, why);
        assert!(logged);
        let said = m.ui.log.len();

        // Still refused: the state keeps saying so, the log does not repeat it.
        let logged = apply_refusal(&mut m, &why, logged);
        assert_eq!(m.net.survey_refused, why);
        assert_eq!(m.ui.log.len(), said, "the log repeated itself");

        // The rate is widened and the survey runs again.
        let logged = apply_refusal(&mut m, &None, logged);
        assert_eq!(
            m.net.survey_refused, None,
            "the panel would still be explaining a refusal that is over"
        );
        assert!(
            !logged,
            "and a later refusal is said again rather than swallowed"
        );

        // Refused once more, and it is said again.
        apply_refusal(&mut m, &why, logged);
        assert!(m.ui.log.len() > said);
    }
}
