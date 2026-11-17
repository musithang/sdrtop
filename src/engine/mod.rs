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

pub mod mode;
pub mod testimony;
