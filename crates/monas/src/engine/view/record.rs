// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The IQ recorder: the recording running now, or the last one to end, as
//! the recorder reports it once a second and when it ends.

use serde::Serialize;

use super::{Group, Reading};
use crate::engine::testimony::{Staleness, Testimony};
use crate::state::SdrMetrics;

/// One recording's account.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Recording {
    /// The `.sigmf-data` file's name; its `.sigmf-meta` sits beside it.
    pub file: String,
    pub running: bool,
    pub rate_hz: f64,
    pub pairs: u64,
    pub bytes: u64,
    /// Pairs missing from the file, of every cause.
    pub lost: u64,
    /// The fraction of its rate the radio did not deliver without saying
    /// so, once that is past the recording's tolerance.
    pub short: Option<f64>,
    /// Drops the radio counted in itself while recording, where it keeps
    /// such a count.
    pub radio_drops: Option<u64>,
    /// Why it ended; `None` while it runs.
    pub ended: Option<String>,
}

/// The recorder's group. Never stale: it is an account of files, not a
/// measurement, and the recorder writes it itself. Undated, because the
/// recorder stamps no time on it; the `.sigmf-meta` beside the file carries
/// the recording's own times.
pub fn record(m: &SdrMetrics) -> Group<Reading<Recording>> {
    Group {
        testimony: Testimony::resolve(m, Staleness::Never, None, None),
        readings: Reading::of(
            m.record.current.as_ref().map(|r| Recording {
                file: r
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                running: r.ended.is_none(),
                rate_hz: r.rate_hz,
                pairs: r.pairs,
                bytes: r.bytes,
                lost: r.lost,
                short: r.short,
                radio_drops: r.radio_drops,
                ended: r.ended.clone(),
            }),
            || "nothing recorded this session".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RecordProgress;

    #[test]
    fn a_recording_is_reported_by_its_file_name_as_the_recorder_wrote_it() {
        let mut m = SdrMetrics::fixture();
        assert_eq!(
            record(&m).readings.absent(),
            Some("nothing recorded this session")
        );
        m.record.current = Some(RecordProgress {
            path: "/home/someone/.local/share/sdrtop/iq-20260927-101500.sigmf-data".into(),
            rate_hz: 2e6,
            pairs: 4_000_000,
            bytes: 8_000_000,
            lost: 12,
            short: None,
            radio_drops: Some(0),
            ended: None,
        });
        let r = record(&m).readings.value().cloned().unwrap();
        assert_eq!(r.file, "iq-20260927-101500.sigmf-data");
        assert!(r.running);
        assert_eq!(r.lost, 12);
    }
}
