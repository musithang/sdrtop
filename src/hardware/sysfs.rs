// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::fs;
use std::path::Path;

const HACKRF_VID: &str = "1d50";
const HACKRF_PID: &str = "6089";

const RTLSDR_VID: &str = "0bda";
const RTLSDR_PIDS: &[&str] = &["2832", "2838"];

pub struct HackRfSysInfo {
    pub product: String,
    pub manufacturer: String,
    pub serial: String,
    pub speed_mbits: u32,
    pub max_power: String,
    pub bus: u32,
    pub dev: u32,
    pub connected_secs: Option<u64>,
}

pub struct OwnerInfo {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
    pub cpu_ticks: u64,
    pub rss_mb: u64,
    pub running_secs: u64,
}

fn read_sysfs(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Scans /sys/bus/usb/devices/ for a USB device matching `vid` and one of
/// `pids`. Works even when another app holds exclusive access to the device,
/// which is what makes observer mode possible.
fn find_usb(
    vid: &str,
    pids: &[&str],
    default_product: &str,
    default_mfr: &str,
) -> Option<HackRfSysInfo> {
    for entry in fs::read_dir("/sys/bus/usb/devices").ok()?.flatten() {
        let base = entry.path();
        let dev_vid = match read_sysfs(&base.join("idVendor")) {
            Some(v) => v,
            None => continue, // interface entries (X:Y.Z) don't have idVendor
        };
        if dev_vid != vid {
            continue;
        }
        let dev_pid = match read_sysfs(&base.join("idProduct")) {
            Some(p) => p,
            None => continue,
        };
        if !pids.contains(&dev_pid.as_str()) {
            continue;
        }

        let product = read_sysfs(&base.join("product")).unwrap_or_else(|| default_product.into());
        let manufacturer =
            read_sysfs(&base.join("manufacturer")).unwrap_or_else(|| default_mfr.into());
        let serial = read_sysfs(&base.join("serial")).unwrap_or_else(|| "—".into());
        let speed_mbits = read_sysfs(&base.join("speed"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(480);
        let max_power = read_sysfs(&base.join("bMaxPower")).unwrap_or_else(|| "500mA".into());
        let bus = read_sysfs(&base.join("busnum"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let dev = read_sysfs(&base.join("devnum"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // connected_duration is in microseconds; may not exist on all kernels
        let connected_secs = read_sysfs(&base.join("power/connected_duration"))
            .and_then(|s| s.parse::<u64>().ok())
            .map(|us| us / 1_000_000);

        return Some(HackRfSysInfo {
            product,
            manufacturer,
            serial,
            speed_mbits,
            max_power,
            bus,
            dev,
            connected_secs,
        });
    }
    None
}

/// Locate a HackRF One (VID=1d50, PID=6089) via sysfs, even when busy.
pub fn find_hackrf() -> Option<HackRfSysInfo> {
    find_usb(
        HACKRF_VID,
        &[HACKRF_PID],
        "HackRF One",
        "Great Scott Gadgets",
    )
}

/// Locate an RTL-SDR dongle (VID=0bda, PID=2832/2838) via sysfs, even when busy.
pub fn find_rtlsdr() -> Option<HackRfSysInfo> {
    find_usb(RTLSDR_VID, RTLSDR_PIDS, "RTL-SDR", "Realtek")
}

/// Scans /proc/*/fd/ to find which process has /dev/bus/usb/BUS/DEV open.
/// Only works for processes owned by the same user (or root). Skips others silently.
pub fn find_owner(bus: u32, dev: u32) -> Option<OwnerInfo> {
    let device_node = format!("/dev/bus/usb/{:03}/{:03}", bus, dev);

    let uptime_secs = read_sysfs(Path::new("/proc/uptime"))
        .and_then(|s| {
            s.split_whitespace()
                .next()
                .and_then(|v| v.parse::<f64>().ok())
        })
        .unwrap_or(0.0) as u64;

    let ticks_raw = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let ticks_per_sec: u64 = if ticks_raw > 0 { ticks_raw as u64 } else { 100 };

    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let pid_str = entry.file_name();
        let Ok(pid) = pid_str.to_string_lossy().parse::<u32>() else {
            continue;
        };

        let proc_base = entry.path();

        // Check if this process has the HackRF device node open.
        // read_dir on fd/ will fail if we don't own the process - skip silently.
        let owns_device = fs::read_dir(proc_base.join("fd"))
            .map(|entries| {
                entries.flatten().any(|fd| {
                    fs::read_link(fd.path())
                        .map(|t| t.to_string_lossy() == device_node)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);

        if !owns_device {
            continue;
        }

        let name = read_sysfs(&proc_base.join("comm")).unwrap_or_else(|| "unknown".into());

        let cmdline = fs::read(proc_base.join("cmdline"))
            .ok()
            .map(|bytes| {
                bytes
                    .iter()
                    .map(|&b| if b == 0 { ' ' } else { b as char })
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();

        // Parse /proc/PID/stat: after last ')' → state ppid ... utime(11) stime(12) ... starttime(19)
        let (cpu_ticks, starttime_ticks) = read_sysfs(&proc_base.join("stat"))
            .and_then(|s| {
                let after = s.rsplit_once(')')?.1;
                let f: Vec<&str> = after.split_whitespace().collect();
                let utime: u64 = f.get(11)?.parse().ok()?;
                let stime: u64 = f.get(12)?.parse().ok()?;
                let starttime: u64 = f.get(19)?.parse().ok()?;
                Some((utime + stime, starttime))
            })
            .unwrap_or((0, 0));

        let running_secs = uptime_secs.saturating_sub(starttime_ticks / ticks_per_sec.max(1));

        let rss_mb = read_sysfs(&proc_base.join("status"))
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmRSS:"))?
                    .split_whitespace()
                    .nth(1)?
                    .parse::<u64>()
                    .ok()
            })
            .map(|kb| kb / 1024)
            .unwrap_or(0);

        return Some(OwnerInfo {
            pid,
            name,
            cmdline,
            cpu_ticks,
            rss_mb,
            running_secs,
        });
    }
    None
}
