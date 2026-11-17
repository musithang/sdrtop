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

pub mod command;
pub mod mode;
pub mod testimony;

use std::sync::{Arc, Mutex, MutexGuard};

use crate::hardware::SdrDevice;
use crate::state::SdrMetrics;

/// The one handle to everything that measures. Cheap to clone: every clone
/// is the same engine.
#[derive(Clone)]
pub struct Engine {
    state: Arc<Mutex<SdrMetrics>>,
    device: Arc<dyn SdrDevice>,
}

impl Engine {
    pub fn new(state: Arc<Mutex<SdrMetrics>>, device: Arc<dyn SdrDevice>) -> Self {
        Self { state, device }
    }

    /// Everything the engine knows, as of now: the copy a frame is drawn from.
    #[cfg(test)]
    pub fn snapshot(&self) -> SdrMetrics {
        self.lock().clone()
    }

    /// Change the shared state directly. For what is not yet a command; every
    /// use is one the commands have still to absorb.
    pub fn with_state<R>(&self, f: impl FnOnce(&mut SdrMetrics) -> R) -> R {
        f(&mut self.lock())
    }

    /// The radio itself. Only for what measures the radio's own calls (the
    /// retune timing bench), never to change a setting: settings are
    /// [`command::Command`]s.
    pub fn device(&self) -> &Arc<dyn SdrDevice> {
        &self.device
    }

    /// The state, locked. A poisoned lock is recovered, never propagated: a
    /// panic on one thread must not take every reader down with it.
    fn lock(&self) -> MutexGuard<'_, SdrMetrics> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}
