// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::fmt_duration;
use crate::hardware::{self, discovery::ObserverProfile};
use crate::state::SdrMetrics;

/// Polls the USB scan every 1 s to track the SDR device and which process
/// owns it (observer mode only). Writes device identity and owner info to
/// `state`.
///
/// `profile` carries the scan, so this task never asks which backend it is
/// watching. A backend with no profile cannot get here: observer mode is
/// refused for it before the task is spawned.
pub fn spawn_observer_task(state: Arc<Mutex<SdrMetrics>>, profile: ObserverProfile) {
    tokio::spawn(async move {
        let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        let mut last_owner_cpu: Option<(u64, Instant)> = None;

        loop {
            // A scan that finds nothing, or that the system refused, leaves
            // the last account as it was, as an unplugged radio always has.
            if let Ok(Some(info)) = (profile.scan)() {
                let owner = hardware::usb::find_owner(&info);
                let usb = hardware::usb::usb_line(&info);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());

                m.observer.device = Some(format!("{} · {}", info.product, info.manufacturer));
                m.observer.serial = info.serial;
                m.observer.usb = Some(usb).filter(|l| !l.is_empty());
                m.observer.connected = info.connected_secs.map(fmt_duration);

                if let Some(o) = owner {
                    let cpu_pct = if let Some((last_ticks, last_time)) = last_owner_cpu {
                        let elapsed = last_time.elapsed().as_secs_f64();
                        let delta = o.cpu_ticks.saturating_sub(last_ticks) as f64;
                        if elapsed > 0.0 && ticks_per_sec > 0.0 {
                            (delta / ticks_per_sec / elapsed * 100.0).min(100.0) as f32
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };
                    last_owner_cpu = Some((o.cpu_ticks, Instant::now()));

                    m.observer.owner = Some(format!("{} (PID {})", o.name, o.pid));
                    m.observer.cmdline = Some(o.cmdline);
                    m.observer.owner_cpu_pct = cpu_pct;
                    m.observer.owner_ram_mb = o.rss_mb;
                    m.observer.owner_uptime = Some(fmt_duration(o.running_secs));
                } else {
                    last_owner_cpu = None;
                    m.observer.owner = None;
                    m.observer.cmdline = None;
                    m.observer.owner_cpu_pct = 0.0;
                    m.observer.owner_ram_mb = 0;
                    m.observer.owner_uptime = None;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}
