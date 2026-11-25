// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The 2.4 GHz band, logged unattended: `[monasd] job = "occupancy"`.
//!
//! One JSON line per megahertz per period into
//! `occupancy-YYYY-MM-DD.jsonl` in the data folder, a new file each UTC day.
//! Three kinds of line:
//!
//! - `{"provenance": {...}}`: the program and the export header's fields,
//!   first in every file and again whenever monasd starts.
//! - `{"cell": {...}}`: one megahertz over one period, the band CSV's
//!   columns as fields. A cell nobody looked at in the period is `null` in
//!   every figure, never zero: zero is a quiet megahertz.
//! - `{"gap": {...}}`: a stretch when the log was blind or partly blind,
//!   from, to, and why: the stream losing samples, the NET feed refusing
//!   blocks, the radio stopping or gone, the survey not running, dwells
//!   measured against a noise floor that failed. A week of this file is
//!   only worth reading if a blind hour never reads as a quiet one.
//!
//! The period's figures are every dwell inside it
//! ([`crate::state::Tally`]), not the band as it stood when the period
//! ended. A duty cycle the panel would dash is `null` here too, its spread
//! kept, by the same question asked of the same uncertainty.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use monas::signal::net::occupancy;
use serde_json::{json, Value};

use crate::engine::events::{Event, Waited};
use crate::export::provenance;
use crate::state::{SdrMetrics, Tally};

/// One line per megahertz per this long.
///
/// A minute: 83 lines, about seventeen kilobytes (an hour on the desk wrote
/// 1.05 MB), so a day is about twenty-five megabytes and a week fits
/// anywhere. Fine enough to see a microwave oven
/// come and go, coarse enough that each cell has thousands of windows behind
/// it even while the survey is elsewhere most of the time.
pub const PERIOD: Duration = Duration::from_secs(60);

/// Where the lines go: a file per name, opened to append.
pub trait Sink {
    fn open(&mut self, name: &str) -> std::io::Result<Box<dyn Write + Send>>;
}

/// The data folder, the files appended to and created when missing.
pub struct Folder(pub std::path::PathBuf);

impl Sink for Folder {
    fn open(&mut self, name: &str) -> std::io::Result<Box<dyn Write + Send>> {
        std::fs::create_dir_all(&self.0)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.0.join(name))?;
        Ok(Box::new(file))
    }
}

/// The day's file name for a moment, by its UTC date.
pub fn file_name(unix_ms: u64) -> String {
    let day = provenance::iso8601((unix_ms / 1000) as i64);
    format!("occupancy-{}.jsonl", &day[..10])
}

/// The log: the open day's file, and what it writes.
pub struct Log<S: Sink> {
    sink: S,
    writer: String,
    provenance: Vec<(&'static str, String)>,
    open: Option<(String, Box<dyn Write + Send>)>,
    stopped: bool,
}

impl<S: Sink> Log<S> {
    /// A log that writes through `sink`, naming `writer` (the program and
    /// its version) as the one that wrote it.
    pub fn new(sink: S, writer: &str) -> Self {
        Log {
            sink,
            writer: writer.to_string(),
            provenance: Vec::new(),
            open: None,
            stopped: false,
        }
    }

    /// The export header's fields, for the next file opened.
    pub fn set_provenance(&mut self, fields: Vec<(&'static str, String)>) {
        self.provenance = fields;
    }

    /// Whether a write failed and the log gave up.
    #[cfg(test)]
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// One period, `from` to `to`: a line per cell, and a gap line when
    /// dwells were left out for a failed floor.
    pub fn period(&mut self, from: u64, to: u64, tally: &Tally) -> Result<(), String> {
        let seconds = to.saturating_sub(from) as f64 / 1000.0;
        let mut lines: Vec<Value> = (0..occupancy::CELLS)
            .map(|i| {
                let c = tally.cells.get(i).copied().unwrap_or_default();
                cell_line(i, from, to, &c, seconds)
            })
            .collect();
        if tally.untrusted_dwells > 0 {
            lines.push(gap_line(
                from,
                to,
                &format!(
                    "{} dwells were measured against a noise floor that failed its \
                     preconditions, and are left out",
                    tally.untrusted_dwells
                ),
            ));
        }
        self.write(from, &lines)
    }

    /// A gap, `from` to `to`, and why.
    pub fn gap(&mut self, from: u64, to: u64, why: &str) -> Result<(), String> {
        self.write(from, &[gap_line(from, to, why)])
    }

    /// The lines into the file of `at`'s day, opening it (provenance first)
    /// when the day is new. The first failure stops the log and is the one
    /// sentence said about it.
    fn write(&mut self, at: u64, lines: &[Value]) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        let name = file_name(at);
        let result = self.write_into(&name, lines);
        result.map_err(|e| {
            self.stopped = true;
            self.open = None;
            format!("monasd: the occupancy log stopped, could not write {name}: {e}")
        })
    }

    fn write_into(&mut self, name: &str, lines: &[Value]) -> std::io::Result<()> {
        if self.open.as_ref().is_none_or(|(n, _)| n != name) {
            let file = self.sink.open(name)?;
            self.open = Some((name.to_string(), file));
            let mut p = serde_json::Map::new();
            p.insert("writer".into(), Value::from(self.writer.clone()));
            for (k, v) in &self.provenance {
                p.insert((*k).into(), Value::from(v.clone()));
            }
            let first = json!({ "provenance": p });
            self.put(&first)?;
        }
        for line in lines {
            self.put(line)?;
        }
        if let Some((_, f)) = self.open.as_mut() {
            f.flush()?;
        }
        Ok(())
    }

    fn put(&mut self, line: &Value) -> std::io::Result<()> {
        let (_, f) = self.open.as_mut().expect("opened above");
        let mut text = line.to_string();
        text.push('\n');
        f.write_all(text.as_bytes())
    }
}

/// A figure to the places the band CSV writes it to.
fn places(v: f64, n: i32) -> f64 {
    let k = 10f64.powi(n);
    (v * k).round() / k
}

fn cell_line(i: usize, from: u64, to: u64, c: &crate::state::CellTally, seconds: f64) -> Value {
    let mhz = occupancy::cell_centre_hz(i) as f64 / 1e6;
    let (duty, sigma) = match c.duty() {
        Some(d) => {
            let u = occupancy::duty_uncertain(d, c.windows).scale(100.0);
            (
                u.is_resolved(occupancy::DUTY_RESOLUTION * 100.0)
                    .then(|| places(u.value(), 2)),
                u.sigma().is_finite().then(|| places(u.sigma(), 2)),
            )
        }
        None => (None, None),
    };
    let observed = c.windows > 0;
    json!({ "cell": {
        "from_unix_ms": from,
        "to_unix_ms": to,
        "mhz": mhz,
        "observed": observed,
        "duty_pct": duty,
        "duty_sigma_pct": sigma,
        "coverage_pct": (observed && seconds > 0.0)
            .then(|| places((c.observed_s / seconds).min(1.0) * 100.0, 1)),
        "mean_dbfs": c.mean_dbfs().map(|v| places(v, 1)),
        "peak_dbfs": if observed { c.peak_dbfs.map(|v| places(v, 1)) } else { None },
        "windows": c.windows,
    }})
}

fn gap_line(from: u64, to: u64, why: &str) -> Value {
    json!({ "gap": { "from_unix_ms": from, "to_unix_ms": to, "why": why } })
}

/// The gaps the engine tells of, made into lines: from its events, and from
/// the NET feed's own account of its losses.
#[derive(Default)]
pub struct Gaps {
    stopped_at: Option<u64>,
    feed_seen: Option<Instant>,
    feed_checked: Option<u64>,
}

impl Gaps {
    /// The gap an event makes, if it makes one: `(from, to, why)`.
    pub fn event(&mut self, event: &Event, now_ms: u64) -> Option<(u64, u64, String)> {
        match event {
            Event::Gap {
                at_unix_ms,
                samples,
                driver,
                monas,
            } => Some((
                *at_unix_ms,
                *at_unix_ms,
                format!("the stream lost {samples} samples (driver {driver}, monas {monas})"),
            )),
            Event::Stale { stale: true } => {
                self.stopped_at.get_or_insert(now_ms);
                None
            }
            Event::Stale { stale: false } => self
                .stopped_at
                .take()
                .map(|from| (from, now_ms, "the radio stopped delivering".to_string())),
            Event::Outage {
                from_unix_ms,
                to_unix_ms,
                reason,
            } => Some((
                *from_unix_ms,
                *to_unix_ms,
                format!("the radio was gone: {reason}"),
            )),
            _ => None,
        }
    }

    /// A gap when the NET feed's last loss is a new one since the last
    /// look, from that look to now: when inside it is all the feed says.
    pub fn feed(&mut self, last_loss: Option<Instant>, now_ms: u64) -> Option<(u64, u64, String)> {
        let Some(since) = self.feed_checked.replace(now_ms) else {
            // The first look: where the feed's account stands. A loss
            // already there was before the log began.
            self.feed_seen = last_loss;
            return None;
        };
        let loss = last_loss?;
        if self.feed_seen == Some(loss) {
            return None;
        }
        self.feed_seen = Some(loss);
        Some((
            since,
            now_ms,
            "the NET feed lost blocks; the band's dwells here are short of them".to_string(),
        ))
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The next period boundary after `ms`, periods counted from the epoch so
/// a minute's lines start on the minute.
fn next_boundary(ms: u64, period_ms: u64) -> u64 {
    (ms / period_ms + 1) * period_ms
}

/// What a look at the clock decides: the period to write, a gap if the
/// clock jumped, and where the next period starts and ends.
#[derive(Debug, PartialEq)]
pub struct Step {
    pub period: (u64, u64),
    pub gap: Option<(u64, u64, String)>,
    pub from: u64,
    pub boundary: u64,
}

/// The period running `from` to `boundary`, looked at `now`: `None` while
/// it runs. A clock more than a period past the boundary, or behind the
/// start, jumped: the period closes once, at now (never before its start),
/// a gap says so, and the next one starts at now. Never a period per
/// minute the clock skipped.
pub fn step(from: u64, boundary: u64, now: u64, period_ms: u64, stopping: bool) -> Option<Step> {
    let jumped = now < from || now >= boundary + period_ms;
    if jumped {
        let direction = if now < from { "back" } else { "forward" };
        let moved_s = now.abs_diff(boundary.min(from + period_ms)) / 1000;
        return Some(Step {
            period: (from, now.max(from)),
            gap: Some((
                from.min(now),
                now.max(from),
                format!(
                    "the wall clock jumped {direction} by about {moved_s} s; this period \
                     ends where the jump was noticed"
                ),
            )),
            from: now,
            boundary: next_boundary(now, period_ms),
        });
    }
    if stopping {
        return Some(Step {
            period: (from, now),
            gap: None,
            from: now,
            boundary: next_boundary(now, period_ms),
        });
    }
    (now >= boundary).then(|| Step {
        period: (from, boundary),
        gap: None,
        from: boundary,
        boundary: next_boundary(boundary, period_ms),
    })
}

/// Run the log until `stop`: a period's lines at every boundary, a gap line
/// as each gap is told of, and the part-period left when stopped. A write
/// that fails stops the log with one error in the engine's log; the engine
/// carries on measuring either way.
pub async fn run<S: Sink>(
    state: Arc<Mutex<SdrMetrics>>,
    sink: S,
    writer: String,
    period: Duration,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let period_ms = period.as_millis().max(1) as u64;
    let tick = (period / 4).min(Duration::from_secs(1));
    let mut log = Log::new(sink, &writer);
    let mut gaps = Gaps::default();
    let events = {
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        // Dwells from before the log began are no part of its first period.
        m.net.band.take_tally();
        m.events.subscribe(1024)
    };
    let mut from = unix_ms();
    let mut boundary = next_boundary(from, period_ms);
    loop {
        let stopping = tokio::select! {
            _ = tokio::time::sleep(tick) => false,
            _ = stop.changed() => true,
        };
        let now = unix_ms();
        let mut told = Vec::new();
        while let Waited::Event(d) = events.wait(Duration::ZERO) {
            if d.lost_before > 0 {
                told.push((
                    now,
                    now,
                    format!(
                        "the log missed {} of the engine's events, and a gap among them \
                         may be missing here",
                        d.lost_before
                    ),
                ));
            }
            told.extend(gaps.event(&d.event, now));
        }
        let (last_loss, period_end) = {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let end = step(from, boundary, now, period_ms, stopping).map(|step| {
                (
                    step,
                    m.net.band.take_tally(),
                    m.mode.net.survey,
                    m.mode.view.clone(),
                    provenance::fields(&m, (now / 1000) as i64),
                )
            });
            (m.net.health.last_loss, end)
        };
        told.extend(gaps.feed(last_loss, now));
        let mut result = Ok(());
        for (a, b, why) in told {
            result = result.and_then(|()| log.gap(a, b, &why));
        }
        if let Some((step, tally, surveying, view, fields)) = period_end {
            let (from_ms, to) = step.period;
            log.set_provenance(fields);
            if let Some((a, b, why)) = &step.gap {
                result = result.and_then(|()| log.gap(*a, *b, why));
            }
            if !surveying {
                result = result.and_then(|()| {
                    log.gap(
                        from_ms,
                        to,
                        &format!("the band was not being surveyed: the mode was {view}"),
                    )
                });
            }
            result = result.and_then(|()| log.period(from_ms, to, &tally));
            from = step.from;
            boundary = step.boundary;
        }
        if let Err(why) = result {
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_log(why);
            return;
        }
        if stopping {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::events::Event;
    use crate::state::{CellTally, Tally};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    /// Files in memory, by name, each written as a disk would write it.
    #[derive(Clone, Default)]
    struct Mem(Arc<Mutex<BTreeMap<String, Vec<u8>>>>);

    struct MemFile(Arc<Mutex<BTreeMap<String, Vec<u8>>>>, String);

    impl std::io::Write for MemFile {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut files = self.0.lock().unwrap();
            files.get_mut(&self.1).unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Sink for Mem {
        fn open(&mut self, name: &str) -> std::io::Result<Box<dyn std::io::Write + Send>> {
            self.0.lock().unwrap().entry(name.to_string()).or_default();
            Ok(Box::new(MemFile(Arc::clone(&self.0), name.to_string())))
        }
    }

    impl Mem {
        fn lines(&self, name: &str) -> Vec<serde_json::Value> {
            let files = self.0.lock().unwrap();
            let text = String::from_utf8(files.get(name).cloned().unwrap_or_default()).unwrap();
            text.lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
        fn names(&self) -> Vec<String> {
            self.0.lock().unwrap().keys().cloned().collect()
        }
    }

    /// 2026-11-25 12:00:00 UTC.
    const NOON: u64 = 1_795_608_000_000;
    const MINUTE: u64 = 60_000;

    fn log(sink: &Mem) -> Log<Mem> {
        let mut log = Log::new(sink.clone(), "monasd test");
        log.set_provenance(vec![("exported", "now".to_string())]);
        log
    }

    fn tally(cells: &[(usize, CellTally)]) -> Tally {
        let mut t = Tally {
            cells: vec![CellTally::default(); 83],
            untrusted_dwells: 0,
        };
        for &(i, c) in cells {
            t.cells[i] = c;
        }
        t
    }

    /// Ten seconds of a cell, a quarter of it busy, at -60 dBFS.
    fn seen(windows: u64, duty: f64) -> CellTally {
        CellTally {
            windows,
            busy: duty * windows as f64,
            power: 1e-6 * windows as f64,
            peak_dbfs: Some(-30.0),
            observed_s: 10.0,
        }
    }

    #[test]
    fn the_file_is_named_by_its_utc_day() {
        assert_eq!(file_name(NOON), "occupancy-2026-11-25.jsonl");
        assert_eq!(
            file_name(NOON + 12 * 60 * MINUTE),
            "occupancy-2026-11-26.jsonl"
        );
    }

    /// **The first line says where everything under it came from**: the
    /// program, then the export header's own fields.
    #[test]
    fn the_first_line_is_provenance() {
        let sink = Mem::default();
        let mut log = log(&sink);
        log.period(NOON, NOON + MINUTE, &tally(&[(3, seen(1_000_000, 0.25))]))
            .unwrap();
        let lines = sink.lines("occupancy-2026-11-25.jsonl");
        let p = &lines[0]["provenance"];
        assert_eq!(p["writer"], "monasd test");
        assert_eq!(p["exported"], "now");
        assert_eq!(lines.len(), 1 + 83, "provenance, then a line per cell");
        let c = &lines[4]["cell"];
        assert_eq!(c["from_unix_ms"], NOON);
        assert_eq!(c["to_unix_ms"], NOON + MINUTE);
        assert_eq!(c["mhz"], 2403.5);
        assert_eq!(c["observed"], true);
        assert_eq!(c["duty_pct"], 25.0);
        assert_eq!(c["mean_dbfs"], -60.0);
        assert_eq!(c["peak_dbfs"], -30.0);
        assert_eq!(c["windows"], 1_000_000);
        // Ten seconds watched in a minute.
        assert_eq!(c["coverage_pct"], 16.7);
    }

    /// **A cell nobody looked at in the period is null, never zero**: zero
    /// is a quiet megahertz, null is nobody looking.
    #[test]
    fn an_unobserved_cell_is_null_never_zero() {
        let sink = Mem::default();
        let mut log = log(&sink);
        log.period(NOON, NOON + MINUTE, &tally(&[(3, seen(1_000_000, 0.0))]))
            .unwrap();
        let lines = sink.lines("occupancy-2026-11-25.jsonl");
        let unseen = &lines[1]["cell"];
        assert_eq!(unseen["observed"], false);
        assert_eq!(unseen["windows"], 0);
        for field in [
            "duty_pct",
            "duty_sigma_pct",
            "coverage_pct",
            "mean_dbfs",
            "peak_dbfs",
        ] {
            assert!(unseen[field].is_null(), "{field}: {}", unseen[field]);
        }
        // And a quiet cell that was watched reads zero, so the two differ.
        assert_eq!(lines[4]["cell"]["duty_pct"], 0.0);
    }

    /// A duty cycle its own spread cannot support is null, the spread kept:
    /// the panel's rule, asked of the same uncertainty.
    #[test]
    fn a_duty_cycle_the_panel_would_dash_is_null_and_keeps_its_spread() {
        let sink = Mem::default();
        let mut log = log(&sink);
        log.period(NOON, NOON + MINUTE, &tally(&[(10, seen(40, 0.5))]))
            .unwrap();
        let c = &sink.lines("occupancy-2026-11-25.jsonl")[11]["cell"];
        assert_eq!(c["observed"], true);
        assert!(c["duty_pct"].is_null());
        assert!(c["duty_sigma_pct"].as_f64().unwrap() > 5.0);
    }

    /// Dwells left out for a failed floor leave the period with a gap line
    /// saying how many, so a thin period is never read as a quiet one.
    #[test]
    fn dwells_on_a_failed_floor_are_a_gap_line() {
        let sink = Mem::default();
        let mut log = log(&sink);
        let mut t = tally(&[]);
        t.untrusted_dwells = 7;
        log.period(NOON, NOON + MINUTE, &t).unwrap();
        let lines = sink.lines("occupancy-2026-11-25.jsonl");
        let gap = lines.iter().find_map(|l| l.get("gap")).expect("a gap line");
        assert_eq!(gap["from_unix_ms"], NOON);
        assert_eq!(gap["to_unix_ms"], NOON + MINUTE);
        let why = gap["why"].as_str().unwrap();
        assert!(
            why.contains("7 dwells") && why.contains("noise floor"),
            "{why}"
        );
    }

    /// **The stream losing samples is a gap line**, with whose loss it was.
    #[test]
    fn a_feed_loss_inside_a_dwell_is_a_gap_line() {
        let mut gaps = Gaps::default();
        let (from, to, why) = gaps
            .event(
                &Event::Gap {
                    at_unix_ms: NOON + 5,
                    samples: 4096,
                    driver: 4096,
                    monas: 0,
                },
                NOON + 10,
            )
            .unwrap();
        assert_eq!((from, to), (NOON + 5, NOON + 5));
        assert!(why.contains("4096") && why.contains("driver"), "{why}");

        let sink = Mem::default();
        let mut log = log(&sink);
        log.gap(from, to, &why).unwrap();
        let lines = sink.lines("occupancy-2026-11-25.jsonl");
        assert!(lines[0].get("provenance").is_some());
        assert_eq!(lines[1]["gap"]["why"], why);
    }

    /// The NET feed refusing blocks is a loss too, said once per loss.
    #[test]
    fn the_net_feed_losing_blocks_is_a_gap_line() {
        let mut gaps = Gaps::default();
        let t0 = std::time::Instant::now();
        assert!(gaps.feed(None, NOON).is_none());
        let (from, to, why) = gaps.feed(Some(t0), NOON + 1_000).unwrap();
        assert_eq!((from, to), (NOON, NOON + 1_000));
        assert!(why.contains("NET feed"), "{why}");
        assert!(gaps.feed(Some(t0), NOON + 2_000).is_none(), "the same loss");
        let later = t0 + std::time::Duration::from_millis(1);
        assert!(gaps.feed(Some(later), NOON + 3_000).is_some(), "a new one");
    }

    /// A loss from before the log began is no gap of the log's: the first
    /// look only learns where the feed's account stands.
    #[test]
    fn a_feed_loss_from_before_the_log_is_not_its_gap() {
        let mut gaps = Gaps::default();
        let t0 = std::time::Instant::now();
        assert!(gaps.feed(Some(t0), NOON).is_none());
        assert!(gaps.feed(Some(t0), NOON + 1_000).is_none());
    }

    /// **A period ends on its boundary, and a clock that jumps does not
    /// make a decade of them.** A box with no clock battery boots in 1970
    /// and is set right by NTP a minute later: the period closes when that
    /// is noticed, a gap line says the clock moved, and the next period
    /// starts on the next boundary after now. Back in time likewise.
    #[test]
    fn a_clock_that_jumps_closes_the_period_once_and_says_so() {
        let p = MINUTE;
        // On time: the boundary, and the next one.
        let s = step(NOON, NOON + p, NOON + p + 300, p, false).unwrap();
        assert_eq!(s.period, (NOON, NOON + p));
        assert!(s.gap.is_none());
        assert_eq!((s.from, s.boundary), (NOON + p, NOON + 2 * p));
        // Not yet.
        assert!(step(NOON, NOON + p, NOON + 10, p, false).is_none());
        // Stopping: what there is, up to now.
        let s = step(NOON, NOON + p, NOON + 10, p, true).unwrap();
        assert_eq!(s.period, (NOON, NOON + 10));

        // Forward by a day: one period, closed at now, and a gap.
        let booted = 60_000;
        let s = step(booted, booted + p, NOON, p, false).unwrap();
        assert_eq!(s.period, (booted, NOON));
        let (_, _, why) = s.gap.unwrap();
        assert!(why.contains("clock"), "{why}");
        assert_eq!((s.from, s.boundary), (NOON, NOON + p));

        // Backward: nothing written for a stretch that ends before it began.
        let s = step(NOON, NOON + p, NOON - 3_600_000, p, false).unwrap();
        assert_eq!(s.period.0, NOON);
        assert!(s.period.1 >= s.period.0);
        assert!(s.gap.unwrap().2.contains("clock"));
        assert_eq!(s.from, NOON - 3_600_000);
        assert_eq!(s.boundary, NOON - 3_600_000 + p - (NOON - 3_600_000) % p);
    }

    /// The radio stopping and starting again is one gap, its two ends; an
    /// outage is one already.
    #[test]
    fn the_radio_stopping_is_one_gap_from_stop_to_start() {
        let mut gaps = Gaps::default();
        assert!(gaps.event(&Event::Stale { stale: true }, NOON).is_none());
        let (from, to, why) = gaps
            .event(&Event::Stale { stale: false }, NOON + 9_000)
            .unwrap();
        assert_eq!((from, to), (NOON, NOON + 9_000));
        assert!(why.contains("stopped delivering"), "{why}");

        let (from, to, why) = gaps
            .event(
                &Event::Outage {
                    from_unix_ms: NOON,
                    to_unix_ms: NOON + MINUTE,
                    reason: "unplugged".into(),
                },
                NOON + MINUTE,
            )
            .unwrap();
        assert_eq!((from, to), (NOON, NOON + MINUTE));
        assert!(why.contains("unplugged"), "{why}");
    }

    /// **Midnight starts a new file, and the new file starts with where it
    /// came from**: a day's file read alone still has its provenance.
    #[test]
    fn midnight_starts_a_new_file_with_provenance() {
        let sink = Mem::default();
        let mut log = log(&sink);
        let before = NOON + 12 * 60 * MINUTE - MINUTE;
        log.period(before, before + MINUTE, &tally(&[])).unwrap();
        log.period(before + MINUTE, before + 2 * MINUTE, &tally(&[]))
            .unwrap();
        assert_eq!(
            sink.names(),
            ["occupancy-2026-11-25.jsonl", "occupancy-2026-11-26.jsonl"]
        );
        let first = sink.lines("occupancy-2026-11-25.jsonl");
        let second = sink.lines("occupancy-2026-11-26.jsonl");
        assert_eq!(first.len(), 1 + 83);
        assert_eq!(second.len(), 1 + 83);
        assert!(second[0].get("provenance").is_some());
    }

    /// **The job, run against a state as monasd runs it**: the survey's
    /// tally becomes a period's lines, the engine's gap event a gap line,
    /// and the part-period left when it is stopped is written too, not
    /// lost.
    #[tokio::test]
    async fn the_job_writes_periods_gaps_and_the_last_part_on_stop() {
        let dir = std::env::temp_dir().join(format!("monasd-occupancy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = crate::state::SdrMetrics::fixture().streaming();
        m.show_preset(monas::signal::net::lock::SURVEY_VIEW);
        // From before the log began: none of its first period.
        m.net.band.tally = tally(&[(7, seen(1_000_000, 0.5))]);
        let state = Arc::new(Mutex::new(m));
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let job = tokio::spawn(run(
            Arc::clone(&state),
            Folder(dir.clone()),
            "monasd test".to_string(),
            Duration::from_millis(200),
            stopped,
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        {
            // Added to whatever the tally holds, as a dwell is.
            let mut m = state.lock().unwrap();
            let cells = &mut m.net.band.tally.cells;
            cells.resize(83, CellTally::default());
            cells[5] = seen(1_000_000, 0.5);
        }
        state.lock().unwrap().events.publish(Event::Gap {
            at_unix_ms: 1,
            samples: 4096,
            driver: 0,
            monas: 4096,
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        stop.send(true).unwrap();
        job.await.unwrap();

        let mut text = String::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            text.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap());
        }
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(lines[0].get("provenance").is_some(), "{text}");
        let cells: Vec<&serde_json::Value> = lines.iter().filter_map(|l| l.get("cell")).collect();
        assert!(cells.len() >= 2 * 83, "at least a period and the last part");
        assert_eq!(cells.len() % 83, 0);
        // The tally, taken once, in whichever period it landed; the one from
        // before the log began in none.
        let counted: Vec<_> = cells.iter().filter(|c| c["windows"] != 0).collect();
        assert_eq!(counted.len(), 1, "{counted:?}");
        assert_eq!(counted[0]["windows"], 1_000_000);
        assert_eq!(counted[0]["mhz"], 2405.5);
        assert!(
            lines
                .iter()
                .filter_map(|l| l.get("gap"))
                .any(|g| g["why"].as_str().unwrap().contains("4096")),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A disk that fills: the writing fails, once.
    struct Full;

    impl std::io::Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(28))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct FullDisk;

    impl Sink for FullDisk {
        fn open(&mut self, _: &str) -> std::io::Result<Box<dyn std::io::Write + Send>> {
            Ok(Box::new(Full))
        }
    }

    /// **A full disk stops the log and says so, once, as an error**; nothing
    /// panics, and what comes after is not tried again.
    #[test]
    fn disk_full_stops_the_log_and_says_so() {
        let mut log = Log::new(FullDisk, "monasd test");
        let why = log.period(NOON, NOON + MINUTE, &tally(&[])).unwrap_err();
        assert_eq!(
            crate::state::LogLevel::infer(&why),
            crate::state::LogLevel::Error,
            "{why}"
        );
        assert!(why.contains("occupancy-2026-11-25.jsonl"), "{why}");
        assert!(log.stopped());
        assert!(log
            .period(NOON + MINUTE, NOON + 2 * MINUTE, &tally(&[]))
            .is_ok());
        assert!(log.gap(NOON, NOON, "x").is_ok());
    }
}
