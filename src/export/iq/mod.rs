// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Recording the raw IQ stream to a SigMF pair, from any layout.
//!
//! Three parts, split on what each may touch:
//!
//! - [`recording`] decides everything from block positions alone: segments,
//!   what is missing and who lost it, limits. No file, no clock.
//! - [`sigmf`] turns that and a [`sigmf::Header`] into the `.sigmf-meta`
//!   document. No file either.
//! - This file is the thread that owns the files: it takes blocks from the
//!   tap in `process_block` (`hardware::RecordTap`), writes their bytes, and
//!   rewrites the metadata once a second and at the end.
//!
//! **The metadata is rewritten while the data grows**, write then rename, so a
//! crash, a pulled cable or a full disk still leaves a pair that opens and
//! says every loss up to its last second. A document written only at the end
//! would leave gigabytes of samples nobody can place.

pub mod recording;
pub mod sigmf;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::RecvTimeoutError;

use crate::hardware::record_tap::RecordMsg;
use crate::hardware::{DeviceCapabilities, RecordTap};
use crate::state::{RecordProgress, SdrMetrics};
use recording::{Limits, Recording, Stop};
use sigmf::Header;

/// How often the metadata and the progress are brought up to date.
const REFRESH: Duration = Duration::from_secs(1);

/// Room left on the disk beyond the limit, for the metadata and for the
/// rest of the system that shares the disk.
const DISK_MARGIN_BYTES: u64 = 64 << 20;

/// A running recording, as the app holds it.
pub struct Recorder {
    tap: Arc<RecordTap>,
    asked: Arc<Mutex<Option<Stop>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Recorder {
    /// Start recording what `tap` is fed into `dir`, or say why not.
    pub fn start(
        state: &Arc<Mutex<SdrMetrics>>,
        tap: &Arc<RecordTap>,
        dir: &Path,
        limits: Limits,
        now_unix: f64,
    ) -> Result<Recorder, String> {
        if tap.armed() {
            return Err("a recording is running already".to_string());
        }
        let (header, caps) = {
            let m = state.lock().unwrap_or_else(|e| e.into_inner());
            if !m.radio.rx_enabled {
                return Err("not streaming: start the radio with Space first".to_string());
            }
            (Header::from_state(&m)?, Arc::clone(&m.caps))
        };
        let needed = limits.bytes(header.sample_rate, header.bytes_per_pair) + DISK_MARGIN_BYTES;
        let free = crate::export::destination::free_bytes(dir)?;
        if free < needed {
            return Err(format!(
                "{} has {:.1} GB free and the limit needs {:.1} GB",
                dir.display(),
                free as f64 / 1e9,
                needed as f64 / 1e9
            ));
        }
        let base = crate::export::destination::base_name("iq", now_unix.floor() as i64);
        let data_path = dir.join(format!("{base}.sigmf-data"));
        let meta_path = dir.join(format!("{base}.sigmf-meta"));
        // The metadata first: if its name is taken, nothing has been written.
        let mut meta_file = crate::export::destination::create(&meta_path)?;
        let data_file = crate::export::destination::create(&data_path)?;
        let rec = Recording::new(header.sample_rate, header.bytes_per_pair, limits);
        meta_file
            .write_all(sigmf::meta(&header, &rec, None).as_bytes())
            .map_err(|e| format!("cannot write {}: {e}", meta_path.display()))?;
        drop(meta_file);

        let (tx, rx) = crossbeam_channel::unbounded();
        tap.arm(tx);
        let asked = Arc::new(Mutex::new(None));
        let writer = Writer {
            rx,
            tap: Arc::clone(tap),
            asked: Arc::clone(&asked),
            state: Arc::clone(state),
            caps,
            data: std::io::BufWriter::with_capacity(1 << 20, data_file),
            data_path: data_path.clone(),
            meta_path,
            header,
            rec,
        };
        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.record.current = Some(writer.progress(None));
            m.push_log(format!("IQ: recording to {}", data_path.display()));
        }
        let thread = std::thread::Builder::new()
            .name("iq-recorder".to_string())
            .spawn(move || writer.run())
            .map_err(|e| {
                tap.disarm();
                format!("cannot start the recorder: {e}")
            })?;
        Ok(Recorder {
            tap: Arc::clone(tap),
            asked,
            thread: Some(thread),
        })
    }

    /// Stop, keeping every block already queued: they were captured before
    /// the key was pressed.
    pub fn stop(&self) {
        let mut asked = self.asked.lock().unwrap_or_else(|e| e.into_inner());
        asked.get_or_insert(Stop::Asked);
        drop(asked);
        self.tap.disarm();
    }

    /// Wait for the writer to finish: at most what was queued, 32 MiB. Only
    /// on the way out, where a finished file is worth a moment; while the app
    /// runs it never waits for a disk.
    pub fn finish(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The recorder thread's state.
struct Writer {
    rx: crossbeam_channel::Receiver<RecordMsg>,
    tap: Arc<RecordTap>,
    asked: Arc<Mutex<Option<Stop>>>,
    state: Arc<Mutex<SdrMetrics>>,
    caps: Arc<DeviceCapabilities>,
    data: std::io::BufWriter<std::fs::File>,
    data_path: PathBuf,
    meta_path: PathBuf,
    header: Header,
    rec: Recording,
}

impl Writer {
    fn run(mut self) {
        let mut refreshed = Instant::now();
        let mut gain_cache: Option<(Vec<f64>, bool, u64, String)> = None;
        // Whether the channel closed under the writer: only a disarm does
        // that, so the tap is someone else's to manage from then on.
        let mut closed = false;
        let stop = loop {
            match self.rx.recv_timeout(REFRESH / 4) {
                Ok(RecordMsg::Refused { at }) => {
                    if let Some(stop) = self.rec.refused(&at) {
                        break stop;
                    }
                }
                Ok(RecordMsg::Radio(note)) => self.rec.radio(note),
                Ok(RecordMsg::Block {
                    at,
                    bytes,
                    gains,
                    boost,
                }) => {
                    self.tap.release(bytes.len());
                    // Keyed by the frequency too: the same settings read as
                    // dB on one side of a stage's measured band and as steps
                    // on the other.
                    let hz = at.centre_hz;
                    let fresh = !matches!(&gain_cache,
                        Some((g, b, f, _)) if *g == gains && *b == boost && *f == hz);
                    if fresh {
                        let text = sigmf::gain_text(&self.caps, &gains, boost, hz);
                        gain_cache = Some((gains, boost, hz, text));
                    }
                    let gain = gain_cache.as_ref().map(|c| c.3.as_str()).unwrap_or("");
                    let (take, stop) = self.rec.block(&at, gain);
                    let n = (take * self.header.bytes_per_pair) as usize;
                    if let Err(e) = self.data.write_all(&bytes[..n.min(bytes.len())]) {
                        break self.write_failed(e);
                    }
                    if let Some(stop) = stop {
                        break stop;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    closed = true;
                    let asked = self.asked.lock().unwrap_or_else(|e| e.into_inner()).take();
                    break asked.unwrap_or(Stop::StreamEnded);
                }
            }
            if refreshed.elapsed() >= REFRESH {
                refreshed = Instant::now();
                if let Err(e) = self.data.flush() {
                    break self.write_failed(e);
                }
                self.refresh(None);
            }
        };
        // A recording that ended itself still owns the tap, since nothing
        // can start another while it is armed. One that was disarmed may
        // share its tap with a new recording by now, and leaves it alone.
        if !closed {
            self.tap.disarm();
        }
        let stop = match self.data.flush() {
            Err(e) if !matches!(stop, Stop::Write(_)) => self.write_failed(e),
            _ => stop,
        };
        self.rec.finish(&stop);
        self.refresh(Some(&stop));
    }

    /// The disk refused a write. What counts from here is what is on it.
    fn write_failed(&mut self, e: std::io::Error) -> Stop {
        if let Ok(meta) = self.data.get_ref().metadata() {
            self.rec.written = meta.len() / self.header.bytes_per_pair;
        }
        Stop::Write(e.to_string())
    }

    fn progress(&self, stop: Option<&Stop>) -> RecordProgress {
        RecordProgress {
            path: self.data_path.clone(),
            rate_hz: self.header.sample_rate,
            pairs: self.rec.written,
            bytes: self.rec.bytes_written(),
            lost: self.rec.lost(),
            short: self.rec.shortfall(),
            radio_drops: self.rec.radio_drops,
            ended: stop.map(Stop::sentence),
        }
    }

    /// Rewrite the metadata and publish the progress.
    fn refresh(&self, stop: Option<&Stop>) {
        let text = sigmf::meta(&self.header, &self.rec, stop);
        let meta_result = write_atomically(&self.meta_path, text.as_bytes());
        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
        m.record.current = Some(self.progress(stop));
        if let Err(e) = meta_result {
            m.push_log(format!("IQ: {e}"));
        }
        if let Some(stop) = stop {
            m.push_log(format!(
                "IQ: {} after {:.1} s, {} pairs missing: {}",
                stop.sentence(),
                self.rec.written as f64 / self.header.sample_rate,
                self.rec.lost(),
                self.data_path.display()
            ));
            if let Some(sentence) = self.rec.shortfall_sentence() {
                m.push_log(format!("IQ: {sentence}"));
            }
        }
    }
}

/// Write `bytes` beside `path`, then rename over it, so a reader never sees
/// half a document.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("sigmf-meta.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::record_tap::BlockAt;
    use serde_json::Value;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sdrtop-iq-{tag}-{}-{}",
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

    fn at(first_pair: u64, pairs: u64) -> BlockAt {
        BlockAt {
            first_pair,
            pairs,
            driver_dropped: 0,
            centre_hz: 100_000_000,
            rate_hz: 1e6,
            arrived_unix: NOW + (first_pair + pairs) as f64 / 1e6,
        }
    }

    const NOW: f64 = 1_790_500_000.0;

    fn data_path_of(state: &Arc<Mutex<SdrMetrics>>) -> PathBuf {
        state
            .lock()
            .unwrap()
            .record
            .current
            .as_ref()
            .unwrap()
            .path
            .clone()
    }

    fn meta_of(data: &Path) -> Value {
        let text = std::fs::read_to_string(data.with_extension("sigmf-meta")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    /// Blocks in, a stop, and the pair on disk says exactly what went in:
    /// the bytes unchanged, the hole where the queue refused a block, and
    /// why it ended.
    #[test]
    fn a_recording_holds_the_bytes_and_says_where_they_are_missing() {
        let dir = scratch("roundtrip");
        let state = streaming();
        let tap = Arc::new(RecordTap::default());
        let rec = Recorder::start(&state, &tap, &dir, Limits::DEFAULT, NOW).unwrap();
        assert!(state
            .lock()
            .unwrap()
            .record
            .current
            .as_ref()
            .unwrap()
            .ended
            .is_none());
        let gains = state.lock().unwrap().radio.gains.clone();

        let first: Vec<u8> = (0..200u32).map(|v| v as u8).collect();
        let second: Vec<u8> = (0..200u32).map(|v| (v * 3) as u8).collect();
        tap.offer(at(0, 100), &first, gains.clone(), false);
        // A block bigger than the whole queue is refused, as a full queue
        // refuses one: by position only.
        let too_big = vec![0u8; crate::hardware::record_tap::RECORD_QUEUE_BYTES + 2];
        tap.offer(at(100, 50), &too_big, vec![], false);
        tap.offer(at(150, 100), &second, gains, false);
        tap.radio(crate::hardware::record_tap::RadioNote::Counted {
            events: 3,
            from_pair: 160,
            to_pair: 240,
            longest_bytes: 64,
        });
        rec.stop();
        let path = data_path_of(&state);
        rec.finish();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes, [first, second].concat());
        let meta = meta_of(&path);
        assert_eq!(meta["global"]["sdrtop:state"], "finished");
        assert_eq!(meta["global"]["sdrtop:stopped"], "stopped by the user");
        assert_eq!(meta["global"]["sdrtop:lost_queue"], 50);
        assert_eq!(meta["captures"][1]["core:sample_start"], 100);
        assert_eq!(meta["captures"][1]["core:global_index"], 150);
        assert_eq!(meta["annotations"][0]["core:label"], "lost 50");
        assert_eq!(meta["global"]["sdrtop:radio_drops"], 3);
        let radio = meta["annotations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["core:label"] == "radio dropped 3")
            .unwrap()
            .clone();
        assert_eq!(
            (
                radio["core:sample_start"].clone(),
                radio["core:sample_count"].clone()
            ),
            (110.into(), 80.into())
        );
        let m = state.lock().unwrap();
        let done = m.record.current.as_ref().unwrap();
        assert_eq!(
            (done.pairs, done.ended.as_deref()),
            (200, Some("stopped by the user"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The limit ends it to the pair, lets the tap go, and says which limit.
    #[test]
    fn the_limit_ends_it_and_lets_the_tap_go() {
        let dir = scratch("limit");
        let state = streaming();
        let tap = Arc::new(RecordTap::default());
        let limits = Limits {
            max_secs: 0.00015,
            max_bytes: 4_000_000_000,
        };
        let rec = Recorder::start(&state, &tap, &dir, limits, NOW).unwrap();
        tap.offer(at(0, 100), &[1; 200], vec![], false);
        tap.offer(at(100, 100), &[2; 200], vec![], false);
        let path = data_path_of(&state);
        rec.finish();
        assert!(!tap.armed(), "a finished recording must not keep the feed");
        assert_eq!(std::fs::read(&path).unwrap().len(), 300);
        let meta = meta_of(&path);
        assert_eq!(meta["global"]["sdrtop:pairs_written"], 150);
        assert!(meta["global"]["sdrtop:stopped"]
            .as_str()
            .unwrap()
            .contains("length limit"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **What recording costs, measured.** Not a check: run it by hand, in
    /// release, on the machine and the disk the question is about
    /// (`cargo test --release what_recording_costs -- --ignored --nocapture`).
    /// It offers 20 Msps of 8-bit blocks at the pace a HackRF delivers them,
    /// for five seconds (or `SDRTOP_RECORD_SECONDS`), into `target/`, and prints how much the queue refused
    /// and how long the offers took the callback.
    #[test]
    #[ignore]
    fn what_recording_costs() {
        const RATE: f64 = 20e6;
        const BLOCK_PAIRS: u64 = 131_072;
        // `SDRTOP_RECORD_SECONDS=60` runs a whole default limit, past what
        // the page cache can hide.
        let seconds: f64 = std::env::var("SDRTOP_RECORD_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5.0);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/iq-measure");
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = SdrMetrics::fixture().streaming();
        m.radio.config_sample_rate = RATE;
        let state = Arc::new(Mutex::new(m));
        let tap = Arc::new(RecordTap::default());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let rec = Recorder::start(&state, &tap, &dir, Limits::DEFAULT, now).unwrap();
        let block = vec![3u8; (BLOCK_PAIRS * 2) as usize];
        let every = Duration::from_secs_f64(BLOCK_PAIRS as f64 / RATE);
        let blocks = (seconds * RATE / BLOCK_PAIRS as f64) as u64;
        let start = Instant::now();
        let mut offering = Duration::ZERO;
        for k in 0..blocks {
            let due = start + every * k as u32;
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            let t = Instant::now();
            tap.offer(at(k * BLOCK_PAIRS, BLOCK_PAIRS), &block, vec![], false);
            offering += t.elapsed();
        }
        rec.stop();
        let path = data_path_of(&state);
        rec.finish();
        let meta = meta_of(&path);
        eprintln!(
            "20 Msps for {seconds} s: {} of {} pairs refused by the queue; the callback spent {:.1} us an offer",
            meta["global"]["sdrtop:lost_queue"],
            blocks * BLOCK_PAIRS,
            offering.as_secs_f64() * 1e6 / blocks as f64
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sigmf-meta"));
    }

    /// Each refusal says why, and leaves nothing on disk.
    #[test]
    fn a_recording_that_cannot_start_says_why_and_writes_nothing() {
        let dir = scratch("refuse");
        let tap = Arc::new(RecordTap::default());

        let idle = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let err = Recorder::start(&idle, &tap, &dir, Limits::DEFAULT, NOW)
            .err()
            .unwrap();
        assert!(err.contains("not streaming"), "{err}");

        let state = streaming();
        let huge = Limits {
            max_secs: 1e12,
            max_bytes: u64::MAX / 4,
        };
        let err = Recorder::start(&state, &tap, &dir, huge, NOW)
            .err()
            .unwrap();
        assert!(err.contains("GB free"), "{err}");

        let rec = Recorder::start(&state, &tap, &dir, Limits::DEFAULT, NOW).unwrap();
        let err = Recorder::start(&state, &tap, &dir, Limits::DEFAULT, NOW + 5.0)
            .err()
            .unwrap();
        assert!(err.contains("running already"), "{err}");
        rec.stop();
        rec.finish();

        // One pair per second is the naming, and a name is never reused.
        let err = Recorder::start(&state, &tap, &dir, Limits::DEFAULT, NOW)
            .err()
            .unwrap();
        assert!(err.contains("refusing to overwrite"), "{err}");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            2,
            "one pair, no more"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
