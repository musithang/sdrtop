// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! [`SoapyDevice`]: open a device, describe it, and drive its controls.
//!
//! Orchestration only. The unsafe calls are in [`super::api`], the
//! interpretation is in [`super::caps`], and what is left here is the order
//! things happen in.
//!
//! Streaming lives in [`super::stream`], which owns the read thread.
//!
//! **Still not wired into [`crate::hardware::list_all_devices`].** S11 does
//! that, together with the deduplication against the native backends.

use std::sync::Arc;

use super::api::{self, SoapyApi, SoapySDRDevice};
use super::{args, caps};
use crate::hardware::{
    DeviceCapabilities, DeviceInfo, DeviceKind, DeviceListing, RxContext, SdrDevice,
};

pub struct SoapyDevice {
    api: &'static SoapyApi,
    dev: *mut SoapySDRDevice,
    caps: DeviceCapabilities,
    info: DeviceInfo,
    /// The argument string this device was opened with, kept for the log so a
    /// failure names the device it happened on.
    args: String,
    /// The driver's own wire format, kept because `setupStream` wants the name
    /// and `caps` only kept what the name meant.
    native_format: String,
    streaming: super::stream::Streaming,
    /// What `caps` declined to use, in words, for the startup log.
    notes: Vec<String>,
}

// Safety: the same split the two native backends already rely on. The handle is
// touched from the input thread (control) and the read thread (S9), never
// concurrently for the same call, and SoapySDR's own device objects are
// documented as safe to use from multiple threads.
unsafe impl Send for SoapyDevice {}
unsafe impl Sync for SoapyDevice {}

impl SoapyDevice {
    /// Open the device an argument string names, and ask it about itself.
    pub fn open(args: &str) -> anyhow::Result<Self> {
        let Some(api) = api::api() else {
            anyhow::bail!("libSoapySDR is not available");
        };
        // Safety: the handle is stored in `self.dev` and unmade exactly once, in
        // `Drop`.
        let dev = unsafe { api.make(args) }
            .map_err(|e| anyhow::anyhow!("SoapySDR could not open {args}: {e}"))?;

        // Safety: `dev` is live from here until Drop.
        let answers = unsafe { ask(api, dev) };
        let built = match caps::capabilities(&answers) {
            Ok(c) => c,
            Err(why) => {
                unsafe { api.unmake(dev) };
                anyhow::bail!("SoapySDR device {args} cannot be used: {why}");
            }
        };
        let caps = built.caps;
        let info = unsafe { describe(api, dev, args) };

        Ok(Self {
            api,
            dev,
            caps,
            info,
            args: args.to_string(),
            native_format: answers.native_format,
            streaming: super::stream::Streaming::default(),
            // `caps` refuses an element by name rather than silently keeping it.
            // There is no log to say so to yet, so it is carried out to the
            // startup sequence, which has one.
            notes: built.notes,
        })
    }

    /// Both boost keys land here. Which one the user pressed does not matter;
    /// what matters is which mechanism the driver actually has.
    ///
    /// Guarded rather than attempted. Calling `setGainMode` on a driver without
    /// one is an error return in the good case, and `SoapyHackRF` really does
    /// report `Supports AGC: NO`, so this is the common path and not the edge.
    fn set_boost(&self, on: bool) -> anyhow::Result<()> {
        let crate::hardware::GainModel::Soapy { boost, .. } = &self.caps.gain else {
            return Ok(());
        };
        match boost {
            None => Ok(()),
            Some(crate::hardware::SoapyBoost::GainMode) => {
                unsafe { self.api.set_gain_mode(self.dev, on) }
                    .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))
            }
            // A two-position element: driven to one end or the other. Its own
            // reported bounds decide which, rather than 0 and 14 from knowing
            // what a HackRF is.
            Some(crate::hardware::SoapyBoost::Element(s)) => {
                let db = if on { s.max_db } else { s.min_db };
                unsafe { self.api.set_gain_element(self.dev, &s.name, db) }
                    .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))
            }
        }
    }
}

impl SdrDevice for SoapyDevice {
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.caps
    }

    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn start_rx(&self, ctx: Arc<RxContext>) -> anyhow::Result<()> {
        self.streaming
            .start(self.api, self.dev, self.native_format.clone(), ctx)
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        self.streaming.stop();
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        self.streaming.is_active()
    }

    fn open_notes(&self) -> &[String] {
        &self.notes
    }

    fn read_loop_us(&self) -> Option<(u64, u64)> {
        Some(self.streaming.clock().read())
    }

    fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
        unsafe { self.api.set_frequency(self.dev, hz as f64) }
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))
    }

    /// Sets the rate, then matches the baseband filter to it where the device
    /// has one, and returns the bandwidth actually asked for.
    ///
    /// The filter follows the rate rather than being left where it was, because
    /// a filter wider than the sample rate aliases everything outside the window
    /// back into it, and a filter far narrower throws away signal the user can
    /// see on screen.
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<u32> {
        unsafe { self.api.set_sample_rate(self.dev, hz) }
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))?;
        if !self.caps.has_bb_filter {
            return Ok(0);
        }
        match unsafe { self.api.set_bandwidth(self.dev, hz) } {
            Ok(()) => Ok(hz as u32),
            // A device with a bandwidth range that refuses this particular one
            // is still a working receiver. Say so and carry on rather than
            // failing a retune over the filter.
            Err(_) => Ok(0),
        }
    }

    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()> {
        let (clamped, _) = self.caps.gain.clamp_gains(db, 0);
        unsafe { self.api.set_gain(self.dev, clamped as f64) }
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))
    }

    /// There is no second stage. The trait's default would do, but saying it
    /// here keeps the "why nothing happened" answer next to the key.
    fn set_vga_gain(&self, _db: u32) -> anyhow::Result<()> {
        Ok(())
    }

    fn set_amp_enable(&self, on: bool) -> anyhow::Result<()> {
        self.set_boost(on)
    }

    fn set_tuner_agc(&self, on: bool) -> anyhow::Result<()> {
        self.set_boost(on)
    }
}

impl Drop for SoapyDevice {
    fn drop(&mut self) {
        // The read thread borrows `dev`, so it has to be gone before the handle
        // is. `Streaming` also stops itself on drop, but field drop order is not
        // something to leave a device handle's lifetime resting on.
        self.streaming.stop();
        // Safety: `dev` came from `make` and this is the only place it is
        // unmade.
        unsafe { self.api.unmake(self.dev) };
    }
}

/// Everything sdrtop asks a device about itself, in one place.
///
/// # Safety
/// `dev` must be a live handle.
unsafe fn ask(api: &SoapyApi, dev: *mut SoapySDRDevice) -> caps::DriverAnswers {
    let (native_format, native_full_scale) = unsafe { api.native_format(dev) };
    caps::DriverAnswers {
        freq_ranges: unsafe { api.freq_ranges(dev) },
        rate_ranges: unsafe { api.rate_ranges(dev) },
        gain_range: unsafe { api.gain_range(dev) },
        // `listGains` names them; each name is then asked for its own range,
        // which is where the step comes from. Order is preserved exactly: it is
        // the driver's statement about its chain.
        gain_elements: unsafe { api.gain_elements(dev) }
            .into_iter()
            .map(|name| {
                let r = unsafe { api.gain_element_range(dev, &name) }.unwrap_or_default();
                crate::hardware::StageSpec {
                    name,
                    min_db: r.minimum,
                    max_db: r.maximum,
                    step_db: r.step,
                }
            })
            .collect(),
        has_gain_mode: unsafe { api.has_gain_mode(dev) },
        bandwidth_ranges: unsafe { api.bandwidth_ranges(dev) },
        native_format,
        native_full_scale,
    }
}

/// Identity for the header and the RF panels.
///
/// # Safety
/// `dev` must be a live handle.
unsafe fn describe(api: &SoapyApi, dev: *mut SoapySDRDevice, args: &str) -> DeviceInfo {
    let hardware = unsafe { api.hardware_key(dev) };
    let driver = unsafe { api.driver_key(dev) };
    DeviceInfo {
        board_name: if hardware.is_empty() {
            format!("SoapySDR {driver}")
        } else {
            hardware
        },
        // The serial is in the arguments we opened with, since that is what
        // identified this device in the first place.
        serial: serial_from(args).unwrap_or_else(|| driver.clone()),
        fw_version: None,
        board_rev: None,
        usb_api_version: None,
        // Soapy has no notion of a tuner chip, so the driver key is the closest
        // true answer. Better than leaving the field blank and better than
        // inventing a chip name.
        tuner_name: (!driver.is_empty()).then_some(driver.clone()),
        // Whatever firmware is in there is the driver's business, not ours. The
        // header names the path instead: which library, and which driver inside
        // it, because that is what a bug report needs.
        stack: Some(crate::hardware::SoftwareStack {
            label: "soapysdr  ",
            value: std::sync::Arc::from(
                if driver.is_empty() {
                    "unknown".to_string()
                } else {
                    driver.to_ascii_lowercase()
                }
                .as_str(),
            ),
        }),
    }
}

/// Pull the serial back out of an argument string like
/// `driver=hackrf, serial=0000...c3`.
fn serial_from(args: &str) -> Option<String> {
    args::value_of(args, "serial").map(str::to_string)
}

/// Every device SoapySDR can see, as listings.
///
/// Not called from [`crate::hardware::list_all_devices`] yet; S11 wires it in
/// along with the deduplication against the native backends and the audio
/// driver's default exclusion.
pub fn list() -> Vec<DeviceListing> {
    let Some(api) = api::api() else {
        return Vec::new();
    };
    api.enumerate()
        .into_iter()
        .enumerate()
        .map(|(i, kwargs)| DeviceListing {
            kind: DeviceKind::Soapy,
            index: i,
            label: args::label(&kwargs, i),
            serial: args::get(&kwargs, "serial").map(str::to_string),
            args: Some(args::open_markup(&kwargs, i)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::GainModel;

    #[test]
    fn the_serial_comes_back_out_of_the_argument_string() {
        assert_eq!(
            serial_from("driver=hackrf, serial=0000000000000000955c64dc2a3d89c3").as_deref(),
            Some("0000000000000000955c64dc2a3d89c3")
        );
        assert_eq!(serial_from("driver=audio, device_id=0"), None);
        assert_eq!(serial_from(""), None);
        assert_eq!(serial_from("serial="), None, "blank is not a serial");
    }

    /// Enumeration, for real, against whatever this machine has.
    ///
    /// Nothing here can assert that a device is present: CI has none and a
    /// developer machine might have three. What must hold either way is that
    /// every listing this backend produces is one `open_device` could actually
    /// act on. A listing with no arguments is unopenable, and that is exactly
    /// the mistake this backend's index-free addressing exists to avoid.
    #[test]
    fn every_listing_carries_what_it_takes_to_open_it() {
        for l in list() {
            assert_eq!(l.kind, DeviceKind::Soapy);
            assert!(!l.label.is_empty(), "a blank row in the device selector");
            let args = l
                .args
                .expect("a Soapy listing without arguments cannot be opened");
            assert!(args.contains('='), "not argument markup: {args:?}");
        }
    }

    /// The two real gain ranges probed on this machine, plus the shapes a driver
    /// is free to invent. None of them may panic or produce a value outside the
    /// device's own range.
    #[test]
    fn the_gain_lands_inside_whatever_range_the_driver_reported() {
        let soapy = |min_db, max_db| GainModel::Soapy {
            min_db,
            max_db,
            stages: vec![],
            boost: None,
        };
        // SoapyHackRF: 0 to 116 dB.
        assert_eq!(soapy(0, 116).clamp_gains(40, 0).0, 40);
        assert_eq!(soapy(0, 116).clamp_gains(200, 0).0, 116);
        // The sound card: no gain control at all.
        assert_eq!(soapy(0, 0).clamp_gains(30, 0).0, 0);
        // A driver reporting its range backwards must not produce a clamp that
        // panics on an inverted interval.
        assert_eq!(soapy(50, 10).clamp_gains(30, 0).0, 50);
    }
}
