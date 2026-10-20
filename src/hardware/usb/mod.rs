// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The USB radios sdrtop knows by vendor and product id, found even while
//! another app holds them: what observer mode describes, and who holds them.
//!
//! Split by system, as the rest of the hardware layer is split by what it
//! can test: what a device is and how it reads on screen is plain code here,
//! tested on every host; how it is found differs. Linux reads `/sys`
//! (`linux`), macOS asks IOKit (`macos`). A value the system does not give
//! is missing, never a plausible default (POLICY rule 2).

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

/// A USB device sdrtop recognised, as the system describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct UsbDeviceInfo {
    /// The product and manufacturer strings, or the names the vendor and
    /// product id stand for when the device does not give its own.
    pub product: String,
    pub manufacturer: String,
    pub serial: Option<String>,
    /// The link speed in Mbit/s (1.5 for a low-speed device).
    pub speed_mbits: Option<f64>,
    /// The power it asked the bus for, as the system writes it (`500mA`).
    pub max_power: Option<String>,
    pub bus: Option<u32>,
    pub dev: Option<u32>,
    pub connected_secs: Option<u64>,
}

/// The process holding a device, as observer mode describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnerInfo {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
    pub cpu_ticks: u64,
    pub rss_mb: u64,
    pub running_secs: u64,
}

/// A radio looked for: its vendor id, the product ids it ships under, and the
/// names they stand for when the device gives none of its own.
#[derive(Clone, Copy, Debug)]
pub struct Wanted {
    pub vid: u16,
    pub pids: &'static [u16],
    pub product: &'static str,
    pub manufacturer: &'static str,
}

impl Wanted {
    /// Whether `vid`:`pid` is this radio.
    pub fn is(&self, vid: u16, pid: u16) -> bool {
        vid == self.vid && self.pids.contains(&pid)
    }
}

/// A HackRF One (Great Scott Gadgets, 1d50:6089).
pub const HACKRF: Wanted = Wanted {
    vid: 0x1d50,
    pids: &[0x6089],
    product: "HackRF One",
    manufacturer: "Great Scott Gadgets",
};

/// An RTL-SDR dongle (Realtek, 0bda:2832 or 0bda:2838).
pub const RTLSDR: Wanted = Wanted {
    vid: 0x0bda,
    pids: &[0x2832, 0x2838],
    product: "RTL-SDR",
    manufacturer: "Realtek",
};

/// Looks for `wanted` on this system. `Ok(None)` is the radio not being
/// there; an error is the system refusing to say, and says why.
pub fn find(wanted: &Wanted) -> Result<Option<UsbDeviceInfo>, String> {
    #[cfg(target_os = "linux")]
    return linux::find(wanted);
    #[cfg(target_os = "macos")]
    return macos::find(wanted);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = wanted;
        Err("sdrtop cannot list USB devices on this system".into())
    }
}

/// A HackRF One, held by another app or not.
pub fn find_hackrf() -> Result<Option<UsbDeviceInfo>, String> {
    find(&HACKRF)
}

/// An RTL-SDR dongle, held by another app or not.
pub fn find_rtlsdr() -> Result<Option<UsbDeviceInfo>, String> {
    find(&RTLSDR)
}

/// The process holding `device`, when this system can tell. Linux finds it
/// by the device node it has open, which needs the bus and device number;
/// macOS does not say yet.
pub fn find_owner(device: &UsbDeviceInfo) -> Option<OwnerInfo> {
    #[cfg(target_os = "linux")]
    return linux::find_owner(device.bus?, device.dev?);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = device;
        None
    }
}

/// A USB speed's name, from its rate in Mbit/s.
pub fn speed_name(mbits: f64) -> &'static str {
    match mbits {
        m if m < 12.0 => "Low Speed",
        m if m < 480.0 => "Full Speed",
        m if m < 5000.0 => "High Speed",
        m if m < 10000.0 => "SuperSpeed",
        _ => "SuperSpeed+",
    }
}

/// The observer's USB line: the speed by name with its rate, the power, the
/// bus and the device number, each only when the system gave it.
pub fn usb_line(info: &UsbDeviceInfo) -> String {
    let speed = info
        .speed_mbits
        .map(|m| format!("{} ({m} Mbit/s)", speed_name(m)));
    let place = [
        info.bus.map(|b| format!("Bus {b}")),
        info.dev.map(|d| format!("Dev {d}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let place = (!place.is_empty()).then(|| place.join(", "));
    [speed, info.max_power.clone(), place]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// What IOKit's registry says about one USB device, each property only when
/// the device has it.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, Default)]
pub struct IoUsbProps {
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    /// `USB Product Name`, `USB Vendor Name`, `USB Serial Number`.
    pub product: Option<String>,
    pub vendor: Option<String>,
    pub serial: Option<String>,
    /// `locationID`: the controller in its top byte, the ports below it.
    pub location_id: Option<u32>,
    /// `USB Address`, the device number on its bus.
    pub address: Option<u32>,
    /// `Device Speed`, IOKit's code for the link speed.
    pub speed: Option<u32>,
}

/// The device as IOKit describes it, if it is `wanted`.
///
/// The bus is the top byte of `locationID`, as libusb numbers it on macOS.
/// IOKit gives no requested power and no connect time for a device, so those
/// stay missing. A speed code this code does not know is missing too, not
/// rounded to the nearest one it does.
#[cfg(any(target_os = "macos", test))]
pub fn from_iokit(props: &IoUsbProps, wanted: &Wanted) -> Option<UsbDeviceInfo> {
    if !wanted.is(props.vid?, props.pid?) {
        return None;
    }
    // IOKit's USB speed codes, kUSBDeviceSpeedLow (0) through
    // kUSBDeviceSpeedSuperPlusBy2 (5), as libusb reads them on macOS too.
    let speed_mbits = props.speed.and_then(|code| match code {
        0 => Some(1.5),
        1 => Some(12.0),
        2 => Some(480.0),
        3 => Some(5000.0),
        4 => Some(10000.0),
        5 => Some(20000.0),
        _ => None,
    });
    Some(UsbDeviceInfo {
        product: props
            .product
            .clone()
            .unwrap_or_else(|| wanted.product.into()),
        manufacturer: props
            .vendor
            .clone()
            .unwrap_or_else(|| wanted.manufacturer.into()),
        serial: props.serial.clone(),
        speed_mbits,
        max_power: None,
        bus: props.location_id.map(|l| l >> 24),
        dev: props.address,
        connected_secs: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> UsbDeviceInfo {
        UsbDeviceInfo {
            product: "HackRF One".into(),
            manufacturer: "Great Scott Gadgets".into(),
            serial: Some("0000000000000000".into()),
            speed_mbits: Some(480.0),
            max_power: Some("500mA".into()),
            bus: Some(1),
            dev: Some(5),
            connected_secs: None,
        }
    }

    /// A radio is its vendor id with one of its product ids.
    #[test]
    fn a_radio_is_its_vendor_and_one_of_its_products() {
        assert!(HACKRF.is(0x1d50, 0x6089));
        assert!(RTLSDR.is(0x0bda, 0x2838));
        assert!(RTLSDR.is(0x0bda, 0x2832));
        assert!(!RTLSDR.is(0x0bda, 0x0129), "a card reader, not a dongle");
        assert!(!HACKRF.is(0x1d51, 0x6089));
    }

    /// Each speed by its name, a low-speed device's 1.5 included.
    #[test]
    fn a_speed_is_named_from_its_rate() {
        assert_eq!(speed_name(1.5), "Low Speed");
        assert_eq!(speed_name(12.0), "Full Speed");
        assert_eq!(speed_name(480.0), "High Speed");
        assert_eq!(speed_name(5000.0), "SuperSpeed");
        assert_eq!(speed_name(10000.0), "SuperSpeed+");
    }

    /// **The line says what the system gave, and only that**: the speed
    /// named from its own rate (a full-speed device is not called "High
    /// Speed"), and a power or a bus the system did not give left out
    /// rather than filled in.
    #[test]
    fn the_usb_line_says_what_the_system_gave() {
        assert_eq!(
            usb_line(&info()),
            "High Speed (480 Mbit/s) · 500mA · Bus 1, Dev 5"
        );
        let full = UsbDeviceInfo {
            speed_mbits: Some(12.0),
            ..info()
        };
        assert!(usb_line(&full).starts_with("Full Speed (12 Mbit/s)"));
        let bare = UsbDeviceInfo {
            speed_mbits: None,
            max_power: None,
            bus: None,
            dev: None,
            ..info()
        };
        assert_eq!(usb_line(&bare), "");
        let low = UsbDeviceInfo {
            speed_mbits: Some(1.5),
            max_power: None,
            ..info()
        };
        assert_eq!(usb_line(&low), "Low Speed (1.5 Mbit/s) · Bus 1, Dev 5");
    }

    /// The scan runs on this system and answers, radio or no radio: on the
    /// macOS runner this is IOKit being asked for real.
    #[test]
    fn the_scan_answers_on_this_host() {
        assert!(find(&HACKRF).is_ok(), "{:?}", find(&HACKRF));
        assert!(find(&RTLSDR).is_ok(), "{:?}", find(&RTLSDR));
    }

    fn hackrf_props() -> IoUsbProps {
        IoUsbProps {
            vid: Some(0x1d50),
            pid: Some(0x6089),
            product: Some("HackRF One".into()),
            vendor: Some("Great Scott Gadgets".into()),
            serial: Some("0000000000000000a06063c8234e925f".into()),
            location_id: Some(0x0110_0000),
            address: Some(3),
            speed: Some(2),
        }
    }

    /// A HackRF as IOKit lists it: the bus from the top byte of
    /// `locationID`, the device number from `USB Address`, the speed from
    /// its code, and no power or connect time, which IOKit does not give.
    #[test]
    fn a_hackrf_reads_as_iokit_lists_it() {
        assert_eq!(
            from_iokit(&hackrf_props(), &HACKRF),
            Some(UsbDeviceInfo {
                product: "HackRF One".into(),
                manufacturer: "Great Scott Gadgets".into(),
                serial: Some("0000000000000000a06063c8234e925f".into()),
                speed_mbits: Some(480.0),
                max_power: None,
                bus: Some(1),
                dev: Some(3),
                connected_secs: None,
            })
        );
        assert_eq!(from_iokit(&hackrf_props(), &RTLSDR), None);
    }

    /// Every speed code IOKit defines, and one it does not.
    #[test]
    fn each_iokit_speed_code_is_its_rate() {
        for (code, mbits) in [
            (0, 1.5),
            (1, 12.0),
            (2, 480.0),
            (3, 5000.0),
            (4, 10000.0),
            (5, 20000.0),
        ] {
            let props = IoUsbProps {
                speed: Some(code),
                ..hackrf_props()
            };
            let info = from_iokit(&props, &HACKRF).unwrap();
            assert_eq!(info.speed_mbits, Some(mbits), "code {code}");
        }
        let unknown = IoUsbProps {
            speed: Some(9),
            ..hackrf_props()
        };
        assert_eq!(from_iokit(&unknown, &HACKRF).unwrap().speed_mbits, None);
    }

    /// A device with only its ids is named for them; one without ids is no
    /// radio of ours.
    #[test]
    fn a_device_with_only_its_ids_is_named_for_them() {
        let bare = IoUsbProps {
            vid: Some(0x0bda),
            pid: Some(0x2832),
            ..IoUsbProps::default()
        };
        let info = from_iokit(&bare, &RTLSDR).unwrap();
        assert_eq!(
            (info.product.as_str(), info.manufacturer.as_str()),
            ("RTL-SDR", "Realtek")
        );
        assert_eq!(
            (info.serial, info.speed_mbits, info.bus, info.dev),
            (None, None, None, None)
        );
        assert_eq!(from_iokit(&IoUsbProps::default(), &RTLSDR), None);
    }
}
