// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The engine: everything that measures, with nothing that draws.
//!
//! The radio, the intake, the workers, the shared state and the recorder run
//! here, and the screen is one client of them. A headless instance on a
//! server and any other front end see exactly what the terminal sees,
//! because they all ask the same engine the same way.
//!
//! **Why a module and not a crate of its own.** Splitting the source into a
//! library and a binary would move every `pub(crate)` seam into a public API
//! with a stability promise, and change how a release is cut and published.
//! The same separation is held here by a test instead:
//! `source_tests::the_engine_never_imports_the_ui` in `main.rs` fails on any
//! line under `src/engine/` that names the drawing libraries, the panels or
//! the key handling. While that test holds, moving this module into a crate
//! of its own is a mechanical change, whenever there is a reason for one.

pub mod boot;
pub mod command;
pub mod events;
pub mod mode;
pub mod option;
pub mod start;
pub mod testimony;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod view;

use std::sync::{Arc, Mutex, MutexGuard};

use crate::hardware::SdrDevice;
use crate::state::SdrMetrics;

/// The one handle to everything that measures. Cheap to clone: every clone
/// is the same engine.
#[derive(Clone)]
pub struct Engine {
    state: Arc<Mutex<SdrMetrics>>,
    /// Shared by every clone, so a radio opened again is the one every
    /// client's commands reach. Read for the length of one call, never held
    /// across one.
    device: Arc<std::sync::RwLock<Arc<dyn SdrDevice>>>,
}

impl Engine {
    pub fn new(state: Arc<Mutex<SdrMetrics>>, device: Arc<dyn SdrDevice>) -> Self {
        Self {
            state,
            device: Arc::new(std::sync::RwLock::new(device)),
        }
    }

    /// Everything the engine knows, as of now: the copy a frame is drawn from.
    pub fn snapshot(&self) -> SdrMetrics {
        self.lock().clone()
    }

    /// Change the shared state directly. For what is not yet a command; every
    /// use is one the commands have still to absorb.
    pub fn with_state<R>(&self, f: impl FnOnce(&mut SdrMetrics) -> R) -> R {
        f(&mut self.lock())
    }

    /// A new client of the engine's events, with room for `capacity` unread.
    pub fn subscribe(&self, capacity: usize) -> events::Subscription {
        self.lock().events.subscribe(capacity)
    }

    /// The radio itself. Only for what measures the radio's own calls (the
    /// retune timing bench), never to change a setting: settings are
    /// [`command::Command`]s.
    pub fn device(&self) -> Arc<dyn SdrDevice> {
        Arc::clone(&self.device.read().unwrap_or_else(|e| e.into_inner()))
    }

    /// Tell the radio what the one before it was set to, as `was` holds
    /// it: its rate, its frequency, its gains, its boost and its own options,
    /// by the commands a client would send, so each is answered as one is.
    /// For a radio opened again after it went away. The refusals, each with
    /// what was refused, are returned; nothing else is said.
    pub fn restore(&self, was: &SdrMetrics) -> Vec<String> {
        let mut commands = vec![
            (
                "rate",
                command::Command::SampleRate {
                    hz: was.radio.config_sample_rate,
                },
            ),
            (
                "frequency",
                command::Command::Tune {
                    hz: was.radio.frequency,
                },
            ),
            (
                "gains",
                command::Command::Gains {
                    values: was.radio.gains.clone(),
                },
            ),
        ];
        if was.caps.gain.boost().is_some() {
            commands.push((
                "boost",
                command::Command::Boost {
                    on: was.radio.amp_enabled,
                },
            ));
        }
        for option in was.device_options.iter() {
            commands.push((
                "option",
                command::Command::DeviceOption {
                    id: option.id.clone(),
                    choice: option.selected_choice.clone(),
                },
            ));
        }
        commands
            .into_iter()
            .filter_map(|(what, c)| match self.apply(c) {
                command::Reply::Refused { reason } => Some(format!("{what}: {reason}")),
                command::Reply::Applied { .. } => None,
            })
            .collect()
    }

    /// Put `device` in the place of the radio this engine drove, for every
    /// clone at once: the same radio opened again after it went away.
    pub fn replace_device(&self, device: Arc<dyn SdrDevice>) {
        *self.device.write().unwrap_or_else(|e| e.into_inner()) = device;
    }

    /// Put the tuner back where a scanner found it, before the session ends.
    ///
    /// A frequency sweep parks the radio at each position in turn and writes
    /// each one to `radio.frequency`, because that is the field the FFT worker
    /// stamps its frames with. The sweep task restores the interrupted tuning
    /// when it notices `sweep.active` has gone false, but ending the session
    /// never gives it another iteration, and the save then writes out
    /// whichever position the scan was parked on: the next start opened
    /// somewhere in the middle of the swept band, one band-width further along
    /// each time. The NET survey does the same for the same reason, so it
    /// gives the tuner back here too. Harmless on a state that did neither.
    pub fn hand_back_the_tuner(&self) {
        let moved = {
            let mut m = self.lock();
            let was = m.radio.frequency;
            m.radio.frequency = m.sweep.end(was).tune_hz;
            // Second, because either may have moved the radio but never both:
            // they belong to different views and only one is computed.
            let after_sweep = m.radio.frequency;
            m.radio.frequency = m.net.end(after_sweep).tune_hz;
            // Against where the radio actually was, not against whatever the
            // first of the two handed on.
            (m.radio.frequency != was).then_some(m.radio.frequency)
        };
        // Only when a scanner had moved the radio: a retune to where it
        // already is would be one more device call during teardown for
        // nothing. With no lock held, as everywhere else.
        if let Some(hz) = moved {
            let _ = self.device().set_frequency(hz);
        }
    }

    /// The state, locked. A poisoned lock is recovered, never propagated: a
    /// panic on one thread must not take every reader down with it.
    fn lock(&self) -> MutexGuard<'_, SdrMetrics> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    /// **A radio opened again is told what the one before was set to**, by
    /// the commands a client sends: the new radio is tuned where the old one
    /// was, and swapping it in reaches every clone of the engine.
    #[test]
    fn a_radio_opened_again_gets_the_old_ones_settings() {
        use super::testing::{engine_on, Bench};
        use std::sync::Arc;
        let (engine, old) = engine_on(Bench::new());
        let clone = engine.clone();
        engine.apply(super::command::Command::Tune { hz: 433_920_000 });
        assert_eq!(old.tuned.lock().unwrap().last(), Some(&433_920_000));
        let was = engine.snapshot();

        let fresh = Arc::new(Bench::new());
        engine.replace_device(Arc::clone(&fresh) as Arc<dyn crate::hardware::SdrDevice>);
        let refused = clone.restore(&was);
        assert!(refused.is_empty(), "{refused:?}");
        assert_eq!(fresh.tuned.lock().unwrap().last(), Some(&433_920_000));
        assert!(
            Arc::ptr_eq(
                &(clone.device() as Arc<dyn crate::hardware::SdrDevice>),
                &(fresh as Arc<dyn crate::hardware::SdrDevice>)
            ),
            "the clone drives the new radio"
        );
    }

    /// **Every scanner that parks the radio gives the tuner back on quit.**
    ///
    /// Two now do: the frequency sweep and the NET survey. Both write their
    /// current position into `radio.frequency` because that is the field the FFT
    /// worker stamps frames with, and `save_config` persists that field, so a
    /// scanner missing from this function reopens the app somewhere in the
    /// middle of its band - one position further along each time.
    ///
    /// Read as source text, as the structural tests elsewhere are: adding
    /// a third scanner and forgetting this line compiles perfectly, and the
    /// symptom shows up a week later as "the app forgot where I was tuned".
    #[test]
    fn every_scanner_gives_the_tuner_back_on_quit() {
        let body = include_str!("mod.rs")
            .split_once("pub fn hand_back_the_tuner(&self) {")
            .expect("hand_back_the_tuner has been renamed")
            .1
            .split_once("\n    fn ")
            .expect("hand_back_the_tuner no longer ends")
            .0;
        for scanner in ["m.sweep.end(", "m.net.end("] {
            assert!(
                body.contains(scanner),
                "{scanner} is missing: quitting mid-scan will save the scan position"
            );
        }
    }
}
