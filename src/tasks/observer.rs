// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::fmt_duration;
use super::system::cpu_percent;
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
        // The holder's share is on the same scale as our own, every core
        // together, so the two CPU figures on one screen can be compared.
        let ncpus = std::thread::available_parallelism()
            .map(|n| n.get() as f64)
            .unwrap_or(1.0);
        let mut last_owner_cpu: Option<(u32, f64, Instant)> = None;

        loop {
            // A scan that finds nothing, or that the system refused, leaves
            // the last account as it was, as an unplugged radio always has.
            if let Ok(Some(info)) = (profile.scan)() {
                let owner = hardware::usb::find_owner(&info);
                let usb = hardware::usb::usb_line(&info);
                // A share needs two readings of the same process; the first
                // one, or one after the holder changed, has none yet.
                let now = Instant::now();
                let cpu_pct = owner.as_ref().and_then(|o| {
                    let (pid, used, at) = last_owner_cpu?;
                    let cpu_s = o.cpu_s?;
                    (pid == o.pid)
                        .then(|| cpu_percent(cpu_s - used, (now - at).as_secs_f64(), ncpus))
                });
                last_owner_cpu = owner.as_ref().and_then(|o| Some((o.pid, o.cpu_s?, now)));

                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.observer.device = Some(format!("{} · {}", info.product, info.manufacturer));
                m.observer.serial = info.serial;
                m.observer.usb = Some(usb).filter(|l| !l.is_empty());
                m.observer.connected = info.connected_secs.map(fmt_duration);

                if let Some(o) = owner {
                    m.observer.owner = Some(format!("{} (PID {})", o.name, o.pid));
                    m.observer.cmdline = o.cmdline;
                    m.observer.owner_cpu_pct = cpu_pct;
                    m.observer.owner_ram_mb = o.rss_mb;
                    m.observer.owner_uptime = o.running_secs.map(fmt_duration);
                } else {
                    m.observer.owner = None;
                    m.observer.cmdline = None;
                    m.observer.owner_cpu_pct = None;
                    m.observer.owner_ram_mb = None;
                    m.observer.owner_uptime = None;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}
