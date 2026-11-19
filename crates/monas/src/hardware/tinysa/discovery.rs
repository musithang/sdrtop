// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Finding a tinySA's USB console without being told where it is.
//!
//! What counts as a tinySA is decided once, in [`recognise`], from the USB
//! identity both systems report; how that identity is reached differs.
//! Linux walks up from each `/dev/ttyACM*` through `/sys`; macOS lists its
//! serial ports through the `serialport` crate, which asks IOKit, and keeps
//! the `/dev/cu.*` ("callout") name of each, the one that opens without
//! waiting for a modem's carrier signal.

use std::path::PathBuf;
#[cfg(any(target_os = "linux", test))]
use std::{fs, path::Path};

use crate::hardware::{DeviceKind, DeviceListing};

/// The STM32 virtual COM port id every tinySA enumerates as.
const VID: u16 = 0x0483;
const PID: u16 = 0x5740;

pub(super) fn list() -> Vec<DeviceListing> {
    #[cfg(target_os = "linux")]
    return listings(list_at(Path::new("/sys/class/tty"), Path::new("/dev")));
    #[cfg(target_os = "macos")]
    return listings(from_ports(
        serialport::available_ports().unwrap_or_default(),
    ));
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    Vec::new()
}

/// The product name a USB device goes by, when it is a tinySA: the STM32
/// console id is shared with other boards, so the manufacturer
/// (`tinysa.org`) or the product (`tinySA`, `tinySA4`) must say so too.
fn recognise(
    vid: u16,
    pid: u16,
    manufacturer: Option<&str>,
    product: Option<&str>,
) -> Option<String> {
    if (vid, pid) != (VID, PID) {
        return None;
    }
    let manufacturer = manufacturer.unwrap_or_default();
    let product = product.unwrap_or_default();
    let known_manufacturer = manufacturer.eq_ignore_ascii_case("tinysa.org");
    let known_product =
        product.eq_ignore_ascii_case("tinysa") || product.eq_ignore_ascii_case("tinysa4");
    if !known_manufacturer && !known_product {
        return None;
    }
    Some(if product.is_empty() {
        "tinySA".to_string()
    } else {
        product.to_string()
    })
}

/// Each port found, by path, numbered in path order.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn listings(mut found: Vec<(PathBuf, String)>) -> Vec<DeviceListing> {
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
        .into_iter()
        .enumerate()
        .map(|(index, (path, product))| DeviceListing {
            kind: DeviceKind::TinySa,
            index,
            label: format!("{product} · {}", path.display()),
            serial: None,
            args: None,
            path: Some(path),
            tiny_sa_input: None,
        })
        .collect()
}

/// The tinySAs among macOS's serial ports, by their callout names.
#[cfg(any(target_os = "macos", test))]
fn from_ports(ports: Vec<serialport::SerialPortInfo>) -> Vec<(PathBuf, String)> {
    ports
        .into_iter()
        .filter(|port| port.port_name.starts_with("/dev/cu."))
        .filter_map(|port| match port.port_type {
            serialport::SerialPortType::UsbPort(usb) => {
                let product = recognise(
                    usb.vid,
                    usb.pid,
                    usb.manufacturer.as_deref(),
                    usb.product.as_deref(),
                )?;
                Some((PathBuf::from(port.port_name), product))
            }
            _ => None,
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn list_at(tty_root: &Path, dev_root: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = fs::read_dir(tty_root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("ttyACM") {
            continue;
        }
        let Ok(mut ancestor) = fs::canonicalize(entry.path()) else {
            continue;
        };
        loop {
            if let Some(product) = matching_product(&ancestor) {
                found.push((dev_root.join(name.as_ref()), product));
                break;
            }
            if !ancestor.pop() {
                break;
            }
        }
    }
    found
}

#[cfg(any(target_os = "linux", test))]
fn matching_product(path: &Path) -> Option<String> {
    let id = |file: &str| u16::from_str_radix(&read(path.join(file))?, 16).ok();
    recognise(
        id("idVendor")?,
        id("idProduct")?,
        read(path.join("manufacturer")).as_deref(),
        read(path.join("product")).as_deref(),
    )
}

#[cfg(any(target_os = "linux", test))]
fn read(path: PathBuf) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};

    /// The console id alone is any STM32 board; with the maker or the
    /// product name it is a tinySA, named as it names itself, or `tinySA`
    /// when it gives no product name.
    #[test]
    fn a_tinysa_is_its_console_id_and_its_name() {
        let ultra = recognise(0x0483, 0x5740, Some("tinysa.org"), Some("tinySA4"));
        assert_eq!(ultra.as_deref(), Some("tinySA4"));
        let basic = recognise(0x0483, 0x5740, None, Some("tinySA"));
        assert_eq!(basic.as_deref(), Some("tinySA"));
        let unnamed = recognise(0x0483, 0x5740, Some("tinysa.org"), None);
        assert_eq!(unnamed.as_deref(), Some("tinySA"));
        assert_eq!(
            recognise(
                0x0483,
                0x5740,
                Some("STMicroelectronics"),
                Some("Virtual COM Port")
            ),
            None,
            "another board on the same chip"
        );
        assert_eq!(
            recognise(0x0483, 0x5741, Some("tinysa.org"), Some("tinySA4")),
            None
        );
    }

    fn usb(name: &str, vid: u16, product: Option<&str>) -> SerialPortInfo {
        SerialPortInfo {
            port_name: name.into(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid,
                pid: 0x5740,
                serial_number: Some("400".into()),
                manufacturer: Some("tinysa.org".into()),
                product: product.map(Into::into),
            }),
        }
    }

    /// macOS lists each USB serial device twice, as `/dev/cu.*` and
    /// `/dev/tty.*`; only the callout name is kept, since the other waits
    /// for a carrier the tinySA never raises. Other boards, Bluetooth and
    /// built-in ports are passed over.
    #[test]
    fn on_macos_a_tinysa_is_listed_once_by_its_callout_name() {
        let ports = vec![
            usb("/dev/tty.usbmodem4001", 0x0483, Some("tinySA4")),
            usb("/dev/cu.usbmodem4001", 0x0483, Some("tinySA4")),
            usb("/dev/cu.usbserial-10", 0x0403, Some("FT232R")),
            SerialPortInfo {
                port_name: "/dev/cu.Bluetooth-Incoming-Port".into(),
                port_type: SerialPortType::BluetoothPort,
            },
            SerialPortInfo {
                port_name: "/dev/cu.debug-console".into(),
                port_type: SerialPortType::PciPort,
            },
        ];
        assert_eq!(
            from_ports(ports),
            vec![(PathBuf::from("/dev/cu.usbmodem4001"), "tinySA4".to_string())]
        );
    }

    /// Asking this system for its ports answers, tinySA or none: on the
    /// macOS runner that is IOKit, through the `serialport` crate.
    #[test]
    fn the_port_list_answers_on_this_host() {
        assert!(list().iter().all(|d| d.kind == DeviceKind::TinySa));
    }

    /// Two tinySAs are numbered in path order, whichever way they were
    /// found.
    #[test]
    fn two_tinysas_are_numbered_in_path_order() {
        let found = vec![
            (PathBuf::from("/dev/cu.usbmodem4201"), "tinySA4".to_string()),
            (PathBuf::from("/dev/cu.usbmodem4001"), "tinySA".to_string()),
        ];
        let listed = listings(found);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].index, 0);
        assert_eq!(listed[0].label, "tinySA · /dev/cu.usbmodem4001");
        assert_eq!(listed[1].path, Some(PathBuf::from("/dev/cu.usbmodem4201")));
    }

    /// A Linux tree: `/sys/class/tty/ttyACM0` links into the USB device
    /// that carries the ids and names, two levels up.
    #[test]
    fn on_linux_a_tinysa_is_found_through_its_usb_parent() {
        let root = std::env::temp_dir().join(format!("sdrtop-tinysa-tty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let usb = root.join("devices/usb1/1-1");
        let tty_dir = usb.join("1-1:1.0/tty/ttyACM0");
        fs::create_dir_all(&tty_dir).unwrap();
        for (file, value) in [
            ("idVendor", "0483"),
            ("idProduct", "5740"),
            ("manufacturer", "tinysa.org"),
            ("product", "tinySA4"),
        ] {
            fs::write(usb.join(file), format!("{value}\n")).unwrap();
        }
        let class = root.join("class/tty");
        fs::create_dir_all(&class).unwrap();
        std::os::unix::fs::symlink(&tty_dir, class.join("ttyACM0")).unwrap();
        fs::create_dir_all(class.join("ttyS0")).unwrap();

        let found = list_at(&class, Path::new("/dev"));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            found,
            vec![(PathBuf::from("/dev/ttyACM0"), "tinySA4".to_string())]
        );
    }
}
