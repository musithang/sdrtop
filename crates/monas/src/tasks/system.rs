// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The app's own CPU share and resident memory, once a second.
//!
//! Split as the hardware layer is: what a reading means is plain data and
//! plain arithmetic (`parse_proc` for Linux, [`SelfStats::new`], [`cpu_percent`]),
//! tested on every host; how it is read is a thin call that differs by
//! system. Linux reads `/proc/self`; macOS has no `/proc`, so it takes the
//! CPU time from `getrusage` (seconds, unambiguous) and the resident size
//! from `proc_pidinfo` (bytes; its CPU times on Apple Silicon are in mach
//! ticks, which is why they are not used). A reading that cannot be taken
//! is no reading, never a zero.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::state::SdrMetrics;

/// What the process has used: CPU time across all its threads so far, and
/// the memory resident now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelfStats {
    pub cpu_s: f64,
    pub rss_mb: u64,
}

impl SelfStats {
    /// From CPU seconds and resident bytes, the bytes as whole megabytes.
    pub fn new(cpu_s: f64, rss_bytes: u64) -> Self {
        SelfStats {
            cpu_s,
            rss_mb: rss_bytes / (1024 * 1024),
        }
    }
}

/// The share of every core together, 0 to 100 %, that `used_s` of CPU time
/// over `elapsed_s` of wall time is on `ncpus` cores; 0 when no time passed.
pub fn cpu_percent(used_s: f64, elapsed_s: f64, ncpus: f64) -> f32 {
    if elapsed_s > 0.0 && ncpus > 0.0 {
        (used_s / elapsed_s / ncpus * 100.0).min(100.0) as f32
    } else {
        0.0
    }
}

/// Linux's `/proc/self/stat` and `/proc/self/status`, with the kernel's
/// `ticks_per_sec`: user and system ticks as CPU seconds, `VmRSS` as
/// megabytes. The fields after the process name are counted from its last
/// `)`, so a name with spaces or brackets in it does not move them. Linux's
/// alone, and compiled for the tests on every host.
#[cfg(any(target_os = "linux", test))]
pub fn parse_proc(stat: &str, status: &str, ticks_per_sec: f64) -> Option<SelfStats> {
    let after_comm = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let rss_kb: u64 = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    if ticks_per_sec <= 0.0 {
        return None;
    }
    Some(SelfStats::new(
        (utime + stime) as f64 / ticks_per_sec,
        rss_kb * 1024,
    ))
}

/// The app's own load now, on this host; `None` when it cannot be read.
pub fn read_self_stats() -> Option<SelfStats> {
    host::read()
}

#[cfg(target_os = "linux")]
mod host {
    use super::{parse_proc, SelfStats};

    pub fn read() -> Option<SelfStats> {
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        parse_proc(&stat, &status, ticks)
    }
}

#[cfg(target_os = "macos")]
mod host {
    use super::SelfStats;

    pub fn read() -> Option<SelfStats> {
        // Safety: both calls fill a zeroed struct of the size passed and
        // read nothing else; a failure is reported by the return value.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
            return None;
        }
        let seconds = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
        let cpu_s = seconds(usage.ru_utime) + seconds(usage.ru_stime);
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        let got = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                (&mut info as *mut libc::proc_taskinfo).cast(),
                size,
            )
        };
        if got != size {
            return None;
        }
        Some(SelfStats::new(cpu_s, info.pti_resident_size))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod host {
    pub fn read() -> Option<super::SelfStats> {
        None
    }
}

/// Measures the app's own CPU usage and RAM every 1 s, writes to `state`.
pub fn spawn_sys_resource_task(state: Arc<Mutex<SdrMetrics>>) {
    tokio::spawn(async move {
        // Normalise by core count so the result is a system-wide 0–100% figure.
        // Without this, a multi-threaded process (FFT worker + rx task + UI) can
        // consume >1 core, making the raw per-core figure pin to 100%.
        let ncpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1) as f64;
        let mut last_cpu = read_self_stats().map(|s| s.cpu_s).unwrap_or(0.0);
        let mut last_time = Instant::now();

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Some(now) = read_self_stats() {
                let cpu_pct = cpu_percent(
                    (now.cpu_s - last_cpu).max(0.0),
                    last_time.elapsed().as_secs_f64(),
                    ncpus,
                );
                last_cpu = now.cpu_s;
                last_time = Instant::now();
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.system.process_cpu_pct = cpu_pct;
                m.system.process_rss_mb = now.rss_mb;
                if m.system.cpu_history.len() >= crate::state::THROUGHPUT_HISTORY_LEN {
                    m.system.cpu_history.pop_front();
                }
                m.system.cpu_history.push_back((cpu_pct * 10.0) as u64);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_field_indices() {
        let fake = "1234 (my process) S 1 1 1 0 -1 4194304 0 0 0 0 42 7 0 0 20 0 1 0 0 0 0";
        let after_comm = fake.rsplit_once(')').unwrap().1;
        let fields: Vec<&str> = after_comm.split_whitespace().collect();
        assert_eq!(fields.get(11), Some(&"42"), "utime at index 11");
        assert_eq!(fields.get(12), Some(&"7"), "stime at index 12");
    }

    /// Linux's `/proc/self` text becomes CPU seconds (user and system
    /// ticks over the tick rate) and resident megabytes; a process name
    /// with a `)` and spaces in it does not move the fields.
    #[test]
    fn proc_text_becomes_cpu_seconds_and_megabytes() {
        let stat = "1234 (my (odd) proc) S 1 1 1 0 -1 4194304 0 0 0 0 42 8 0 0 20 0 1 0 0 0 0";
        let status = "Name:\tsdrtop\nVmRSS:\t   5120 kB\nThreads:\t9\n";
        let s = parse_proc(stat, status, 100.0).expect("parsed");
        assert!((s.cpu_s - 0.5).abs() < 1e-9, "{s:?}");
        assert_eq!(s.rss_mb, 5);
        assert!(parse_proc("garbage", status, 100.0).is_none());
        assert!(parse_proc(stat, "Name:\tsdrtop\n", 100.0).is_none());
    }

    /// Resident bytes become whole megabytes, rounded down.
    #[test]
    fn resident_bytes_become_megabytes() {
        assert_eq!(SelfStats::new(1.25, 5 * 1024 * 1024 + 1).rss_mb, 5);
    }

    /// The CPU share is of every core together, 0 to 100 %: half a core's
    /// second over a second on four cores is 12.5 %; no time elapsed is 0.
    #[test]
    fn the_cpu_share_is_of_every_core_together() {
        assert!((cpu_percent(0.5, 1.0, 4.0) - 12.5).abs() < 1e-6);
        assert_eq!(cpu_percent(10.0, 1.0, 2.0), 100.0);
        assert_eq!(cpu_percent(0.5, 0.0, 4.0), 0.0);
    }

    /// **Our own load reads on this host**, Linux here and macOS in CI: a
    /// resident size above a megabyte, and CPU time that grows by about
    /// what a 200 ms spin uses, so a value in the wrong unit is caught.
    #[test]
    fn our_own_load_reads_on_this_host() {
        let before = read_self_stats().expect("our own load must read");
        assert!(before.rss_mb >= 1, "{before:?}");
        let start = Instant::now();
        let mut x = 0u64;
        while start.elapsed() < Duration::from_millis(200) {
            x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
        }
        let after = read_self_stats().unwrap();
        let used = after.cpu_s - before.cpu_s;
        assert!(
            used > 0.02 && used < 2.0,
            "{used} s for a 200 ms spin: {before:?} {after:?}"
        );
    }
}
