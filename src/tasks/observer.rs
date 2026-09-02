// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::fmt_duration;
use crate::hardware::{self, DeviceKind};
use crate::state::SdrMetrics;

/// Polls sysfs/proc every 1 s to track which process owns the SDR device
/// (observer mode only).  Writes device identity and owner info to `state`.
pub fn spawn_observer_task(state: Arc<Mutex<SdrMetrics>>, bus: u32, dev: u32, kind: DeviceKind) {
    tokio::spawn(async move {
        let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        let mut last_owner_cpu: Option<(u64, Instant)> = None;

        loop {
            let found = match kind {
                DeviceKind::HackRf => hardware::sysfs::find_hackrf(),
                DeviceKind::RtlSdr => hardware::sysfs::find_rtlsdr(),
                // Never spawned for a Soapy device: `App::new` refuses observer
                // mode there because there is no sysfs profile to read. None
                // keeps the loop honest rather than reporting a HackRF's USB
                // statistics for something else entirely.
                DeviceKind::Soapy => None,
            };
            if let Some(info) = found {
                let owner = hardware::sysfs::find_owner(info.bus, info.dev);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());

                m.observer.device = Some(format!("{} · {}", info.product, info.manufacturer));
                m.observer.serial = Some(info.serial);
                m.observer.usb = Some(format!(
                    "High Speed ({} Mbit/s) · {} · Bus {}, Dev {}",
                    info.speed_mbits, info.max_power, info.bus, info.dev
                ));
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
            // bus/dev retained to avoid unused-variable warnings; may be used for
            // direct sysfs node lookup in a future improvement.
            let _ = (bus, dev);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}
