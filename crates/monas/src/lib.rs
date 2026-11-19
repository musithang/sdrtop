// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Monas, the engine inside sdrtop: the radio, the workers that measure what
//! it hears, and the state they write. A front end opens a radio through
//! [`engine::start`], reads the state through an [`engine::Engine`] and asks
//! for what it wants computed with an [`engine::mode::Mode`]; it never draws
//! anything here, and nothing here knows who its front ends are.
//!
//! Published only so that the programs built on it can be installed from
//! crates.io. It has no stability promise of its own: its version is
//! sdrtop's, and a version of it works with the same version of sdrtop and no
//! other.

// Monas's documentation is its developers', so a public item's comment may
// point at the private one that explains it. rustdoc still resolves every
// such link, and a broken one fails the doc build as it always did.
#![allow(rustdoc::private_intra_doc_links)]

pub mod config;
pub mod engine;
pub mod export;
pub mod geo;
pub mod hardware;
pub mod host;
pub mod signal;
pub mod state;
pub mod tasks;
