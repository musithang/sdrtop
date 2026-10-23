// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The USB scan on Linux: `/sys/bus/usb/devices` for the device, `/proc/*/fd`
//! for who holds it. Both read while another app has the radio open, which
//! is what makes observer mode possible.

use std::fs;
use std::path::Path;

use super::{UsbDeviceInfo, Wanted};

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn hex(path: &Path) -> Option<u16> {
    u16::from_str_radix(&read(path)?, 16).ok()
}

/// The first device under `devices` (a `/sys/bus/usb/devices` tree) that is
/// `wanted`.
pub fn scan(devices: &Path, wanted: &Wanted) -> Option<UsbDeviceInfo> {
    fs::read_dir(devices).ok()?.flatten().find_map(|entry| {
        let base = entry.path();
        // Interface entries (`1-2:1.0`) have no idVendor and are passed over.
        let vid = hex(&base.join("idVendor"))?;
        let pid = hex(&base.join("idProduct"))?;
        if !wanted.is(vid, pid) {
            return None;
        }
        let number = |file: &str| read(&base.join(file)).and_then(|s| s.parse().ok());
        Some(UsbDeviceInfo {
            product: read(&base.join("product")).unwrap_or_else(|| wanted.product.into()),
            manufacturer: read(&base.join("manufacturer"))
                .unwrap_or_else(|| wanted.manufacturer.into()),
            serial: read(&base.join("serial")),
            speed_mbits: read(&base.join("speed")).and_then(|s| s.parse().ok()),
            max_power: read(&base.join("bMaxPower")),
            bus: number("busnum"),
            dev: number("devnum"),
            // In milliseconds (the kernel's sysfs-bus-usb ABI; checked against
            // a plug-in time in the kernel log), and not written by every
            // kernel.
            connected_secs: read(&base.join("power/connected_duration"))
                .and_then(|s| s.parse::<u64>().ok())
                .map(|ms| ms / 1_000),
            // Linux names no holder; `find_owner` asks by device node.
            holder: None,
        })
    })
}

/// The radio as this system's `/sys` describes it.
#[cfg(target_os = "linux")]
pub fn find(wanted: &Wanted) -> Result<Option<UsbDeviceInfo>, String> {
    let devices = Path::new("/sys/bus/usb/devices");
    if !devices.exists() {
        return Ok(None);
    }
    Ok(scan(devices, wanted))
}

/// Scans /proc/*/fd/ to find which process has /dev/bus/usb/BUS/DEV open.
/// Only works for processes owned by the same user (or root). Skips others silently.
#[cfg(target_os = "linux")]
pub fn find_owner(bus: u32, dev: u32) -> Option<super::OwnerInfo> {
    let device_node = format!("/dev/bus/usb/{:03}/{:03}", bus, dev);
    fs::read_dir("/proc").ok()?.flatten().find_map(|entry| {
        let pid = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
        // read_dir on fd/ fails for another user's process: passed over.
        let owns_device = fs::read_dir(entry.path().join("fd"))
            .ok()?
            .flatten()
            .any(|fd| {
                fs::read_link(fd.path())
                    .map(|t| t.to_string_lossy() == device_node)
                    .unwrap_or(false)
            });
        if owns_device {
            process(pid)
        } else {
            None
        }
    })
}

/// What `/proc` says about process `pid`, each reading only when it could
/// be read. A process with no name has gone, and is `None`.
#[cfg(target_os = "linux")]
fn process(pid: u32) -> Option<super::OwnerInfo> {
    let base = Path::new("/proc").join(pid.to_string());
    let name = read(&base.join("comm"))?;
    let cmdline = fs::read(base.join("cmdline")).ok().and_then(|bytes| {
        let line = String::from_utf8_lossy(&bytes).replace('\0', " ");
        let line = line.trim();
        (!line.is_empty()).then(|| line.to_string())
    });

    // SAFETY: sysconf reads a constant and has no preconditions.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let ticks = (ticks > 0).then_some(ticks as f64);
    // After the last ')': state(0) ppid(1) ... utime(11) stime(12) ...
    // starttime(19), the times in clock ticks.
    let stat = read(&base.join("stat")).and_then(|s| {
        let fields: Vec<u64> = s
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .map(|f| f.parse().unwrap_or(0))
            .collect();
        Some((*fields.get(11)? + *fields.get(12)?, *fields.get(19)?))
    });
    let uptime = read(Path::new("/proc/uptime"))
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok());
    let cpu_s = stat.zip(ticks).map(|((cpu, _), t)| cpu as f64 / t);
    let running_secs = stat
        .zip(ticks)
        .zip(uptime)
        .map(|(((_, start), t), up)| (up - start as f64 / t).max(0.0) as u64);

    let rss_mb = read(&base.join("status")).and_then(|s| {
        let kb = s
            .lines()
            .find(|l| l.starts_with("VmRSS:"))?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()?;
        Some(kb / 1024)
    });

    Some(super::OwnerInfo {
        pid,
        name,
        cmdline,
        cpu_s,
        rss_mb,
        running_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{HACKRF, RTLSDR};
    use super::*;
    use std::path::PathBuf;

    /// A `/sys/bus/usb/devices` tree of its own, removed when dropped.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Tree {
            let root =
                std::env::temp_dir().join(format!("sdrtop-usb-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Tree(root)
        }

        fn device(&self, name: &str, files: &[(&str, &str)]) {
            let dir = self.0.join(name);
            fs::create_dir_all(dir.join("power")).unwrap();
            for (file, value) in files {
                fs::write(dir.join(file), format!("{value}\n")).unwrap();
            }
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A HackRF reads as the kernel writes it: its own strings, the speed,
    /// the power, the bus and device numbers and how long it has been in.
    /// An interface entry and another vendor's device are passed over.
    #[test]
    fn a_hackrf_reads_as_the_kernel_writes_it() {
        let tree = Tree::new("hackrf");
        tree.device("1-2:1.0", &[("bInterfaceClass", "ff")]);
        tree.device("1-1", &[("idVendor", "046d"), ("idProduct", "c52b")]);
        tree.device(
            "1-2",
            &[
                ("idVendor", "1d50"),
                ("idProduct", "6089"),
                ("product", "HackRF One"),
                ("manufacturer", "Great Scott Gadgets"),
                ("serial", "0000000000000000a06063c8234e925f"),
                ("speed", "480"),
                ("bMaxPower", "500mA"),
                ("busnum", "1"),
                ("devnum", "7"),
                ("power/connected_duration", "93000"),
            ],
        );
        let info = scan(&tree.0, &HACKRF).expect("the HackRF is found");
        assert_eq!(
            info,
            UsbDeviceInfo {
                product: "HackRF One".into(),
                manufacturer: "Great Scott Gadgets".into(),
                serial: Some("0000000000000000a06063c8234e925f".into()),
                speed_mbits: Some(480.0),
                max_power: Some("500mA".into()),
                bus: Some(1),
                dev: Some(7),
                connected_secs: Some(93),
                holder: None,
            }
        );
        assert_eq!(scan(&tree.0, &RTLSDR), None);
    }

    /// **What the kernel did not write is missing**, never a plausible
    /// value: no "480", no "500mA", no bus 0. A dongle with no strings of
    /// its own is named for its ids, which is what they stand for. A
    /// low-speed device's 1.5 is read as 1.5.
    #[test]
    fn what_the_kernel_did_not_write_is_missing() {
        let tree = Tree::new("bare");
        tree.device(
            "3-1",
            &[
                ("idVendor", "0bda"),
                ("idProduct", "2838"),
                ("speed", "1.5"),
            ],
        );
        let info = scan(&tree.0, &RTLSDR).expect("the dongle is found");
        assert_eq!(
            info,
            UsbDeviceInfo {
                product: "RTL-SDR".into(),
                manufacturer: "Realtek".into(),
                serial: None,
                speed_mbits: Some(1.5),
                max_power: None,
                bus: None,
                dev: None,
                connected_secs: None,
                holder: None,
            }
        );
    }

    /// This test's own process, read the way a holder is: its name, its
    /// command line, and readings that are there and plausible.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_reads_as_proc_describes_it() {
        let me = process(std::process::id()).expect("our own process reads");
        assert_eq!(me.pid, std::process::id());
        assert!(!me.name.is_empty());
        assert!(me.cmdline.as_deref().is_some_and(|c| c.contains("sdrtop")));
        assert!(me.cpu_s.is_some_and(|s| s >= 0.0));
        assert!(me.rss_mb.is_some());
        assert!(me.running_secs.is_some_and(|s| s < 3600));
        assert_eq!(process(u32::MAX), None, "no such process");
    }

    /// A tree that is not there finds nothing rather than failing.
    #[test]
    fn a_missing_tree_finds_nothing() {
        let gone = std::env::temp_dir().join("sdrtop-usb-tree-that-does-not-exist");
        assert_eq!(scan(&gone, &HACKRF), None);
    }
}
