// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! An RSP1B opened through SDRplay's API: selected, its parameter block
//! written to a stated start, its settings carried in that block.
//!
//! **The block is written in place and applied at `Init`.** SDRplay's API
//! hands out a pointer to its parameters for the selected radio; what is
//! written there before the stream starts is what the stream starts with.
//! So every setting here is written into the block, and nothing is asked of
//! the API until the stream starts.
//!
//! **Nothing is left to the API's defaults.** Its own start has the AGC on
//! at 50 Hz (measured on this RSP1B after `SelectDevice`); the start written
//! here states the AGC off, the bias tee off (and reads it back), the API's
//! DC and IQ corrections on, both notches off and isochronous transfers.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::hardware::traits::RateSet;
use crate::hardware::{
    DeviceCapabilities, DeviceInfo, DeviceListing, RxContext, SdrDevice, SoftwareStack,
};

use super::layout::{DeviceParamsT, DeviceT, RxChannelParamsT, MAX_DEVICES};
use super::{ffi, model, serial_of, Model, Problem};

/// What the device holds behind its lock: the radio as selected, the API's
/// parameter block for it, and the rate mode written there.
struct Selected {
    device: DeviceT,
    params: *mut DeviceParamsT,
    mode: model::RateMode,
}

/// An RSP1B, selected for this process until it is dropped.
pub struct SdrplayDevice {
    api: &'static ffi::SdrplayApi,
    selected: Mutex<Selected>,
    caps: DeviceCapabilities,
    info: DeviceInfo,
    notes: Vec<String>,
}

// SAFETY: the parameter block and the handle are only touched behind the
// `selected` lock, and the API's own calls are safe from any thread.
unsafe impl Send for SdrplayDevice {}
unsafe impl Sync for SdrplayDevice {}

/// Closes the API session it was made for unless defused: every way out of
/// [`SdrplayDevice::open_with_api`] that opened the session closes it.
struct Session(&'static ffi::SdrplayApi, bool);

impl Drop for Session {
    fn drop(&mut self) {
        if self.1 {
            // SAFETY: closes the session this guard was made for.
            unsafe { (self.0.close)() };
        }
    }
}

impl SdrplayDevice {
    /// The RSP1B a listing names, by its serial.
    pub fn open(listing: &DeviceListing) -> anyhow::Result<Self> {
        Self::open_with_api(ffi::api()?, listing)
    }

    pub(super) fn open_with_api(
        api: &'static ffi::SdrplayApi,
        listing: &DeviceListing,
    ) -> anyhow::Result<Self> {
        let Some(serial) = listing.serial.as_deref() else {
            anyhow::bail!("{}: no serial to select it by", listing.label);
        };
        // SAFETY: a call with no arguments; the guard closes what it opens.
        let opened = unsafe { (api.open)() };
        if opened != ffi::SUCCESS {
            anyhow::bail!(Problem::Open(api.error_name(opened)).text());
        }
        let mut session = Session(api, true);
        let mut version = 0f32;
        // SAFETY: writes one float.
        let asked = unsafe { (api.api_version)(&mut version) };
        if asked != ffi::SUCCESS {
            anyhow::bail!(Problem::Open(api.error_name(asked)).text());
        }
        if !ffi::is_checked(version) {
            anyhow::bail!(Problem::Version(ffi::version_text(version)).text());
        }
        let mut device = select(api, serial, &listing.label)?;
        let mut params: *mut DeviceParamsT = std::ptr::null_mut();
        // SAFETY: the handle was filled in by SelectDevice; the API writes a
        // pointer to its own block for this radio.
        let got = unsafe { (api.get_device_params)(device.dev, &mut params) };
        // SAFETY: the pointers the API handed out, checked for null first.
        let usable = got == ffi::SUCCESS
            && !params.is_null()
            && unsafe { !(*params).dev_params.is_null() && !(*params).rx_channel_a.is_null() };
        if !usable {
            // SAFETY: releases the radio selected above.
            unsafe { (api.release_device)(&mut device) };
            anyhow::bail!(
                "{}: SDRplay's API gave no parameters for it ({})",
                listing.label,
                api.error_name(got)
            );
        }
        let mode = model::mode_for(model::caps().default_sample_rate_hz)
            .expect("the default rate is one the radio makes");
        // SAFETY: the block belongs to the radio selected above and is
        // written before anything else can reach it.
        let bias_read_back = unsafe { start(params, mode, model::caps().default_frequency_hz) };
        if bias_read_back != 0 {
            // SAFETY: as above.
            unsafe { (api.release_device)(&mut device) };
            anyhow::bail!(
                "{}: the bias tee would not read back off; not opened",
                listing.label
            );
        }
        session.1 = false;
        let version_text = ffi::version_text(version);
        Ok(Self {
            api,
            selected: Mutex::new(Selected {
                device,
                params,
                mode,
            }),
            caps: model::caps(),
            info: DeviceInfo {
                board_name: "SDRplay RSP1B".into(),
                serial: serial.to_string(),
                fw_version: None,
                board_rev: None,
                usb_api_version: None,
                tuner_name: None,
                stack: Some(SoftwareStack {
                    label: "sdrplay api ",
                    value: Arc::from(version_text.as_str()),
                }),
            },
            notes: vec![format!(
                "SDRplay API {version_text}: bias tee off, AGC off, the API's DC and IQ \
                 correction on, both notches off"
            )],
        })
    }

    fn lock(&self) -> MutexGuard<'_, Selected> {
        self.selected.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One tuner's parameters, for writing behind the lock.
    ///
    /// # Safety
    /// The block belongs to the selected radio for as long as `sel` is held.
    unsafe fn rx(sel: &mut Selected) -> &mut RxChannelParamsT {
        // SAFETY: checked non-null at open; the caller holds the lock.
        unsafe { &mut *(*sel.params).rx_channel_a }
    }
}

/// Finds `serial` among the radios the API lists and selects it, under the
/// device lock. Only an RSP1B is taken.
fn select(api: &ffi::SdrplayApi, serial: &str, label: &str) -> anyhow::Result<DeviceT> {
    let mut devices = [DeviceT::default(); MAX_DEVICES];
    let mut count = 0u32;
    // SAFETY: the lock is released on every path below; GetDevices writes at
    // most MAX_DEVICES entries and their number.
    unsafe { (api.lock_device_api)() };
    let listed = unsafe { (api.get_devices)(devices.as_mut_ptr(), &mut count, MAX_DEVICES as u32) };
    let found = (listed == ffi::SUCCESS)
        .then(|| {
            devices[..(count as usize).min(MAX_DEVICES)]
                .iter()
                .find(|d| serial_of(&d.ser_no) == serial)
                .copied()
        })
        .flatten();
    let result = match found {
        None if listed != ffi::SUCCESS => {
            Err(anyhow::anyhow!(Problem::List(api.error_name(listed)).text()))
        }
        // A radio another program has selected is not in the API's list.
        None => Err(anyhow::anyhow!(
            "{label}: SDRplay's API does not list it now: in use by another program through \
             SDRplay's service, or gone"
        )),
        Some(d) if Model::from_hw_ver(d.hw_ver) != Model::Rsp1b => Err(anyhow::anyhow!(
            "{label}: only the RSP1B is driven natively; open it with --device soapy=driver=sdrplay"
        )),
        Some(mut d) => {
            // SAFETY: the entry the API listed; it fills in the handle.
            let picked = unsafe { (api.select_device)(&mut d) };
            if picked == ffi::SUCCESS && !d.dev.is_null() {
                Ok(d)
            } else {
                Err(anyhow::anyhow!(
                    "{label}: SDRplay's API would not select it ({})",
                    api.error_name(picked)
                ))
            }
        }
    };
    // SAFETY: gives back the lock taken above.
    unsafe { (api.unlock_device_api)() };
    result
}

/// Writes the stated start into a radio's block: the rate `mode`, the
/// frequency, the AGC off, the bias tee off, the corrections on, the notches
/// off, isochronous transfers, the gains at the model's start. Returns the
/// bias tee as read back, which must be 0.
///
/// # Safety
/// `params` must be the block the API handed out for a radio this process
/// has selected, with its two pointers checked non-null.
unsafe fn start(params: *mut DeviceParamsT, mode: model::RateMode, freq_hz: u64) -> u8 {
    // SAFETY: the caller's contract.
    unsafe {
        let dev = &mut *(*params).dev_params;
        let rx = &mut *(*params).rx_channel_a;
        dev.mode = 0;
        dev.rsp1a_params.rf_notch_enable = 0;
        dev.rsp1a_params.rf_dab_notch_enable = 0;
        rx.rsp1a_tuner_params.bias_t_enable = 0;
        rx.ctrl_params.agc.enable = 0;
        rx.ctrl_params.dc_offset.dc_enable = 1;
        rx.ctrl_params.dc_offset.iq_enable = 1;
        rx.tuner_params.rf_freq.rf_hz = freq_hz as f64;
        write_mode(dev, rx, mode);
        let gains = model::gain_model().start_gains(None);
        rx.tuner_params.gain.lna_state = gains[0].round().clamp(0.0, 9.0) as u8;
        rx.tuner_params.gain.gr_db = model::grdb_for(gains[1]);
        rx.rsp1a_tuner_params.bias_t_enable
    }
}

/// A rate mode into the block: the ADC's rate, the decimation, the IF and
/// its filter.
fn write_mode(
    dev: &mut super::layout::DevParamsT,
    rx: &mut RxChannelParamsT,
    mode: model::RateMode,
) {
    dev.fs_freq.fs_hz = mode.fs_hz;
    rx.ctrl_params.decimation.enable = u8::from(mode.decimation > 1);
    rx.ctrl_params.decimation.decimation_factor = mode.decimation;
    rx.tuner_params.if_type = mode.if_khz as i32;
    rx.tuner_params.bw_type = mode.bw_khz as i32;
}

impl SdrDevice for SdrplayDevice {
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.caps
    }

    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn start_rx(&self, _ctx: Arc<RxContext>) -> anyhow::Result<()> {
        anyhow::bail!(
            "sdrtop's own SDRplay backend opens and describes the RSP1B but cannot stream from \
             it yet. For samples, open it with --device soapy=driver=sdrplay"
        )
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        false
    }

    /// The datasheet's span, 1 kHz to 2 GHz: the API takes 500 Hz without
    /// complaint, so the range is held here.
    fn set_frequency(&self, hz: u64) -> anyhow::Result<u64> {
        if !(model::FREQ_MIN_HZ..=model::FREQ_MAX_HZ).contains(&hz) {
            anyhow::bail!(
                "the RSP1B tunes 1 kHz to 2 GHz, not {:.6} MHz",
                hz as f64 / 1e6
            );
        }
        let mut sel = self.lock();
        // SAFETY: the block is the selected radio's; the lock is held.
        unsafe { Self::rx(&mut sel) }.tuner_params.rf_freq.rf_hz = hz as f64;
        Ok(hz)
    }

    /// One of the rates the radio makes honestly ([`model::mode_for`]), with
    /// its ADC rate, decimation, IF and filter.
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        let Some(mode) = model::mode_for(hz) else {
            let listed: Vec<String> = model::RATES_HZ
                .iter()
                .map(|r| format!("{}", r / 1000.0))
                .collect();
            anyhow::bail!("the RSP1B runs at {} kS/s", listed.join(", "));
        };
        let mut sel = self.lock();
        // SAFETY: the block is the selected radio's; the lock is held.
        unsafe {
            let dev = &mut *(*sel.params).dev_params;
            let rx = &mut *(*sel.params).rx_channel_a;
            write_mode(dev, rx, mode);
        }
        sel.mode = mode;
        Ok(RateSet::new(mode.output_hz(), None, 0))
    }

    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()> {
        self.set_stage_gain(0, "LNA", f64::from(db))
    }

    /// The LNA by the radio's own state, 0 to 9 (its value is the state);
    /// the IF by its dB from the measured table, written as the gRdB the API
    /// takes.
    fn set_stage_gain(&self, index: usize, _name: &str, db: f64) -> anyhow::Result<()> {
        let mut sel = self.lock();
        // SAFETY: the block is the selected radio's; the lock is held.
        let rx = unsafe { Self::rx(&mut sel) };
        match index {
            0 => rx.tuner_params.gain.lna_state = db.round().clamp(0.0, 9.0) as u8,
            1 => rx.tuner_params.gain.gr_db = model::grdb_for(db),
            _ => anyhow::bail!("the RSP1B has no gain stage {index}"),
        }
        Ok(())
    }

    /// The API's AGC on the IF, at its own 50 Hz loop and set point, or off.
    fn set_tuner_agc(&self, on: bool) -> anyhow::Result<()> {
        let mut sel = self.lock();
        // SAFETY: the block is the selected radio's; the lock is held.
        unsafe { Self::rx(&mut sel) }.ctrl_params.agc.enable = if on { 2 } else { 0 };
        Ok(())
    }

    fn open_notes(&self) -> &[String] {
        &self.notes
    }

    /// The ADC's depth at the rate it runs ([`model::converter_bits`]).
    fn converter_bits(&self) -> Option<u8> {
        Some(model::converter_bits(self.lock().mode.fs_hz))
    }
}

impl Drop for SdrplayDevice {
    /// The radio back to the service, then the session closed: a radio
    /// left selected is held from every other program until the service's
    /// heartbeat gives up on this one, about three seconds after it is gone.
    fn drop(&mut self) {
        let sel = self.selected.get_mut().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the radio this device selected, released once.
        unsafe {
            (self.api.release_device)(&mut sel.device);
            (self.api.close)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::test_support::TestLibrary;
    use super::*;
    use crate::hardware::DeviceKind;

    /// The fixture's SDRplay parameters, by field (see
    /// `fixture_sdrplay_param`), and its session counters.
    const FS: i32 = 0;
    const RF: i32 = 1;
    const GR: i32 = 2;
    const LNA: i32 = 3;
    const AGC: i32 = 4;
    const BIAS: i32 = 5;
    const DC: i32 = 6;
    const IQ: i32 = 7;
    const BW: i32 = 8;
    const IF: i32 = 9;
    const DEC_ON: i32 = 10;
    const DEC: i32 = 11;
    const NOTCH: i32 = 12;
    const DAB: i32 = 13;
    const OPENS: i32 = 0;
    const CLOSES: i32 = 1;
    const SELECTS: i32 = 5;
    const RELEASES: i32 = 6;

    fn listing() -> DeviceListing {
        DeviceListing {
            kind: DeviceKind::Sdrplay,
            index: 0,
            label: "SDRplay RSP1B · 2402040E60".into(),
            serial: Some("2402040E60".into()),
            args: None,
            path: None,
            tiny_sa_input: None,
        }
    }

    fn api(fixture: &TestLibrary) -> &'static ffi::SdrplayApi {
        Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()))
    }

    /// **Opening states everything the API would default**: the AGC off where
    /// the API's own start has it on, the bias tee off, the corrections on,
    /// the notches off, 2 MSPS zero IF on the 1.536 MHz filter, 100 MHz,
    /// LNA state 4 and the IF's middle (gRdB 39). The identity is the
    /// radio's, with the API's version.
    #[test]
    fn opening_states_the_start_and_names_the_radio() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        let p = |f| fixture.sdrplay_param(f);
        assert_eq!((p(AGC), p(BIAS), p(DC), p(IQ)), (0.0, 0.0, 1.0, 1.0));
        assert_eq!((p(NOTCH), p(DAB)), (0.0, 0.0));
        assert_eq!(
            (p(FS), p(BW), p(IF), p(DEC_ON)),
            (2_000_000.0, 1536.0, 0.0, 0.0)
        );
        assert_eq!(p(RF), 100_000_000.0);
        assert_eq!((p(LNA), p(GR)), (4.0, 39.0));
        let info = device.info();
        assert_eq!(info.board_name, "SDRplay RSP1B");
        assert_eq!(info.serial, "2402040E60");
        assert_eq!(info.stack.unwrap().value.as_ref(), "3.15");
        assert_eq!(device.converter_bits(), Some(14));
        assert!(device.open_notes()[0].contains("AGC off"));
    }

    /// **The radio is given back and the session closed** when the device
    /// goes: one select, one release, one open, one close.
    #[test]
    fn dropping_releases_the_radio_and_closes_the_session() {
        let fixture = TestLibrary::new(false);
        drop(SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap());
        assert_eq!(fixture.sdrplay_state(SELECTS), 1);
        assert_eq!(fixture.sdrplay_state(RELEASES), 1);
        assert_eq!(fixture.sdrplay_state(OPENS), 1);
        assert_eq!(fixture.sdrplay_state(CLOSES), 1);
    }

    /// Another RSP, a radio the API does not list (held through the
    /// service), one it will not select, and an unchecked API version are
    /// each refused by name, with the session closed and nothing left
    /// selected.
    #[test]
    fn what_cannot_be_opened_is_refused_by_name_and_left_closed() {
        for (mode, says) in [
            (43, "only the RSP1B is driven natively"),
            (44, "does not list it now: in use by another program"),
            (45, "would not select it"),
            (42, "version 3.14"),
            (41, "needs SDRplay's service running"),
        ] {
            let fixture = TestLibrary::new(false);
            fixture.mode(mode);
            let err = match SdrplayDevice::open_with_api(api(&fixture), &listing()) {
                Ok(_) => panic!("mode {mode} opened"),
                Err(e) => e.to_string(),
            };
            assert!(err.contains(says), "mode {mode}: {err}");
            // A failed Open opened nothing; every other session is closed.
            let opened = usize::from(mode != 41) as i32;
            assert_eq!(
                fixture.sdrplay_state(CLOSES),
                opened,
                "mode {mode}: every session opened is closed"
            );
            assert_eq!(
                fixture.sdrplay_state(RELEASES),
                0,
                "mode {mode}: nothing selected"
            );
        }
    }

    /// Tuning, rates and gains reach the radio's block: a frequency in the
    /// span, refused outside it; every rate its mode (decimation, the low
    /// IF), a rate the radio cannot make refused; the LNA as its state, the
    /// IF as the gRdB its dB is; the AGC on the IF.
    #[test]
    fn settings_reach_the_radios_block() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        let p = |f| fixture.sdrplay_param(f);
        assert_eq!(device.set_frequency(14_074_000).unwrap(), 14_074_000);
        assert_eq!(p(RF), 14_074_000.0);
        assert!(device.set_frequency(500).is_err(), "the API would take it");
        assert!(device.set_frequency(2_500_000_000).is_err());
        let set = device.set_sample_rate(250_000.0).unwrap();
        assert_eq!(set.rate_hz, 250_000.0);
        assert_eq!(
            (p(FS), p(DEC_ON), p(DEC), p(BW)),
            (2_000_000.0, 1.0, 8.0, 200.0)
        );
        device.set_sample_rate(2_048_000.0).unwrap();
        assert_eq!(
            (p(FS), p(IF), p(BW), p(DEC_ON)),
            (8_192_000.0, 2048.0, 1536.0, 0.0)
        );
        assert_eq!(device.converter_bits(), Some(12), "8.192 MSPS is 12-bit");
        device.set_sample_rate(10_000_000.0).unwrap();
        assert_eq!((p(FS), p(IF), p(BW)), (10_000_000.0, 0.0, 8000.0));
        assert_eq!(device.converter_bits(), Some(8));
        assert!(
            device.set_sample_rate(666_667.0).is_err(),
            "a decimation of 3"
        );
        device.set_stage_gain(0, "LNA", 7.0).unwrap();
        device.set_stage_gain(1, "IF", model::IF_DB[30]).unwrap();
        assert_eq!((p(LNA), p(GR)), (7.0, 29.0));
        device.set_tuner_agc(true).unwrap();
        assert_eq!(p(AGC), 2.0);
        device.set_tuner_agc(false).unwrap();
        assert_eq!(p(AGC), 0.0);
    }

    /// Until the backend streams, starting says so and names the road that
    /// does.
    #[test]
    fn streaming_is_refused_with_the_soapy_road() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        let (ctx, _fft, _demod, _net) =
            crate::hardware::process::tests::rx_ctx_of(model::caps().sample_geometry);
        let err = device.start_rx(ctx).unwrap_err().to_string();
        assert!(err.contains("--device soapy=driver=sdrplay"), "{err}");
        assert!(!device.is_streaming());
    }
}
