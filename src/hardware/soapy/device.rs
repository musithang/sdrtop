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
//! [`list`] hands its results to [`crate::hardware::discovery`], which decides
//! which of them to offer once the native backends have had their say. This
//! module does not know that decision exists.

use std::sync::Arc;

use super::api::{self, SoapyApi, SoapySDRDevice};
use super::{args, caps};
// `RateSet` is not re-exported from `hardware`: what a backend answers a rate
// change with is between the backend and the trait.
use crate::hardware::traits::RateSet;
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
    /// Whether the driver supplied a usable named gain element. If it did not,
    /// the synthetic `RF` stage is backed by the whole-chain API instead.
    named_gain_elements: bool,
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
        let named_gain_elements = answers
            .gain_elements
            .iter()
            .any(|element| element.is_usable());
        let caps = built.caps;
        let info = unsafe { describe(api, dev, args) };

        Ok(Self {
            api,
            dev,
            caps,
            info,
            args: args.to_string(),
            native_format: answers.native_format,
            named_gain_elements,
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
        match self.caps.gain.boost() {
            None => Ok(()),
            Some(crate::hardware::Boost::GainMode) => {
                unsafe { self.api.set_gain_mode(self.dev, on) }
                    .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))
            }
            // A two-position element: driven to one end or the other. Its own
            // reported bounds decide which, rather than 0 and 14 from knowing
            // what a HackRF is.
            Some(crate::hardware::Boost::Element(s)) => {
                let db = if on { s.max_db } else { s.min_db };
                unsafe { self.api.set_gain_element(self.dev, &s.name, db) }
                    .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))
            }
        }
    }

    /// Set stage `index` by the name the driver gave it, or do nothing when the
    /// device has no such stage.
    ///
    /// The shared body of the trait's `set_lna_gain` and `set_vga_gain`, which
    /// are the two named setters the default `set_stage_gain` maps onto. A
    /// device with fewer stages than the index asks for is left alone rather
    /// than sent a name it never reported.
    ///
    /// **Not `setGain`.** The whole-chain call lets the driver distribute the
    /// figure itself, and `SoapyHackRF` does: it splits the value across LNA,
    /// VGA and AMP and switches the AMP on its own threshold. That is exactly
    /// the behaviour `hardware::gain::distribute` exists to replace, and it
    /// makes the AMP - a two-position switch with a key of its own - move while
    /// the user is turning the LNA. Addressing the element by name keeps the
    /// knob deterministic and the AMP a switch.
    fn set_named_stage(&self, index: usize, db: f64) -> anyhow::Result<()> {
        match self.caps.gain.stages().get(index) {
            Some(spec) => unsafe { self.api.set_gain_element(self.dev, &spec.name, db) }
                .map_err(|e| anyhow::anyhow!("{}: {e}", self.args)),
            None => Ok(()),
        }
    }

    /// Set the synthetic fallback stage through SoapySDR's whole-chain API.
    /// Drivers that expose named elements use the exact element path instead;
    /// drivers that expose none have no valid name to pass to setGainElement.
    fn set_whole_gain(&self, db: f64) -> anyhow::Result<()> {
        unsafe { self.api.set_gain(self.dev, db) }
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))
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

    /// The block the running stream settled on, or the one this backend intends
    /// to read when none is running. See [`super::stream::Streaming::block_pairs`].
    fn samples_per_transfer(&self) -> u64 {
        self.streaming
            .block_pairs()
            .unwrap_or(self.caps.samples_per_transfer)
    }

    fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
        unsafe { self.api.set_frequency(self.dev, hz as f64) }
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))
    }

    /// Sets the rate, asks what it became, then matches the baseband filter to
    /// the answer where the device has one.
    ///
    /// **Asked back rather than assumed.** A SoapySDR driver takes any rate and
    /// quietly gives the nearest one it can produce; some offer a fixed list and
    /// nothing between. `getSampleRate` is the only place that lands, and the
    /// filter below has to follow the rate the device settled on rather than the
    /// one that was asked for, or the window and its filter disagree.
    ///
    /// The filter follows the rate rather than being left where it was, because
    /// a filter wider than the sample rate aliases everything outside the window
    /// back into it, and a filter far narrower throws away signal the user can
    /// see on screen.
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        unsafe { self.api.set_sample_rate(self.dev, hz) }
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))?;
        let mut settled = RateSet::new(hz, Some(unsafe { self.api.get_sample_rate(self.dev) }), 0);
        if !self.caps.has_bb_filter {
            return Ok(settled);
        }
        settled.bb_filter_hz = match unsafe { self.api.set_bandwidth(self.dev, settled.rate_hz) } {
            Ok(()) => settled.rate_hz as u32,
            // A device with a bandwidth range that refuses this particular one
            // is still a working receiver. Say so and carry on rather than
            // failing a retune over the filter.
            Err(_) => 0,
        };
        Ok(settled)
    }

    /// The front stage, by the name the driver gave it.
    ///
    /// **Not `setGain`.** The whole-chain call lets the driver distribute the
    /// figure itself, and `SoapyHackRF` does: it splits the value across LNA,
    /// VGA and AMP and switches the AMP on its own threshold. That is exactly
    /// the behaviour `hardware::gain::distribute` exists to replace, and it
    /// makes the AMP - a two-position switch with a key of its own - move while
    /// the user is turning the LNA. Addressing the element by name keeps the
    /// knob deterministic and the AMP a switch.
    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()> {
        let (clamped, _) = self.caps.gain.clamp_gains(db, 0);
        if self.named_gain_elements {
            self.set_named_stage(0, clamped as f64)
        } else {
            self.set_whole_gain(clamped as f64)
        }
    }

    /// The second stage, by the name the driver gave it.
    ///
    /// A Soapy device that reports a second stage has one to set: the old
    /// `Ok(())` here was written when every Soapy device was assumed to be a
    /// single knob, and it made `[` / `]` report success while moving nothing.
    fn set_vga_gain(&self, db: u32) -> anyhow::Result<()> {
        if self.named_gain_elements {
            self.set_named_stage(1, db as f64)
        } else {
            self.set_whole_gain(db as f64)
        }
    }

    /// Address the element by the name the driver gave it, with the exact value.
    ///
    /// This is the call `setGain` was standing in for. The measured difference
    /// is in `measured-receiver-design.md`: the driver's own distribution
    /// reverses itself twice across the range, dropping the LNA by 13 dB at one
    /// point while the user is turning the gain **up**.
    fn set_stage_gain(&self, _index: usize, name: &str, db: f64) -> anyhow::Result<()> {
        if self.named_gain_elements {
            unsafe { self.api.set_gain_element(self.dev, name, db) }
                .map_err(|e| anyhow::anyhow!("{}: {e}", self.args))
        } else {
            self.set_whole_gain(db)
        }
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
                crate::hardware::StageSpec::ranged(&name, r.minimum, r.maximum, r.step)
            })
            .collect(),
        gain_element_is_switch: {
            let names = unsafe { api.gain_elements(dev) };
            names
                .iter()
                .map(|name| {
                    let r = unsafe { api.gain_element_range(dev, name) }.unwrap_or_default();
                    // Only when the step is missing and the range is real. A
                    // step that is present already answers the question, and a
                    // range that is not a range has nothing to probe.
                    if r.step > 0.0 || !(r.maximum.is_finite() && r.minimum.is_finite()) {
                        return None;
                    }
                    if r.maximum <= r.minimum {
                        return None;
                    }
                    unsafe { probe_two_value_switch(api, dev, name, r.minimum, r.maximum) }
                })
                .collect()
        },
        has_gain_mode: unsafe { api.has_gain_mode(dev) },
        bandwidth_ranges: unsafe { api.bandwidth_ranges(dev) },
        native_format,
        native_full_scale,
    }
}

/// Ask the device whether one gain element is a two-position switch.
///
/// The question is put by setting the element to a value strictly between its
/// bounds and reading it back. A switch has nowhere to put that value and snaps
/// to one of its two ends; a continuous control keeps what it was given. The
/// original setting is restored before returning, so opening a device does not
/// move its gain.
///
/// This exists because the step does not always survive the transport. A HackRF
/// through SoapyRemote reports `AMP [0, 14, step 0]`, and the bounds alone
/// cannot tell that from a continuous `[0, 40]`. Asking is the only honest way
/// left, and it is the same principle the rest of this backend follows: ask the
/// device, do not tabulate it.
///
/// `None` when the device would not answer, which is a refusal rather than a
/// `false`: an element we could not ask about is not an element we know to be
/// continuous.
///
/// # Safety
/// `dev` must be a live handle.
unsafe fn probe_two_value_switch(
    api: &SoapyApi,
    dev: *mut SoapySDRDevice,
    name: &str,
    min_db: f64,
    max_db: f64,
) -> Option<bool> {
    let before = unsafe { api.gain_element(dev, name) }?;
    let middle = min_db + (max_db - min_db) / 2.0;
    if unsafe { api.set_gain_element(dev, name, middle) }.is_err() {
        return None;
    }
    let after = unsafe { api.gain_element(dev, name) };
    // Put it back whatever the answer was. A failed restore is not worth
    // failing the open over, but it is worth not pretending happened.
    let _ = unsafe { api.set_gain_element(dev, name, before) };
    let after = after?;
    // A switch cannot hold the middle value: it lands on an end. A continuous
    // control keeps it. The tolerance is a hair, because a driver is free to
    // round the value it was handed.
    let held_the_middle = (after - middle).abs() < 1e-6;
    Some(!held_the_middle)
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
/// Everything SoapySDR reports, unfiltered. The deduplication against the
/// native backends and the audio driver's default exclusion are applied by
/// [`crate::hardware::discovery`], not here: this backend answers for itself
/// and knows nothing about the others.
pub fn list(filter: Option<&str>) -> Vec<DeviceListing> {
    let Some(api) = api::api() else {
        return Vec::new();
    };
    let query = args::enumeration_args(filter);
    api.enumerate(&query)
        .into_iter()
        .enumerate()
        .map(|(i, kwargs)| DeviceListing {
            kind: DeviceKind::Soapy,
            index: i,
            label: args::label(&kwargs, i),
            serial: args::get(&kwargs, "serial").map(str::to_string),
            args: Some(args::open_markup(&kwargs, i)),
            path: None,
            tiny_sa_input: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for l in list(None) {
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
        // The shape `caps.rs` actually produces: a driver that named no usable
        // element still gets one synthesised stage over its whole-chain range.
        // The old version of this test built `stages: vec![]`, which no device
        // can be, and so tested a clamp nothing reaches.
        let soapy = |min_db: u32, max_db: u32| {
            crate::hardware::GainModel::new(
                vec![crate::hardware::StageSpec::ranged(
                    "RF",
                    min_db as f64,
                    max_db as f64,
                    0.0,
                )],
                "RF",
                "RF",
            )
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

    /// The named setters address the stage the driver listed at that position.
    ///
    /// `set_lna_gain` and `set_vga_gain` are the two named setters the trait's
    /// default `set_stage_gain` maps onto, and both now go through
    /// `set_named_stage`. What this pins is the **index-to-name** half: a
    /// SoapyHackRF lists `LNA` first and `VGA` second, so index 0 must name
    /// `LNA` and index 1 `VGA`. Getting that backwards would move the wrong
    /// stage while reporting success, which is the failure the old whole-chain
    /// `setGain` call hid behind the driver's own distribution.
    #[test]
    fn the_named_setters_pick_the_stage_the_driver_listed() {
        let gm = crate::hardware::GainModel::new(
            vec![
                crate::hardware::StageSpec::ranged("LNA", 0.0, 40.0, 8.0),
                crate::hardware::StageSpec::ranged("VGA", 0.0, 62.0, 2.0),
            ],
            "RF",
            "RF",
        );
        let stages = gm.stages();
        assert_eq!(stages.first().map(|s| s.name.as_str()), Some("LNA"));
        assert_eq!(stages.get(1).map(|s| s.name.as_str()), Some("VGA"));
        // A device with fewer stages than the index asks for is left alone
        // rather than sent a name it never reported.
        assert!(stages.get(2).is_none());
    }
}
