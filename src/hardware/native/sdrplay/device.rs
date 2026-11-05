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

use std::sync::atomic::Ordering;

use crate::hardware::traits::RateSet;
use crate::hardware::{
    DeviceCapabilities, DeviceInfo, DeviceListing, OverloadReport, RxContext, SdrDevice,
    SoftwareStack,
};

use super::layout::{DeviceParamsT, DeviceT, RxChannelParamsT, MAX_DEVICES};
use super::stream::{Overload, StreamCtx, REASON_EXT1_NONE};
use super::{ffi, model, serial_of, Model, Problem};

/// `sdrplay_api_Tuner_A`, the RSP1B's one tuner.
const TUNER_A: libc::c_int = 1;
/// `sdrplay_api_ReasonForUpdateT`: what an `Update` tells the API changed.
const UPDATE_FRF: u32 = 0x0002_0000;
const UPDATE_GR: u32 = 0x0000_8000;
const UPDATE_AGC: u32 = 0x0100_0000;

/// What the device holds behind its lock: the radio as selected, the API's
/// parameter block for it, and the rate mode written there.
struct Selected {
    device: DeviceT,
    params: *mut DeviceParamsT,
    mode: model::RateMode,
    /// The running stream's context, kept alive until `Uninit` returns.
    stream: Option<Arc<StreamCtx>>,
}

/// An RSP1B, selected for this process until it is dropped.
pub struct SdrplayDevice {
    api: &'static ffi::SdrplayApi,
    selected: Mutex<Selected>,
    caps: DeviceCapabilities,
    info: DeviceInfo,
    notes: Vec<String>,
    /// The radio's overload flag, as the last stream's events left it.
    overload: Arc<Overload>,
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
                stream: None,
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
            overload: Arc::new(Overload::default()),
            notes: vec![format!(
                "SDRplay API {version_text}: bias tee off, AGC off, the API's DC and IQ \
                 correction on, both notches off"
            )],
        })
    }

    /// Applies what was just written into the block. Stopped, nothing is
    /// sent: `Init` reads the block as it stands. Streaming, the API takes a
    /// change only through `Update`, naming what changed; refused, the error
    /// says so and the caller puts the old value back, so the block never
    /// holds a setting the radio is not at.
    fn apply(&self, sel: &Selected, reason: u32, what: &str) -> anyhow::Result<()> {
        let streaming = sel
            .stream
            .as_ref()
            .is_some_and(|s| !s.ended.load(Ordering::Relaxed));
        if !streaming {
            return Ok(());
        }
        // SAFETY: the radio is selected and streams; the block holds the
        // change.
        let done = unsafe { (self.api.update)(sel.device.dev, TUNER_A, reason, REASON_EXT1_NONE) };
        if done != ffi::SUCCESS {
            anyhow::bail!(
                "SDRplay's API would not change the RSP1B's {what} while it streams ({})",
                self.api.error_name(done)
            );
        }
        Ok(())
    }

    /// `Init` with the block as it stands, into a new stream context kept
    /// in `sel` until `Uninit`.
    fn begin(&self, sel: &mut Selected, ctx: Arc<RxContext>) -> anyhow::Result<()> {
        let stream = Arc::new(StreamCtx::new(
            ctx,
            self.api,
            sel.device.dev,
            Arc::clone(&self.overload),
            sel.mode.output_hz(),
        ));
        // SAFETY: the radio is selected; the callbacks' context outlives the
        // stream (it is dropped only after Uninit); the API reads the
        // callback table it is given.
        let started = unsafe {
            (self.api.init)(
                sel.device.dev,
                &stream.fns as *const _ as *mut _,
                Arc::as_ptr(&stream) as *mut libc::c_void,
            )
        };
        if started != ffi::SUCCESS {
            anyhow::bail!(
                "SDRplay's API would not start the stream ({})",
                self.api.error_name(started)
            );
        }
        sel.stream = Some(stream);
        Ok(())
    }

    /// The LNA state and gRdB together, unless the pair is in the mixer
    /// trap, where the radio gives 9 to 15 dB less gain than one step down.
    fn set_pair(&self, lna: u8, gr: i32) -> anyhow::Result<()> {
        if model::in_trap(lna, gr) {
            anyhow::bail!(
                "LNA state {lna} with the IF over {:.1} dB is the RSP1B's mixer trap: the gain \
                 falls by 9 to 15 dB there instead of rising (measured); not set",
                model::IF_DB[(59 - model::TRAP_GR_FLOOR) as usize]
            );
        }
        let mut sel = self.lock();
        // SAFETY: the block is the selected radio's; the lock is held.
        let gain = &mut unsafe { Self::rx(&mut sel) }.tuner_params.gain;
        let was = (gain.lna_state, gain.gr_db);
        (gain.lna_state, gain.gr_db) = (lna, gr);
        if let Err(e) = self.apply(&sel, UPDATE_GR, "gain") {
            // SAFETY: as above.
            let gain = &mut unsafe { Self::rx(&mut sel) }.tuner_params.gain;
            (gain.lna_state, gain.gr_db) = was;
            return Err(e);
        }
        Ok(())
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

/// The LNA state a stage value names: its nearest whole state, 0 to 9.
fn lna_state_for(value: f64) -> u8 {
    value.round().clamp(0.0, 9.0) as u8
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

/// `Uninit` for a running stream, then its context let go: no callback can
/// reach it once `Uninit` has returned.
fn stop_locked(api: &ffi::SdrplayApi, sel: &mut Selected) {
    if let Some(stream) = sel.stream.take() {
        // SAFETY: the radio is selected and its stream was started here.
        unsafe { (api.uninit)(sel.device.dev) };
        drop(stream);
    }
}

impl SdrDevice for SdrplayDevice {
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.caps
    }

    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    /// `Init` with the block as it stands; the API's thread then calls back
    /// into a context kept here until `Uninit`. A stream that ended by itself
    /// (the radio removed) is stopped first.
    fn start_rx(&self, ctx: Arc<RxContext>) -> anyhow::Result<()> {
        let mut sel = self.lock();
        if let Some(running) = &sel.stream {
            if !running.ended.load(Ordering::Relaxed) {
                return Ok(());
            }
            stop_locked(self.api, &mut sel);
        }
        // A session's overload account starts with its stream; a rate
        // change, which restarts the stream, keeps it.
        self.overload.on.store(false, Ordering::Relaxed);
        self.overload.events.store(0, Ordering::Relaxed);
        self.begin(&mut sel, ctx)
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        stop_locked(self.api, &mut self.lock());
        Ok(())
    }

    /// Streaming until stopped, or until the radio left the bus or failed.
    fn is_streaming(&self) -> bool {
        self.lock()
            .stream
            .as_ref()
            .is_some_and(|s| !s.ended.load(Ordering::Relaxed))
    }

    fn samples_per_transfer(&self) -> u64 {
        self.lock()
            .stream
            .as_ref()
            .map_or(0, |s| u64::from(s.block.load(Ordering::Relaxed)))
    }

    /// The radio's own flag, as its events left it; off with no events
    /// before a stream has run.
    fn radio_overload(&self) -> Option<OverloadReport> {
        Some(OverloadReport {
            on: self.overload.on.load(Ordering::Relaxed),
            events: self.overload.events.load(Ordering::Relaxed),
        })
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
        let rx = unsafe { Self::rx(&mut sel) };
        let was = std::mem::replace(&mut rx.tuner_params.rf_freq.rf_hz, hz as f64);
        if let Err(e) = self.apply(&sel, UPDATE_FRF, "frequency") {
            // SAFETY: as above.
            unsafe { Self::rx(&mut sel) }.tuner_params.rf_freq.rf_hz = was;
            return Err(e);
        }
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
        let was = sel.mode;
        // SAFETY: the block is the selected radio's; the lock is held.
        let write = |sel: &mut Selected, mode| unsafe {
            write_mode(
                &mut *(*sel.params).dev_params,
                &mut *(*sel.params).rx_channel_a,
                mode,
            );
        };
        write(&mut sel, mode);
        sel.mode = mode;
        // Streaming, a new rate is a new stream: stopped and started again
        // in it. Changed through `Update` instead, the API's positions stop
        // describing the samples: a change of decimation moves the count by
        // 14 million with no reset, and one into or out of the low IF
        // restarts it at a rate that is not the samples' (measured). Every
        // mode started by `Init` counts true.
        let running = sel
            .stream
            .as_ref()
            .filter(|s| !s.ended.load(Ordering::Relaxed))
            .map(|s| Arc::clone(&s.rx));
        if let Some(rx) = running {
            stop_locked(self.api, &mut sel);
            if let Err(e) = self.begin(&mut sel, Arc::clone(&rx)) {
                write(&mut sel, was);
                sel.mode = was;
                let back = self.begin(&mut sel, rx);
                anyhow::bail!(
                    "{e} at {:.3} MS/s; {}",
                    mode.output_hz() / 1e6,
                    if back.is_ok() {
                        "streaming again at the rate before"
                    } else {
                        "the stream is stopped"
                    }
                );
            }
        }
        Ok(RateSet::new(mode.output_hz(), None, 0))
    }

    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()> {
        self.set_stage_gain(0, "LNA", f64::from(db))
    }

    /// The LNA by the radio's own state, 0 to 9 (its value is the state);
    /// the IF by its dB from the measured table, written as the gRdB the API
    /// takes. A pair in the mixer trap is refused ([`model::in_trap`]).
    fn set_stage_gain(&self, index: usize, _name: &str, db: f64) -> anyhow::Result<()> {
        let (lna, gr) = {
            let mut sel = self.lock();
            // SAFETY: the block is the selected radio's; the lock is held.
            let gain = &unsafe { Self::rx(&mut sel) }.tuner_params.gain;
            (gain.lna_state, gain.gr_db)
        };
        match index {
            0 => self.set_pair(lna_state_for(db), gr),
            1 => self.set_pair(lna, model::grdb_for(db)),
            _ => anyhow::bail!("the RSP1B has no gain stage {index}"),
        }
    }

    /// Both stages as one pair, checked as a pair and sent in one `Update`:
    /// a curve's step never passes through a trap on its way.
    fn set_stage_gains(&self, gains: &[f64]) -> anyhow::Result<()> {
        let (Some(&lna), Some(&if_db)) = (gains.first(), gains.get(1)) else {
            anyhow::bail!("the RSP1B takes both its stages, LNA and IF");
        };
        self.set_pair(lna_state_for(lna), model::grdb_for(if_db))
    }

    /// The API's AGC on the IF, at its own 50 Hz loop and set point, or off.
    fn set_tuner_agc(&self, on: bool) -> anyhow::Result<()> {
        let mut sel = self.lock();
        // SAFETY: the block is the selected radio's; the lock is held.
        let agc = &mut unsafe { Self::rx(&mut sel) }.ctrl_params.agc.enable;
        let was = std::mem::replace(agc, if on { 2 } else { 0 });
        if let Err(e) = self.apply(&sel, UPDATE_AGC, "AGC") {
            // SAFETY: as above.
            unsafe { Self::rx(&mut sel) }.ctrl_params.agc.enable = was;
            return Err(e);
        }
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
        stop_locked(self.api, sel);
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

    const WAIT: std::time::Duration = std::time::Duration::from_secs(5);
    const INITS: i32 = 7;
    const UNINITS: i32 = 8;
    const ACKS: i32 = 9;

    fn stream_ctx() -> (
        Arc<RxContext>,
        crossbeam_channel::Receiver<crate::hardware::StreamBlock>,
    ) {
        let (ctx, _fft, _demod, net) =
            crate::hardware::process::tests::rx_ctx_of(model::caps().sample_geometry);
        (ctx, net)
    }

    fn words(block: &crate::hardware::StreamBlock) -> Vec<i16> {
        block
            .bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_ne_bytes(*b))
            .collect()
    }

    /// **The stream delivers I and Q interleaved, at their places, gathered**:
    /// the API's first two blocks (from its position 254) go as one delivery
    /// at 0, flushed by the loss of 4 samples before the third, which starts
    /// the next delivery at 12 with the loss counted as the driver's, and
    /// that one goes when 20 ms of the rate has gathered; the
    /// radio's overload is acknowledged at once and reported; after stop
    /// nothing can call back.
    #[test]
    fn a_stream_delivers_its_blocks_with_their_losses_and_the_overload() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        assert_eq!(
            device.radio_overload(),
            Some(OverloadReport {
                on: false,
                events: 0
            })
        );
        let (ctx, net) = stream_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        let blocks: Vec<_> = (0..2).map(|_| net.recv_timeout(WAIT).unwrap()).collect();
        assert_eq!(
            blocks.iter().map(|b| b.first_pair).collect::<Vec<_>>(),
            [0, 12]
        );
        assert_eq!(
            words(&blocks[0]),
            [1, -1, 2, -2, 3, -3, 4, -4, 5, -5, 6, -6, 7, -7, 8, -8]
        );
        assert_eq!(ctx.metrics.lock().unwrap().signal.driver_drops_session, 4);
        assert!(device.is_streaming());
        let span = 40_000;
        let second = words(&blocks[1]).len() / 2;
        assert!(
            (span..span + 1008).contains(&second),
            "the second delivery gathers 20 ms of 2 MSPS, not {second}"
        );
        // A later delivery may have come by now; each is 20 ms.
        assert!((span..span + 1008).contains(&(device.samples_per_transfer() as usize)));
        assert_eq!(
            device.radio_overload(),
            Some(OverloadReport {
                on: true,
                events: 1
            })
        );
        assert_eq!(fixture.sdrplay_count(ACKS), 1, "acknowledged");
        device.stop_rx().unwrap();
        assert!(!device.is_streaming());
        assert_eq!(fixture.sdrplay_count(UNINITS), 1);
        assert_eq!(Arc::strong_count(&ctx), 1, "nothing can call back now");
    }

    const LAST_REASON: i32 = 10;
    const UPDATES: i32 = 11;

    /// **The mixer trap is refused, by a stage or by a pair**, and the block
    /// keeps what it had; a pair is checked as a pair, so a step whose
    /// halfway point is in the trap (LNA 8 with the old IF) still lands, in
    /// one `Update` while streaming.
    #[test]
    fn the_mixer_trap_is_refused_and_a_pair_goes_as_one() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        let p = |f| fixture.sdrplay_param(f);
        device.set_stage_gains(&[3.0, model::IF_DB[38]]).unwrap();
        assert_eq!((p(LNA), p(GR)), (3.0, 21.0));
        let refused = device
            .set_stage_gain(0, "LNA", 8.0)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("mixer trap"), "{refused}");
        assert_eq!((p(LNA), p(GR)), (3.0, 21.0), "the block as it was");
        assert!(device.set_stage_gains(&[9.0, model::IF_DB[37]]).is_err());
        assert_eq!((p(LNA), p(GR)), (3.0, 21.0));

        let (ctx, _net) = stream_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        device.set_stage_gains(&[8.0, model::IF_DB[35]]).unwrap();
        assert_eq!((p(LNA), p(GR)), (8.0, 24.0));
        assert_eq!(fixture.sdrplay_count(UPDATES), 1, "one Update for the pair");
        assert_eq!(fixture.sdrplay_count(LAST_REASON), 0x0000_8000);
        assert!(
            device.set_stage_gain(1, "IF", model::IF_DB[36]).is_ok(),
            "gR 23 is free"
        );
        assert!(
            device.set_stage_gain(1, "IF", model::IF_DB[37]).is_err(),
            "gR 22 is not"
        );
        device.stop_rx().unwrap();
    }

    /// **While it streams, every setting reaches the radio through an
    /// `Update` naming what changed**; stopped, none is sent, since `Init`
    /// reads the block as it stands.
    #[test]
    fn settings_reach_a_streaming_radio_through_update() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        let (ctx, _net) = stream_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        device.set_frequency(14_074_000).unwrap();
        assert_eq!(fixture.sdrplay_param(RF), 14_074_000.0);
        assert_eq!(fixture.sdrplay_count(LAST_REASON), 0x0002_0000, "Tuner_Frf");
        device.set_stage_gain(0, "LNA", 7.0).unwrap();
        assert_eq!(fixture.sdrplay_param(LNA), 7.0);
        assert_eq!(fixture.sdrplay_count(LAST_REASON), 0x0000_8000, "Tuner_Gr");
        device.set_stage_gain(1, "IF", model::IF_DB[30]).unwrap();
        assert_eq!(fixture.sdrplay_param(GR), 29.0);
        device.set_tuner_agc(true).unwrap();
        assert_eq!(fixture.sdrplay_param(AGC), 2.0);
        assert_eq!(fixture.sdrplay_count(LAST_REASON), 0x0100_0000, "Ctrl_Agc");
        assert_eq!(fixture.sdrplay_count(UPDATES), 4);
        device.stop_rx().unwrap();
        device.set_frequency(100_000_000).unwrap();
        device.set_sample_rate(6_000_000.0).unwrap();
        assert_eq!(
            fixture.sdrplay_count(UPDATES),
            4,
            "stopped, Init reads the block"
        );
        assert_eq!(fixture.sdrplay_param(RF), 100_000_000.0);
    }

    /// **A rate changed while streaming is a new stream**: stopped and
    /// started again in the new mode, never sent as an `Update`, with the
    /// deliveries after it 20 ms of the new rate and the session's overload
    /// account kept.
    #[test]
    fn a_rate_changed_while_streaming_restarts_the_stream() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        let (ctx, net) = stream_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        net.recv_timeout(WAIT).unwrap();
        // The first delivery came after the overload: it is counted.
        let overload = device.radio_overload().unwrap().events;
        assert_eq!(overload, 1);
        let set = device.set_sample_rate(10_000_000.0).unwrap();
        assert_eq!(set.rate_hz, 10_000_000.0);
        assert_eq!(
            (fixture.sdrplay_count(UNINITS), fixture.sdrplay_count(INITS)),
            (1, 2)
        );
        assert_eq!(fixture.sdrplay_count(UPDATES), 0, "no Update of a rate");
        assert_eq!(
            (fixture.sdrplay_param(FS), fixture.sdrplay_param(BW)),
            (10_000_000.0, 8000.0)
        );
        assert!(device.is_streaming());
        assert_eq!(device.converter_bits(), Some(8));
        assert!(
            device.radio_overload().unwrap().events >= overload,
            "the session's account, not reset by the restart"
        );
        let deadline = std::time::Instant::now() + WAIT;
        let pairs = loop {
            let pairs = net.recv_timeout(WAIT).unwrap().bytes.len() / 4;
            if pairs >= 100_000 {
                break pairs;
            }
            assert!(std::time::Instant::now() < deadline, "no 10 MSPS delivery");
        };
        assert!((200_000..201_008).contains(&pairs), "{pairs}");
        device.stop_rx().unwrap();
    }

    /// A new rate the stream will not start in puts the old one back and
    /// says whether the stream runs again.
    #[test]
    fn a_rate_the_stream_will_not_start_in_is_undone() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        let (ctx, _net) = stream_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        fixture.mode(48);
        let refused = device
            .set_sample_rate(10_000_000.0)
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("would not start the stream")
                && refused.contains("the stream is stopped"),
            "{refused}"
        );
        assert_eq!(
            fixture.sdrplay_param(FS),
            2_000_000.0,
            "the block as it was"
        );
        assert_eq!(device.converter_bits(), Some(14));
        assert!(!device.is_streaming());
    }

    /// **A change the API refuses while streaming** says so and leaves the
    /// block as it was, so nothing on screen claims a setting the radio is
    /// not at.
    #[test]
    fn a_refused_update_leaves_the_block_as_it_was() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        let (ctx, _net) = stream_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        fixture.mode(49);
        let before: Vec<f64> = [RF, FS, BW, IF, DEC, LNA, GR, AGC]
            .iter()
            .map(|&f| fixture.sdrplay_param(f))
            .collect();
        let refused = device.set_frequency(14_074_000).unwrap_err().to_string();
        assert!(
            refused.contains("would not change the RSP1B's frequency while it streams"),
            "{refused}"
        );
        assert!(device.set_stage_gain(0, "LNA", 1.0).is_err());
        assert!(device.set_stage_gain(1, "IF", model::IF_DB[5]).is_err());
        assert!(device.set_tuner_agc(true).is_err());
        let after: Vec<f64> = [RF, FS, BW, IF, DEC, LNA, GR, AGC]
            .iter()
            .map(|&f| fixture.sdrplay_param(f))
            .collect();
        assert_eq!(after, before);
        device.stop_rx().unwrap();
    }

    /// **The radio leaving**: what was gathered before it is delivered, the
    /// block the API sends then (position 0, no reset) is counted and never
    /// delivered, the stream reads as stopped, and a new start stops the old
    /// stream first.
    #[test]
    fn a_removed_radio_ends_the_stream_without_its_last_block() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        fixture.mode(46);
        let (ctx, net) = stream_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        let gathered = net.recv_timeout(WAIT).unwrap();
        assert_eq!(
            (gathered.first_pair, gathered.bytes.len()),
            (0, 8 * 4),
            "both blocks"
        );
        let deadline = std::time::Instant::now() + WAIT;
        while device.is_streaming() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!device.is_streaming(), "the stream ended with the radio");
        assert!(
            net.recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "the last block was not samples"
        );
        assert_eq!(
            device
                .lock()
                .stream
                .as_ref()
                .unwrap()
                .junk
                .load(Ordering::Relaxed),
            1
        );
        fixture.mode(0);
        device.start_rx(Arc::clone(&ctx)).unwrap();
        assert_eq!(
            fixture.sdrplay_count(UNINITS),
            1,
            "the ended stream stopped first"
        );
        assert_eq!(fixture.sdrplay_count(INITS), 2);
        device.stop_rx().unwrap();
    }

    /// **Across the count's wrap at 2^32** a block continues the run, with
    /// the 4 samples lost across the wrap counted where they were lost: the
    /// block before it is delivered at 0, the one after at 8.
    #[test]
    fn the_stream_continues_across_the_counts_wrap() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        fixture.mode(47);
        let (ctx, net) = stream_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        let blocks: Vec<_> = (0..2).map(|_| net.recv_timeout(WAIT).unwrap()).collect();
        assert_eq!(
            blocks.iter().map(|b| b.first_pair).collect::<Vec<_>>(),
            [0, 8]
        );
        assert_eq!(ctx.metrics.lock().unwrap().signal.driver_drops_session, 4);
        device.stop_rx().unwrap();
    }

    /// A start the API refuses keeps nothing and says why; dropping a
    /// streaming device stops the stream before it gives the radio back.
    #[test]
    fn a_refused_start_keeps_nothing_and_a_drop_stops_the_stream() {
        let fixture = TestLibrary::new(false);
        let device = SdrplayDevice::open_with_api(api(&fixture), &listing()).unwrap();
        fixture.mode(48);
        let (ctx, _net) = stream_ctx();
        let err = device.start_rx(Arc::clone(&ctx)).unwrap_err().to_string();
        assert!(err.contains("would not start the stream"), "{err}");
        assert!(!device.is_streaming());
        assert_eq!(Arc::strong_count(&ctx), 1);
        fixture.mode(0);
        device.start_rx(Arc::clone(&ctx)).unwrap();
        drop(device);
        assert_eq!(fixture.sdrplay_count(UNINITS), 1, "stopped before release");
        assert_eq!(fixture.sdrplay_state(RELEASES), 1);
        assert_eq!(Arc::strong_count(&ctx), 1);
    }
}
