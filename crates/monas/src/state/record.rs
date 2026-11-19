// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the IQ recorder is doing, for the header and the log.
//!
//! Written by the recorder's own thread once a second and when it ends, so the
//! UI reads a recording's progress the way it reads everything else: from the
//! snapshot, never from the writer.

use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct RecordState {
    /// The recording running now, or the last one to end.
    pub current: Option<RecordProgress>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordProgress {
    /// The `.sigmf-data` file.
    pub path: PathBuf,
    pub rate_hz: f64,
    pub pairs: u64,
    pub bytes: u64,
    /// Pairs missing from the file, of every cause.
    pub lost: u64,
    /// The fraction of its rate the radio did not deliver without reporting
    /// it, once that is past the recording's tolerance.
    pub short: Option<f64>,
    /// Drops the radio counted in itself while recording, when it keeps
    /// such a count.
    pub radio_drops: Option<u64>,
    /// Why it ended; `None` while it runs.
    pub ended: Option<String>,
}
