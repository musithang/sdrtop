// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A PlutoSky R2 opened: its transmitter dealt with, its receiver set to a
//! stated start, every setting written and read back.
//!
//! **The order at open is the order of harm.** The transmitter first
//! ([`super::tx`]): silenced if nobody is using it, left alone if someone is.
//! Then the receiver: manual gain (the firmware starts in its slow AGC, and a
//! bench instrument whose gain moves by itself must not be the silent
//! default), then one RF DC offset calibration at the frequency it is on
//! (measured once on the R2 right after a power-up: a large DC and one-sided
//! clipping at high gain that this calibration removed).
//!
//! **The radio's word is the setting.** Every write is read back and the
//! reading is what sdrtop records: the LO lands on the synthesizer's step, a
//! rate on what the clock divides to, a gain on what the band allows. A gain
//! the radio moved is told once through [`SdrDevice::take_moved_stage`].
//!
//! **Streaming has a connection of its own** ([`super::stream`]); settings
//! go on this one, so a retune never waits behind a block.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::hardware::traits::RateSet;
use crate::hardware::{
    AcquisitionKind, Boost, DeliveryModel, DeviceCapabilities, DeviceInfo, DeviceListing,
    DeviceOption, GainModel, LevelUnit, OutsideChange, RxContext, SampleFormat, SampleGeometry,
    SdrDevice, SoftwareStack, StageSpec, IQ_TRACE_STALE_MS,
};

use super::gain;
use super::iiod::{Client, Target, Timeouts};
use super::stream::{self, Streaming};
use super::tx::{self, Finding};
use super::watch::{Expected, Watch};
use super::{context, identify, Model, DEFAULT_ADDRESS};

const PHY: &str = "ad9361-phy";

/// The receiver's LO range, as the radio reports it (`[70e6 1 6e9]`).
pub const FREQ_MIN_HZ: u64 = 70_000_000;
pub const FREQ_MAX_HZ: u64 = 6_000_000_000;

/// The rates offered: those sdrtop's own stream carried without a lost
/// sample over the radio's USB network on the bench machine (a minute each,
/// two requests in flight). 5 MS/s lost a block in a minute there, at the
/// edge of what the link carries (about 5.7); more waits for Ethernet.
pub const RATES_HZ: [f64; 3] = [2_083_333.0, 3_000_000.0, 4_000_000.0];

/// The radio's published floor, `2083333`, is refused when written; the next
/// integer is taken and reads back as the floor (25 MHz / 12 rounds down).
const RATE_FLOOR_WRITE: u64 = 2_083_334;

/// The analog filter's span, `rf_bandwidth_available`.
const BW_MIN_HZ: f64 = 200_000.0;
const BW_MAX_HZ: f64 = 56_000_000.0;

/// The most settings any gain-table band has (-1..73 below 1.3 GHz).
const GAIN_POSITIONS: u32 = 75;

/// Longer than discovery's: the RF DC calibration answers in about 175 ms on
/// its own and took 1.7 s while another client streamed.
const SESSION: Timeouts = Timeouts {
    connect: std::time::Duration::from_millis(1000),
    io: std::time::Duration::from_millis(5000),
};

/// The AD9361's automatic gain modes offered: the two that follow the signal
/// on the R2 (hybrid, measured, does not).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgcMode {
    Slow,
    Fast,
}

impl AgcMode {
    fn driver(self) -> &'static str {
        match self {
            AgcMode::Slow => "slow_attack",
            AgcMode::Fast => "fast_attack",
        }
    }

    fn from_driver(s: &str) -> Option<Self> {
        match s {
            "slow_attack" => Some(AgcMode::Slow),
            "fast_attack" => Some(AgcMode::Fast),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            AgcMode::Slow => "Slow attack",
            AgcMode::Fast => "Fast attack",
        }
    }
}

/// A PlutoSky R2, opened.
pub struct PlutoDevice {
    client: Mutex<Client>,
    caps: DeviceCapabilities,
    info: DeviceInfo,
    notes: Vec<String>,
    moved: Mutex<Option<(usize, f64)>>,
    /// What sdrtop set (or found another program had set); shared with the
    /// watcher, held by every setter for its write and readback.
    radio: Arc<Mutex<Expected>>,
    agc: Mutex<AgcMode>,
    address: String,
    stream_timeouts: Timeouts,
    streaming: Streaming,
    watch: Watch,
    watch_period: std::time::Duration,
}

/// How often the watcher reads the radio back while streaming.
const WATCH_PERIOD: std::time::Duration = std::time::Duration::from_secs(1);

/// The stream's waits: longer than a block at the lowest rate (31 ms) by far,
/// short enough that a radio gone quiet is said within a couple of seconds.
const STREAM: Timeouts = Timeouts {
    connect: std::time::Duration::from_millis(1000),
    io: std::time::Duration::from_millis(2000),
};

impl PlutoDevice {
    /// The R2 a listing names, at the default address.
    pub fn open(
        listing: &DeviceListing,
        settings: &crate::config::PlutoSkySettings,
    ) -> anyhow::Result<Self> {
        let dev = Self::open_at(DEFAULT_ADDRESS, listing, SESSION, STREAM)?;
        if let Some(mode) = AgcMode::from_driver(&settings.agc) {
            *dev.agc.lock().unwrap_or_else(|e| e.into_inner()) = mode;
        }
        Ok(dev)
    }

    pub(super) fn open_at(
        address: &str,
        listing: &DeviceListing,
        t: Timeouts,
        stream_timeouts: Timeouts,
    ) -> anyhow::Result<Self> {
        let refuse = |why: String| anyhow::anyhow!("{}: {why}", listing.label);
        let mut c = Client::connect(address, t).map_err(|e| refuse(e.to_string()))?;
        let version = c.version().map_err(|e| refuse(e.to_string()))?;
        let xml = c.print().map_err(|e| refuse(e.to_string()))?;
        let found = identify(
            &context::parse(&xml),
            &version,
            listing.serial.as_deref(),
            address,
        )
        .map_err(|p| anyhow::anyhow!(p.text()))?;
        if let Model::Other(model) = &found.model {
            return Err(refuse(format!(
                "{model} is not a PlutoSky R2. Open it through SoapySDR: \
                 sdrtop --device soapy=driver=plutosdr"
            )));
        }

        let mut notes = Vec::new();
        let state = tx::read_state(&mut c).map_err(&refuse)?;
        match tx::assess(&state) {
            Finding::Silent => notes.push(
                "PlutoSky: the transmitter was already silent (TX LO powered down, both \
                 channels at the most attenuation)."
                    .to_string(),
            ),
            Finding::Idle => {
                tx::silence(&mut c).map_err(|why| {
                    refuse(format!("the transmitter would not be silenced: {why}"))
                })?;
                notes.push(format!(
                    "PlutoSky: the transmitter was idle with its LO {} (TX1 {} dB, TX2 {} dB); \
                     silenced: both channels at {} dB, TX LO powered down.",
                    if state.lo_powered_down {
                        "powered down"
                    } else {
                        "running"
                    },
                    state.gains_db[0],
                    state.gains_db[1],
                    tx::MOST_ATTENUATION_DB
                ));
            }
            Finding::Other { radiating, what } => notes.push(format!(
                "PlutoSky: another program has the transmitter ({what}{}). sdrtop leaves it \
                 alone; in FDD its signal leaks into the receiver.",
                if radiating {
                    ", LO running"
                } else {
                    ", LO powered down"
                }
            )),
        }

        let rx = Target::Input(PHY, "voltage0");
        c.write(rx, "gain_control_mode", "manual")
            .map_err(|e| refuse(format!("manual gain: {e}")))?;
        let mode = c
            .read(rx, "gain_control_mode")
            .map_err(|e| refuse(e.to_string()))?;
        if mode != "manual" {
            return Err(refuse(format!(
                "the gain stayed in {mode} after manual was set"
            )));
        }
        let port = c
            .read(rx, "rf_port_select")
            .map_err(|e| refuse(e.to_string()))?;
        if port != "A_BALANCED" {
            notes.push(format!(
                "PlutoSky: the receiver's port is {port}, not A_BALANCED."
            ));
        }
        // The tracking loops the I/Q bench's readings are stated after: on,
        // as the firmware starts them, whatever another program left.
        for lp in [
            "quadrature_tracking_en",
            "rf_dc_offset_tracking_en",
            "bb_dc_offset_tracking_en",
        ] {
            let on = c.read(rx, lp).map_err(|e| refuse(format!("{lp}: {e}")))?;
            if on != "1" {
                c.write(rx, lp, "1")
                    .map_err(|e| refuse(format!("{lp}: {e}")))?;
                if c.read(rx, lp).map_err(|e| refuse(e.to_string()))? != "1" {
                    return Err(refuse(format!("{lp} stays off")));
                }
                notes.push(format!("PlutoSky: {lp} was off; switched on."));
            }
        }
        c.write(Target::Device(PHY), "calib_mode", "rf_dc_offs")
            .map_err(|e| refuse(format!("RF DC calibration: {e}")))?;
        notes.push(format!(
            "PlutoSky: iiod {} at {address}; gain manual, RF DC offset calibrated.",
            found.server
        ));

        let info = DeviceInfo {
            board_name: "PlutoSky R2".to_string(),
            serial: found.serial.clone(),
            fw_version: found.firmware.clone(),
            board_rev: None,
            usb_api_version: None,
            tuner_name: Some("AD9361".to_string()),
            // The radio's own firmware, under its own name (ten columns, as
            // the header's field is laid out).
            stack: Some(SoftwareStack {
                label: "pluto fw  ",
                value: Arc::from(found.firmware.as_deref().unwrap_or("\u{2014}")),
            }),
        };
        // The rate the radio runs at now, for the stream's reckoning until
        // one is set.
        let streaming = Streaming::default();
        let rate = c
            .read(rx, "sampling_frequency")
            .ok()
            .and_then(|v| tx::parse_db(&v))
            .unwrap_or(0.0);
        streaming.set_rate(rate);
        let lo_hz = Self::number(&mut c, Target::Output(PHY, "altvoltage0"), "frequency")
            .map_err(|e| refuse(e.to_string()))? as u64;
        let bottom_db = Self::bottom(&mut c).map_err(|e| refuse(e.to_string()))?;
        let hw = Self::number(&mut c, rx, "hardwaregain").map_err(|e| refuse(e.to_string()))?;
        let bw_hz =
            Self::number(&mut c, rx, "rf_bandwidth").map_err(|e| refuse(e.to_string()))? as u32;
        let expected = Expected {
            position: (hw - f64::from(bottom_db)).max(0.0),
            bottom_db,
            lo_hz,
            rate_hz: rate,
            bw_hz,
            agc_on: false,
            // An idle transmitter was silenced above; only another program's
            // signal on the air is left to say.
            other_tx: match tx::assess(&state) {
                f @ Finding::Other { .. } => super::watch::transmitting(&f, state.lo_powered_down),
                _ => None,
            },
        };
        Ok(Self {
            client: Mutex::new(c),
            caps: caps(),
            info,
            notes,
            moved: Mutex::new(None),
            radio: Arc::new(Mutex::new(expected)),
            agc: Mutex::new(AgcMode::Slow),
            address: address.to_string(),
            stream_timeouts,
            streaming,
            watch: Watch::default(),
            watch_period: WATCH_PERIOD,
        })
    }

    fn client(&self) -> MutexGuard<'_, Client> {
        self.client.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Taken after [`Self::client`], never before, by every setter.
    fn expected(&self) -> MutexGuard<'_, Expected> {
        self.radio.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn number(c: &mut Client, target: Target<'_>, attr: &str) -> anyhow::Result<f64> {
        let v = c.read(target, attr)?;
        tx::parse_db(&v).ok_or_else(|| anyhow::anyhow!("{target} {attr} reads {v:?}"))
    }

    /// The lowest `hardwaregain` of the band the radio is tuned to, as it
    /// reports it (`[-3 1 71]`).
    fn bottom(c: &mut Client) -> anyhow::Result<i32> {
        let v = c.read(Target::Input(PHY, "voltage0"), "hardwaregain_available")?;
        v.trim_start_matches('[')
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<f64>().ok())
            .map(|n| n.round() as i32)
            .ok_or_else(|| anyhow::anyhow!("hardwaregain_available reads {v:?}"))
    }

    /// The AGC on (in the chosen mode) or off, read back. Off, the radio
    /// keeps the AGC's last gain, so the setting sdrtop holds is written
    /// again.
    fn set_agc(&self, on: bool) -> anyhow::Result<()> {
        let mode = *self.agc.lock().unwrap_or_else(|e| e.into_inner());
        let want = if on { mode.driver() } else { "manual" };
        let mut c = self.client();
        let mut g = self.expected();
        let rx = Target::Input(PHY, "voltage0");
        c.write(rx, "gain_control_mode", want)?;
        let got = c.read(rx, "gain_control_mode")?;
        if got != want {
            anyhow::bail!("the gain mode reads {got} after {want} was set");
        }
        g.agc_on = on;
        if !on {
            let position = g.position;
            self.apply(&mut c, &mut g, position)?;
        }
        Ok(())
    }

    /// The last setting measured at `hz`, which the radio is kept within.
    fn last_measured(&self, hz: u64) -> f64 {
        self.caps.gain.stages()[0]
            .band_at(hz)
            .map_or(f64::from(GAIN_POSITIONS - 1), |b| {
                b.db.len().saturating_sub(1) as f64
            })
    }

    /// Writes the setting `position` as `hardwaregain` and reads it back;
    /// a position the radio or the measurements put elsewhere is told once.
    fn apply(&self, c: &mut Client, g: &mut Expected, position: f64) -> anyhow::Result<()> {
        let want = position.round().clamp(0.0, self.last_measured(g.lo_hz));
        let rx = Target::Input(PHY, "voltage0");
        c.write(
            rx,
            "hardwaregain",
            &format!("{}", f64::from(g.bottom_db) + want),
        )?;
        let got = Self::number(c, rx, "hardwaregain")? - f64::from(g.bottom_db);
        if (got - position.round()).abs() > 0.01 {
            *self.moved.lock().unwrap_or_else(|e| e.into_inner()) = Some((0, got));
        }
        g.position = got;
        Ok(())
    }
}

/// What the R2 is, for the core.
pub fn caps() -> DeviceCapabilities {
    DeviceCapabilities {
        level_unit: LevelUnit::Dbfs,
        level_min_db: -120.0,
        level_max_db: 0.0,
        trace_stale_ms: IQ_TRACE_STALE_MS,
        acquisition: AcquisitionKind::IqSamples,
        sample_rate_is_span: false,
        freq_min_hz: FREQ_MIN_HZ,
        freq_max_hz: FREQ_MAX_HZ,
        freq_gaps: Vec::new(),
        sample_rate_min_hz: RATES_HZ[0],
        sample_rate_max_hz: RATES_HZ[RATES_HZ.len() - 1],
        sample_rates: RATES_HZ.to_vec(),
        default_frequency_hz: 433_920_000,
        default_sample_rate_hz: 3_000_000.0,
        // 12 bits, sign-extended into int16: -2048..2047 measured on a
        // clipped broadcast band.
        sample_geometry: SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 2048.0,
            converter_bits: Some(12),
        },
        // The setting is a position from the band's bottom; its decibels are
        // the measured ones at the nearest measured frequency. `[A]` is the
        // chip's own AGC.
        gain: GainModel::new(
            vec![StageSpec::banded(
                "hardwaregain",
                GAIN_POSITIONS,
                gain::tables(&gain::points(gain::MEASURED)),
            )],
            "Gain",
            "GAIN",
        )
        .with_boost(Boost::GainMode),
        samples_per_transfer: stream::BLOCK_PAIRS as u64,
        // `rf_bandwidth` sets the chip's analog filters.
        has_bb_filter: true,
        // No stage's own noise figure is published.
        friis_applicable: false,
        // sdrtop asks (`READBUF`) and the server answers.
        delivery: DeliveryModel::Pull,
        driver_queue_blocks: None,
    }
}

impl SdrDevice for PlutoDevice {
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.caps
    }

    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    /// The stream, and the watcher beside it while it runs.
    fn start_rx(&self, ctx: Arc<RxContext>) -> anyhow::Result<()> {
        self.streaming
            .start(&self.address, self.stream_timeouts, ctx)?;
        self.watch.start(
            &self.address,
            self.stream_timeouts,
            self.watch_period,
            Arc::clone(&self.radio),
            self.streaming.rate_handle(),
        );
        Ok(())
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        self.watch.stop();
        self.streaming.stop();
        Ok(())
    }

    fn take_outside_changes(&self) -> Vec<OutsideChange> {
        self.watch.take()
    }

    fn is_streaming(&self) -> bool {
        self.streaming.is_active()
    }

    fn stream_end_reason(&self) -> Option<String> {
        self.streaming.ended().map(|e| stream::text(&e))
    }

    fn read_loop_us(&self) -> Option<(u64, u64)> {
        Some(self.streaming.clock())
    }

    /// Tunes, then keeps the gain's setting where it was in the new band:
    /// across a gain-table band edge the bottom moves (-1, -3, -10 dB), and a
    /// measured table may end lower, so `hardwaregain` is written again.
    fn set_frequency(&self, hz: u64) -> anyhow::Result<u64> {
        let mut c = self.client();
        let mut g = self.expected();
        let lo = Target::Output(PHY, "altvoltage0");
        c.write(lo, "frequency", &hz.to_string())?;
        let tuned = Self::number(&mut c, lo, "frequency")? as u64;
        let bottom = Self::bottom(&mut c)?;
        let before = g.clone();
        g.lo_hz = tuned;
        g.bottom_db = bottom;
        if bottom != before.bottom_db || before.position > self.last_measured(tuned) {
            self.apply(&mut c, &mut g, before.position)?;
        }
        Ok(tuned)
    }

    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        let mut c = self.client();
        let mut g = self.expected();
        let rx = Target::Input(PHY, "voltage0");
        let want = (hz.round() as u64).max(RATE_FLOOR_WRITE - 1);
        let write = if want == RATE_FLOOR_WRITE - 1 {
            RATE_FLOOR_WRITE
        } else {
            want
        };
        c.write(rx, "sampling_frequency", &write.to_string())?;
        let rate = Self::number(&mut c, rx, "sampling_frequency")?;
        // The analog filter follows the rate, inside what the chip offers.
        let bw = rate.clamp(BW_MIN_HZ, BW_MAX_HZ).round() as u64;
        c.write(rx, "rf_bandwidth", &bw.to_string())?;
        let bw = Self::number(&mut c, rx, "rf_bandwidth")?;
        self.streaming.set_rate(rate);
        g.rate_hz = rate;
        g.bw_hz = bw as u32;
        Ok(RateSet::new(hz, Some(rate), bw as u32))
    }

    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()> {
        self.set_stage_gain(0, "hardwaregain", f64::from(db))
    }

    /// `value` is the setting, a position from the band's bottom.
    fn set_stage_gain(&self, index: usize, _name: &str, value: f64) -> anyhow::Result<()> {
        if index != 0 {
            return Ok(());
        }
        let mut c = self.client();
        let mut g = self.expected();
        self.apply(&mut c, &mut g, value)
    }

    fn set_amp_enable(&self, on: bool) -> anyhow::Result<()> {
        self.set_agc(on)
    }

    fn set_tuner_agc(&self, on: bool) -> anyhow::Result<()> {
        self.set_agc(on)
    }

    /// Under the AGC, the gain it chose, in the measured dB of that setting.
    fn auto_stage_db(&self, index: usize) -> Option<f64> {
        let mut c = self.client();
        let g = self.expected().clone();
        if index != 0 || !g.agc_on {
            return None;
        }
        let hw = Self::number(&mut c, Target::Input(PHY, "voltage0"), "hardwaregain").ok()?;
        self.caps.gain.stages()[0].gain_db_at(hw - f64::from(g.bottom_db), g.lo_hz)
    }

    fn options(&self) -> Vec<DeviceOption> {
        let mode = *self.agc.lock().unwrap_or_else(|e| e.into_inner());
        vec![DeviceOption {
            id: "agc".into(),
            label: "AGC mode".into(),
            choices: vec![AgcMode::Slow.label().into(), AgcMode::Fast.label().into()],
            selected_choice: mode.label().into(),
            integer_range: None,
        }]
    }

    /// The mode `[A]` switches on; taken at once if the AGC is on.
    fn set_option(&self, id: &str, choice: &str) -> anyhow::Result<()> {
        if id != "agc" {
            anyhow::bail!("the PlutoSky R2 has no option {id}");
        }
        let mode = [AgcMode::Slow, AgcMode::Fast]
            .into_iter()
            .find(|m| m.label() == choice)
            .ok_or_else(|| anyhow::anyhow!("{choice} is not a choice for {id}"))?;
        *self.agc.lock().unwrap_or_else(|e| e.into_inner()) = mode;
        if self.expected().agc_on {
            self.set_agc(true)?;
        }
        Ok(())
    }

    fn update_config(&self, config: &mut crate::config::EngineConfig) -> anyhow::Result<()> {
        config.plutosky.agc = self
            .agc
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .driver()
            .into();
        Ok(())
    }

    /// The AD9361's quadrature and DC tracking loops run on every R2 sdrtop
    /// opens (the firmware's own default, kept): the image and the DC the
    /// I/Q bench measures are what is left after them.
    fn upstream_correction(&self) -> Option<&'static str> {
        Some("after the AD9361's tracking loops")
    }

    fn take_moved_stage(&self) -> Option<(usize, f64)> {
        self.moved.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    fn open_notes(&self) -> &[String] {
        &self.notes
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::super::iiod::fake;
    use super::*;
    use crate::hardware::DeviceKind;

    const R2_SERIAL: &str = "1044730231960000000000000000000001";

    fn quick() -> Timeouts {
        Timeouts {
            connect: Duration::from_millis(500),
            io: Duration::from_millis(500),
        }
    }

    fn listing() -> DeviceListing {
        DeviceListing {
            kind: DeviceKind::PlutoSky,
            index: 0,
            label: format!("PlutoSky R2 \u{00b7} {R2_SERIAL}"),
            serial: Some(R2_SERIAL.into()),
            args: None,
            path: None,
            tiny_sa_input: None,
        }
    }

    /// The R2 as it reads after power-up (recorded on the bench).
    fn r2_after_power_up() -> fake::Radio {
        let mut attrs = HashMap::new();
        let mut put = |k: &str, v: &str| {
            attrs.insert(k.to_string(), v.to_string());
        };
        put("ad9361-phy OUTPUT altvoltage1 powerdown", "0");
        put("ad9361-phy OUTPUT voltage0 hardwaregain", "-10.000000 dB");
        put("ad9361-phy OUTPUT voltage1 hardwaregain", "-10.000000 dB");
        for i in 0..8 {
            put(
                &format!("cf-ad9361-dds-core-lpc OUTPUT altvoltage{i} scale"),
                "0.000000",
            );
        }
        put("ad9361-phy INPUT voltage0 gain_control_mode", "slow_attack");
        put("ad9361-phy INPUT voltage0 hardwaregain", "71.000000 dB");
        put(
            "ad9361-phy INPUT voltage0 hardwaregain_available",
            "[-3 1 71]",
        );
        put("ad9361-phy INPUT voltage0 rf_port_select", "A_BALANCED");
        // 3 MS/s rather than the power-up 30.72, so the stand-in's paced
        // blocks (21.8 ms each) are easy for a test to keep up with.
        put("ad9361-phy INPUT voltage0 sampling_frequency", "3000000");
        put("ad9361-phy INPUT voltage0 rf_bandwidth", "18000000");
        put("ad9361-phy OUTPUT altvoltage0 frequency", "2400000000");
        put("ad9361-phy calib_mode", "auto");
        for lp in [
            "quadrature_tracking_en",
            "rf_dc_offset_tracking_en",
            "bb_dc_offset_tracking_en",
        ] {
            put(&format!("ad9361-phy INPUT voltage0 {lp}"), "1");
        }
        fake::Radio {
            version: "0.25.v0.25  \n".into(),
            xml: include_str!("../fixtures/pluto-r2-context.xml").into(),
            attrs,
            ..Default::default()
        }
    }

    fn written(radio: &Arc<Mutex<fake::Radio>>) -> Vec<String> {
        radio
            .lock()
            .unwrap()
            .written
            .iter()
            .filter(|w| !w.contains("direct_reg_access"))
            .cloned()
            .collect()
    }

    /// The firmware's idle transmitter is silenced, attenuation before the
    /// LO, then the receiver set to manual and calibrated, in that order.
    #[test]
    fn opening_silences_the_idle_transmitter_first() {
        let (addr, radio) = fake::serve(r2_after_power_up());
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        assert_eq!(
            written(&radio),
            [
                "ad9361-phy OUTPUT voltage0 hardwaregain=-89.75",
                "ad9361-phy OUTPUT voltage1 hardwaregain=-89.75",
                "ad9361-phy OUTPUT altvoltage1 powerdown=1",
                "ad9361-phy INPUT voltage0 gain_control_mode=manual",
                "ad9361-phy calib_mode=rf_dc_offs",
            ]
        );
        assert!(
            dev.open_notes()[0].contains("silenced"),
            "{:?}",
            dev.open_notes()
        );
        assert_eq!(dev.info().board_name, "PlutoSky R2");
        assert_eq!(dev.info().serial, R2_SERIAL);
        assert_eq!(dev.info().fw_version.as_deref(), Some("v0.38"));
    }

    /// A radio that will not take the attenuation refuses the open, before
    /// anything on the receiver is touched.
    #[test]
    fn a_transmitter_that_will_not_be_silenced_refuses_the_open() {
        let mut r = r2_after_power_up();
        r.ignores
            .insert("ad9361-phy OUTPUT voltage1 hardwaregain".into());
        let (addr, radio) = fake::serve(r);
        let err = PlutoDevice::open_at(&addr, &listing(), quick(), quick())
            .err()
            .expect("the open is refused")
            .to_string();
        assert!(err.contains("would not be silenced"), "{err}");
        assert!(err.contains("TX2 reads -10"), "{err}");
        assert!(
            !written(&radio).iter().any(|w| w.contains("INPUT")),
            "the receiver is not touched: {:?}",
            written(&radio)
        );
    }

    /// Another program's tone: nothing on the TX side is written.
    #[test]
    fn another_programs_transmission_is_left_alone() {
        let mut r = r2_after_power_up();
        r.attrs.insert(
            "cf-ad9361-dds-core-lpc OUTPUT altvoltage0 scale".into(),
            "0.099976".into(),
        );
        let (addr, radio) = fake::serve(r);
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        assert!(
            !written(&radio).iter().any(|w| w.contains("OUTPUT")),
            "{:?}",
            written(&radio)
        );
        assert!(
            dev.open_notes()[0].contains("another program has the transmitter"),
            "{:?}",
            dev.open_notes()
        );
    }

    /// A buffer of samples shows only in the DAC's source register.
    #[test]
    fn a_dma_source_is_another_programs_transmission() {
        let mut r = r2_after_power_up();
        r.regs
            .insert(("cf-ad9361-dds-core-lpc".into(), 0x8000_0418), 0x2);
        let (addr, radio) = fake::serve(r);
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        assert!(!written(&radio).iter().any(|w| w.contains("OUTPUT")));
        assert!(dev.open_notes()[0].contains("buffer of samples"));
    }

    #[test]
    fn an_already_silent_transmitter_is_not_written_again() {
        let mut r = r2_after_power_up();
        r.attrs
            .insert("ad9361-phy OUTPUT altvoltage1 powerdown".into(), "1".into());
        for ch in ["voltage0", "voltage1"] {
            r.attrs.insert(
                format!("ad9361-phy OUTPUT {ch} hardwaregain"),
                "-89.750000 dB".into(),
            );
        }
        let (addr, radio) = fake::serve(r);
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        assert!(!written(&radio).iter().any(|w| w.contains("OUTPUT")));
        assert!(dev.open_notes()[0].contains("already silent"));
    }

    /// A tracking loop another program switched off is switched on again,
    /// since the I/Q bench says its readings are after them.
    #[test]
    fn a_tracking_loop_left_off_is_switched_on() {
        let mut r = r2_after_power_up();
        r.attrs.insert(
            "ad9361-phy INPUT voltage0 quadrature_tracking_en".into(),
            "0".into(),
        );
        let (addr, radio) = fake::serve(r);
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        assert!(written(&radio)
            .contains(&"ad9361-phy INPUT voltage0 quadrature_tracking_en=1".to_string()));
        assert!(dev
            .open_notes()
            .iter()
            .any(|n| n.contains("quadrature_tracking_en was off")));
    }

    #[test]
    fn a_receiver_stuck_in_its_agc_refuses_the_open() {
        let mut r = r2_after_power_up();
        r.ignores
            .insert("ad9361-phy INPUT voltage0 gain_control_mode".into());
        let (addr, _) = fake::serve(r);
        let err = PlutoDevice::open_at(&addr, &listing(), quick(), quick())
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("stayed in slow_attack"), "{err}");
    }

    /// Another radio at the address is refused, whatever it is.
    #[test]
    fn a_radio_with_another_serial_is_not_opened() {
        let (addr, radio) = fake::serve(r2_after_power_up());
        let mut l = listing();
        l.serial = Some("deadbeef".into());
        assert!(PlutoDevice::open_at(&addr, &l, quick(), quick()).is_err());
        assert!(written(&radio).is_empty(), "nothing written to a stranger");
    }

    /// The radio's readback is the setting: the floor written one higher,
    /// the filter following the rate.
    #[test]
    fn settings_are_what_the_radio_reads_back() {
        let (addr, radio) = fake::serve(r2_after_power_up());
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        let set = dev.set_sample_rate(2_083_333.0).unwrap();
        assert_eq!(
            set.rate_hz, 2_083_334.0,
            "the stand-in keeps what it is given"
        );
        assert_eq!(set.bb_filter_hz, 2_083_334);
        assert!(written(&radio)
            .contains(&"ad9361-phy INPUT voltage0 sampling_frequency=2083334".to_string()));
    }

    /// The setting is a position from the band's bottom: 20 is 17 dB at
    /// 2.4 GHz (bottom -3) and 19 dB at 433 MHz (bottom -1), and a retune
    /// across the edge writes it again so the position holds.
    #[test]
    fn the_gain_setting_holds_its_position_across_a_band_edge() {
        let (addr, radio) = fake::serve(r2_after_power_up());
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        dev.set_stage_gain(0, "hardwaregain", 20.0).unwrap();
        assert!(written(&radio).contains(&"ad9361-phy INPUT voltage0 hardwaregain=17".to_string()));
        assert_eq!(dev.take_moved_stage(), None);
        assert_eq!(dev.set_frequency(433_500_000).unwrap(), 433_500_000);
        assert_eq!(
            written(&radio).last().unwrap(),
            "ad9361-phy INPUT voltage0 hardwaregain=19"
        );
        assert_eq!(dev.take_moved_stage(), None, "the position held");
    }

    /// Where the measurements stop (2.4 GHz above 66 dB), the radio is kept
    /// at the last measured setting, and the move is told once.
    #[test]
    fn the_gain_stays_within_what_was_measured() {
        let (addr, radio) = fake::serve(r2_after_power_up());
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        dev.set_stage_gain(0, "hardwaregain", 74.0).unwrap();
        assert_eq!(
            written(&radio).last().unwrap(),
            "ad9361-phy INPUT voltage0 hardwaregain=66"
        );
        assert_eq!(dev.take_moved_stage(), Some((0, 69.0)));
        assert_eq!(dev.take_moved_stage(), None, "told once");
        // A radio that will not take the gain: its own value is the setting.
        radio
            .lock()
            .unwrap()
            .ignores
            .insert("ad9361-phy INPUT voltage0 hardwaregain".into());
        dev.set_stage_gain(0, "hardwaregain", 10.0).unwrap();
        assert_eq!(dev.take_moved_stage(), Some((0, 69.0)), "the radio's word");
    }

    const WAIT: Duration = Duration::from_secs(5);

    fn streaming_ctx() -> (
        Arc<RxContext>,
        crossbeam_channel::Receiver<crate::hardware::StreamBlock>,
    ) {
        let (ctx, _fft, _demod, net) =
            crate::hardware::process::tests::rx_ctx_of(caps().sample_geometry);
        (ctx, net)
    }

    fn pairs_of(block: &crate::hardware::StreamBlock) -> Vec<(i16, i16)> {
        block
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| {
                (
                    i16::from_le_bytes([b[0], b[1]]),
                    i16::from_le_bytes([b[2], b[3]]),
                )
            })
            .collect()
    }

    /// Blocks arrive whole and in order on a connection of their own, and
    /// stopping closes the buffer and lets the context go.
    #[test]
    fn blocks_arrive_whole_and_in_order() {
        let (addr, radio) = fake::serve(r2_after_power_up());
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        let (ctx, net) = streaming_ctx();
        dev.start_rx(Arc::clone(&ctx)).unwrap();
        assert!(dev.is_streaming());
        let blocks: Vec<_> = (0..3).map(|_| net.recv_timeout(WAIT).unwrap()).collect();
        // Gaps are not asserted here: the stand-in, unlike a radio, can fall
        // behind its own clock on a loaded machine, and the reckoning would
        // rightly count that. The reckoning has its own tests.
        assert_eq!(blocks[0].first_pair, 0);
        for b in &blocks {
            assert_eq!(b.first_pair % 65_536, 0, "whole blocks");
            assert_eq!(b.bytes.len(), 65_536 * 4);
            // The counting pattern runs on unbroken inside every block.
            let p = pairs_of(b);
            assert!(p
                .windows(2)
                .all(|w| (w[1].0 - w[0].0).rem_euclid(2048) == 1));
            assert!(p.iter().all(|&(i, q)| q == -i));
        }
        assert_eq!(pairs_of(&blocks[0])[..3], [(0, 0), (1, -1), (2, -2)]);
        dev.stop_rx().unwrap();
        assert!(!dev.is_streaming());
        assert!(
            radio.lock().unwrap().open_samples.is_none(),
            "the buffer closed"
        );
        assert_eq!(Arc::strong_count(&ctx), 1, "nothing can deliver now");
        assert_eq!(dev.stream_end_reason(), None, "stopped, not ended");
    }

    /// Another client widened the stream to both receivers: RX1 is picked out
    /// by the mask the server sends.
    #[test]
    fn rx1_is_picked_out_when_another_client_widened_the_stream() {
        let mut r = r2_after_power_up();
        r.channels = 4;
        r.mask_reply = Some("0000000f".into());
        let (addr, _) = fake::serve(r);
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        let (ctx, net) = streaming_ctx();
        dev.start_rx(Arc::clone(&ctx)).unwrap();
        let block = net.recv_timeout(WAIT).unwrap();
        assert_eq!(pairs_of(&block)[..3], [(0, 0), (1, -1), (2, -2)]);
        assert_eq!(
            block.bytes.len(),
            65_536 * 4 / 2,
            "half the pairs per request"
        );
        dev.stop_rx().unwrap();
    }

    /// A radio that stops answering mid-stream ends the stream within the
    /// timeout, and says why.
    #[test]
    fn a_radio_gone_quiet_is_a_stated_stop() {
        let mut r = r2_after_power_up();
        r.vanish_after = Some(2);
        let (addr, _) = fake::serve(r);
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        let (ctx, _net) = streaming_ctx();
        dev.start_rx(Arc::clone(&ctx)).unwrap();
        let started = std::time::Instant::now();
        while dev.is_streaming() && started.elapsed() < WAIT {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!dev.is_streaming(), "the stream ended by itself");
        let why = dev.stream_end_reason().unwrap();
        assert!(why.contains("stopped answering"), "{why}");
        dev.stop_rx().unwrap();
        assert_eq!(Arc::strong_count(&ctx), 1);
    }

    /// An empty reply (another client changing the channels) is said once and
    /// the stream carries on.
    #[test]
    fn an_empty_reply_is_said_and_the_stream_carries_on() {
        let mut r = r2_after_power_up();
        r.empty_replies = 1;
        let (addr, _) = fake::serve(r);
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        let (ctx, net) = streaming_ctx();
        dev.start_rx(Arc::clone(&ctx)).unwrap();
        net.recv_timeout(WAIT).unwrap();
        dev.stop_rx().unwrap();
        let m = ctx.metrics.lock().unwrap();
        let said: Vec<&str> = m.log.iter().map(|e| e.text.as_ref()).collect();
        assert!(
            said.iter()
                .any(|l| l.contains("answered a block with nothing")),
            "{said:?}"
        );
    }

    /// `[A]` switches the chip's AGC in the chosen mode; off again, the
    /// setting sdrtop holds is written back, since manual keeps the AGC's.
    #[test]
    fn the_agc_comes_and_goes_and_the_setting_returns() {
        let (addr, radio) = fake::serve(r2_after_power_up());
        let dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        dev.set_stage_gain(0, "hardwaregain", 20.0).unwrap();
        dev.set_tuner_agc(true).unwrap();
        assert_eq!(
            written(&radio).last().unwrap(),
            "ad9361-phy INPUT voltage0 gain_control_mode=slow_attack"
        );
        // The AGC moves the gain; its choice is shown in measured dB.
        radio.lock().unwrap().attrs.insert(
            "ad9361-phy INPUT voltage0 hardwaregain".into(),
            "40.000000 dB".into(),
        );
        let db = dev.auto_stage_db(0).unwrap();
        assert!((35.0..42.0).contains(&db), "{db}");
        dev.set_option("agc", "Fast attack").unwrap();
        assert_eq!(
            written(&radio).last().unwrap(),
            "ad9361-phy INPUT voltage0 gain_control_mode=fast_attack",
            "taken at once while on"
        );
        dev.set_tuner_agc(false).unwrap();
        let w = written(&radio);
        assert_eq!(
            w[w.len() - 2..],
            [
                "ad9361-phy INPUT voltage0 gain_control_mode=manual".to_string(),
                "ad9361-phy INPUT voltage0 hardwaregain=17".to_string()
            ]
        );
        assert_eq!(dev.auto_stage_db(0), None, "not under the AGC");
        let mut cfg = crate::config::EngineConfig::default();
        dev.update_config(&mut cfg).unwrap();
        assert_eq!(cfg.plutosky.agc, "fast_attack");
        assert!(dev.set_option("agc", "Hybrid").is_err(), "not offered");
    }

    /// While streaming, another program's retune and transmission are found
    /// and handed over, once; sdrtop's own retune is not mistaken for one.
    #[test]
    fn another_programs_changes_are_found_while_streaming() {
        let (addr, radio) = fake::serve(r2_after_power_up());
        let mut dev = PlutoDevice::open_at(&addr, &listing(), quick(), quick()).unwrap();
        dev.watch_period = Duration::from_millis(30);
        let (ctx, _net) = streaming_ctx();
        dev.start_rx(Arc::clone(&ctx)).unwrap();
        dev.set_frequency(2_410_000_000).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(dev.take_outside_changes(), [], "sdrtop's own retune");
        {
            let mut r = radio.lock().unwrap();
            r.attrs.insert(
                "ad9361-phy OUTPUT altvoltage0 frequency".into(),
                "145500000".into(),
            );
            r.attrs.insert(
                "cf-ad9361-dds-core-lpc OUTPUT altvoltage0 scale".into(),
                "0.099976".into(),
            );
            // Its LO running: the tone is on the air.
            r.attrs
                .insert("ad9361-phy OUTPUT altvoltage1 powerdown".into(), "0".into());
        }
        let started = std::time::Instant::now();
        let mut found = Vec::new();
        while found.len() < 2 && started.elapsed() < WAIT {
            std::thread::sleep(Duration::from_millis(30));
            found.extend(dev.take_outside_changes());
        }
        // One reading may straddle the two changes, so either order.
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found.contains(&OutsideChange::Frequency(145_500_000)),
            "{found:?}"
        );
        assert!(
            found.contains(&OutsideChange::Transmitting(Some("1 DDS tone set".into()))),
            "{found:?}"
        );
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(dev.take_outside_changes(), [], "said once");
        dev.stop_rx().unwrap();
        assert_eq!(Arc::strong_count(&ctx), 1);
    }

    #[test]
    fn the_r2_describes_itself_as_measured() {
        let c = caps();
        assert_eq!((c.freq_min_hz, c.freq_max_hz), (70_000_000, 6_000_000_000));
        assert_eq!(c.sample_geometry.full_scale, 2048.0);
        assert_eq!(c.sample_geometry.converter_bits, Some(12));
        assert_eq!(c.delivery, DeliveryModel::Pull);
        assert_eq!(c.sample_rates, RATES_HZ.to_vec());
        c.validate_display().unwrap();
    }
}
