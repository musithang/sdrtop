// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! SigMF recording over the API: started and stopped by a client, written
//! by the same recorder the terminal uses, into one folder on this machine.
//!
//! **No path ever comes from the network.** A recording is named by the
//! recorder (`iq-` and the time), it goes into the configured folder, and a
//! client asking for a recording's metadata names one the folder's own
//! listing holds; nothing it sends is joined to a path. The data file stays
//! on disk: IQ over the network is not what this server is for, and fetching
//! it is scp's job.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::config::RecordSettings;
use crate::engine::command::Reply;
use crate::export::iq::Recorder;
use crate::hardware::RecordTap;
use crate::state::SdrMetrics;

/// What a client may ask of a recording: its limits, each in place of the
/// config's `[record]` value.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct Ask {
    pub seconds: Option<f64>,
    pub gigabytes: Option<f64>,
}

/// One recording in the folder.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    /// Its name, without the `.sigmf-meta` or `.sigmf-data`.
    pub name: String,
    /// The data file's size, where it is there.
    pub data_bytes: Option<u64>,
    /// Whether it is the one being written now.
    pub running: bool,
}

/// The recordings in the folder, for their route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Listing {
    /// Oldest first, by name, which is by time.
    pub recordings: Vec<Entry>,
}

/// The server's recorder and the folder it writes to.
pub struct Recordings {
    state: Arc<Mutex<SdrMetrics>>,
    tap: Arc<RecordTap>,
    dir: PathBuf,
    limits: RecordSettings,
    /// Who writes the files, as `core:recorder` names it.
    program: String,
    recorder: Mutex<Option<Recorder>>,
}

impl Recordings {
    pub fn new(
        state: Arc<Mutex<SdrMetrics>>,
        tap: Arc<RecordTap>,
        dir: PathBuf,
        limits: RecordSettings,
        program: String,
    ) -> Self {
        Self {
            state,
            tap,
            dir,
            limits,
            program,
            recorder: Mutex::new(None),
        }
    }

    fn running(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record
            .current
            .as_ref()
            .is_some_and(|p| p.ended.is_none())
    }

    /// Start a recording, with `ask`'s limits where given.
    pub fn start(&self, ask: Ask, now_unix: f64) -> Reply {
        let refused = |reason: String| Reply::Refused { reason };
        for (name, v) in [("seconds", ask.seconds), ("gigabytes", ask.gigabytes)] {
            if let Some(v) = v.filter(|v| !(v.is_finite() && *v > 0.0)) {
                return refused(format!(
                    "{name} = {v} is not a limit: give a positive number"
                ));
            }
        }
        let settings = RecordSettings {
            max_seconds: ask.seconds.unwrap_or(self.limits.max_seconds),
            max_gb: ask.gigabytes.unwrap_or(self.limits.max_gb),
        };
        let (limits, notes) = settings.limits();
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            return refused(format!("cannot use the recordings folder: {e}"));
        }
        let mut held = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
        if self.running() {
            return refused("a recording is running already".to_string());
        }
        // One that ended by itself (a limit, the radio) is let go first.
        if let Some(old) = held.take() {
            old.finish();
        }
        match Recorder::start(
            &self.state,
            &self.tap,
            &self.dir,
            limits,
            now_unix,
            &self.program,
        ) {
            Ok(recorder) => {
                *held = Some(recorder);
                let name = self
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .record
                    .current
                    .as_ref()
                    .and_then(|p| p.path.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_default();
                let mut detail = format!(
                    "recording {name}, up to {} s or {} GB",
                    limits.max_secs,
                    limits.max_bytes as f64 / 1e9
                );
                for note in notes {
                    detail.push_str(&format!("; {note}"));
                }
                Reply::Applied { detail }
            }
            Err(why) => refused(why),
        }
    }

    /// Stop the recording running, keeping every block already captured.
    pub fn stop(&self) -> Reply {
        let held = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
        match held.as_ref().filter(|_| self.running()) {
            Some(recorder) => {
                recorder.stop();
                Reply::Applied {
                    detail: "stopping, writing what was already captured".to_string(),
                }
            }
            None => Reply::Refused {
                reason: "no recording is running".to_string(),
            },
        }
    }

    /// The recordings in the folder: every name with a `.sigmf-meta`.
    pub fn list(&self) -> Listing {
        let current = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record
            .current
            .as_ref()
            .filter(|p| p.ended.is_none())
            .and_then(|p| p.path.file_stem().map(|s| s.to_string_lossy().into_owned()));
        let mut recordings: Vec<Entry> = names(&self.dir)
            .into_iter()
            .map(|name| Entry {
                data_bytes: std::fs::metadata(self.dir.join(format!("{name}.sigmf-data")))
                    .ok()
                    .map(|m| m.len()),
                running: current.as_deref() == Some(name.as_str()),
                name,
            })
            .collect();
        recordings.sort_by(|a, b| a.name.cmp(&b.name));
        Listing { recordings }
    }

    /// One recording's metadata, by a name the folder's listing holds.
    pub fn meta(&self, name: &str) -> Result<serde_json::Value, String> {
        if !names(&self.dir).iter().any(|n| n == name) {
            return Err(format!("there is no recording called {name}"));
        }
        let path = self.dir.join(format!("{name}.sigmf-meta"));
        let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read it: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("its metadata does not parse: {e}"))
    }

    /// On the way out: stop a recording and wait for its last write, so the
    /// file on disk says it finished, and why.
    pub fn finish(&self) {
        let held = self
            .recorder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(recorder) = held {
            recorder.stop();
            recorder.finish();
        }
    }
}

/// The names of the recordings in `dir`: each file ending `.sigmf-meta`,
/// less that.
fn names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    e.file_name()
                        .to_str()
                        .and_then(|n| n.strip_suffix(".sigmf-meta"))
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_790_500_000.0;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "monasd-record-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn streaming() -> Arc<Mutex<SdrMetrics>> {
        let mut m = SdrMetrics::fixture().streaming();
        m.radio.config_sample_rate = 1e6;
        Arc::new(Mutex::new(m))
    }

    fn recordings(dir: &Path, state: Arc<Mutex<SdrMetrics>>) -> Recordings {
        Recordings::new(
            state,
            Arc::new(RecordTap::default()),
            dir.to_path_buf(),
            RecordSettings::default(),
            "monasd test".to_string(),
        )
    }

    /// Started, listed as running, its metadata naming monasd, stopped; a
    /// second start while it runs and a stop with none running are refused.
    #[test]
    fn a_recording_starts_lists_and_stops() {
        let dir = scratch("cycle");
        let r = recordings(&dir, streaming());
        let Reply::Applied { detail } = r.start(
            Ask {
                seconds: Some(5.0),
                gigabytes: None,
            },
            NOW,
        ) else {
            panic!("not started");
        };
        assert!(detail.contains("up to 5 s"), "{detail}");
        assert!(matches!(
            r.start(Ask::default(), NOW + 1.0),
            Reply::Refused { .. }
        ));
        let listing = r.list();
        assert_eq!(listing.recordings.len(), 1);
        assert!(listing.recordings[0].running);
        let name = listing.recordings[0].name.clone();
        let meta = r.meta(&name).unwrap();
        assert_eq!(meta["global"]["core:recorder"], "monasd test");
        assert!(matches!(r.stop(), Reply::Applied { .. }));
        r.finish();
        assert!(matches!(r.stop(), Reply::Refused { .. }));
        assert!(!r.list().recordings[0].running);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A name from the network is never a path: only a name the folder's
    /// listing holds is read, so `..`, a slash or an absolute path finds
    /// nothing.
    #[test]
    fn a_name_from_the_network_cannot_leave_the_folder() {
        let dir = scratch("names");
        std::fs::write(dir.join("iq-1.sigmf-meta"), "{\"global\":{}}").unwrap();
        let outside = dir.parent().unwrap().join("secret.sigmf-meta");
        let r = recordings(&dir, streaming());
        assert!(r.meta("iq-1").is_ok());
        for bad in ["../secret", "/etc/passwd", "iq-1/../iq-1", "", "IQ-1"] {
            assert!(r.meta(bad).is_err(), "{bad} was read");
        }
        assert!(!outside.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A limit that is no number is refused rather than replaced: a client
    /// that asked for one should hear it was wrong.
    #[test]
    fn a_limit_that_is_no_limit_is_refused() {
        let dir = scratch("limits");
        let r = recordings(&dir, streaming());
        for ask in [
            Ask {
                seconds: Some(-1.0),
                gigabytes: None,
            },
            Ask {
                seconds: None,
                gigabytes: Some(0.0),
            },
            Ask {
                seconds: Some(f64::NAN),
                gigabytes: None,
            },
        ] {
            let Reply::Refused { reason } = r.start(ask, NOW) else {
                panic!("{ask:?} started a recording");
            };
            assert!(reason.contains("not a limit"), "{reason}");
        }
        assert!(r.list().recordings.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With the radio not streaming, the recorder's own refusal comes back.
    #[test]
    fn a_radio_not_streaming_is_not_recorded() {
        let dir = scratch("idle");
        let r = recordings(&dir, Arc::new(Mutex::new(SdrMetrics::fixture())));
        let Reply::Refused { reason } = r.start(Ask::default(), NOW) else {
            panic!("recorded a radio that was not streaming");
        };
        assert!(reason.contains("not streaming"), "{reason}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
