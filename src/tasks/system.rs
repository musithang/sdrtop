use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::state::SdrMetrics;

/// Measures the app's own CPU usage and RAM every 1 s, writes to `state`.
pub fn spawn_sys_resource_task(state: Arc<Mutex<SdrMetrics>>) {
    tokio::spawn(async move {
        let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        // Normalise by core count so the result is a system-wide 0–100% figure.
        // Without this, a multi-threaded process (FFT worker + rx task + UI) can
        // consume >1 core, making the raw per-core figure pin to 100%.
        let ncpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1) as f64;
        let mut last_ticks = read_self_stats().map(|(t, _)| t).unwrap_or(0);
        let mut last_time = Instant::now();

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Some((total_ticks, rss_mb)) = read_self_stats() {
                let elapsed = last_time.elapsed().as_secs_f64();
                let tick_delta = total_ticks.saturating_sub(last_ticks) as f64;
                let cpu_pct = if elapsed > 0.0 && ticks_per_sec > 0.0 {
                    (tick_delta / ticks_per_sec / elapsed / ncpus * 100.0).min(100.0) as f32
                } else {
                    0.0
                };
                last_ticks = total_ticks;
                last_time = Instant::now();
                if let Ok(mut m) = state.lock() {
                    m.system.process_cpu_pct = cpu_pct;
                    m.system.process_rss_mb = rss_mb;
                    if m.system.cpu_history.len() >= crate::state::THROUGHPUT_HISTORY_LEN {
                        m.system.cpu_history.pop_front();
                    }
                    m.system.cpu_history.push_back((cpu_pct * 10.0) as u64);
                }
            }
        }
    });
}

/// Reads CPU ticks (utime + stime) and RSS in MB from `/proc/self`.
/// Returns `None` if the files are unreadable or unparseable.
pub fn read_self_stats() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let rss_kb: u64 = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some((utime + stime, rss_kb / 1024))
}

#[cfg(test)]
mod tests {
    #[test]
    fn proc_stat_field_indices() {
        let fake = "1234 (my process) S 1 1 1 0 -1 4194304 0 0 0 0 42 7 0 0 20 0 1 0 0 0 0";
        let after_comm = fake.rsplit_once(')').unwrap().1;
        let fields: Vec<&str> = after_comm.split_whitespace().collect();
        assert_eq!(fields.get(11), Some(&"42"), "utime at index 11");
        assert_eq!(fields.get(12), Some(&"7"), "stime at index 12");
    }
}
