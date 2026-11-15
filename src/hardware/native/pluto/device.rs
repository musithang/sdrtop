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
//! **Not streaming yet.** It opens and describes itself; samples still come
//! through SoapySDR.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::hardware::traits::RateSet;
use crate::hardware::{
    AcquisitionKind, DeliveryModel, DeviceCapabilities, DeviceInfo, DeviceListing, GainModel,
    LevelUnit, RxContext, SampleFormat, SampleGeometry, SdrDevice, SoftwareStack, StageSpec,
    IQ_TRACE_STALE_MS,
};

use super::iiod::{Client, Target, Timeouts};
use super::tx::{self, Finding};
use super::{context, identify, Model, DEFAULT_ADDRESS};

const PHY: &str = "ad9361-phy";

/// The receiver's LO range, as the radio reports it (`[70e6 1 6e9]`).
pub const FREQ_MIN_HZ: u64 = 70_000_000;
pub const FREQ_MAX_HZ: u64 = 6_000_000_000;

/// The rates offered: those measured to stream without loss over the radio's
/// USB network on the bench machine (libiio's own reader, 4 s each), from the
/// floor the radio accepts. More waits for Ethernet.
pub const RATES_HZ: [f64; 4] = [2_083_333.0, 3_000_000.0, 4_000_000.0, 5_000_000.0];

/// The radio's published floor, `2083333`, is refused when written; the next
/// integer is taken and reads back as the floor (25 MHz / 12 rounds down).
const RATE_FLOOR_WRITE: u64 = 2_083_334;

/// The analog filter's span, `rf_bandwidth_available`.
const BW_MIN_HZ: f64 = 200_000.0;
const BW_MAX_HZ: f64 = 56_000_000.0;

/// The whole span `hardwaregain` takes across the three gain-table bands
/// (-1..73 below 1.3 GHz, -3..71 to 4 GHz, -10..62 above, read from the
/// radio). The band at hand clamps a value outside its own range, and the
/// clamp is read back.
const GAIN_MIN_DB: f64 = -10.0;
const GAIN_MAX_DB: f64 = 73.0;

/// Longer than discovery's: the RF DC calibration answers in about 175 ms on
/// its own and took 1.7 s while another client streamed.
const SESSION: Timeouts = Timeouts {
    connect: std::time::Duration::from_millis(1000),
    io: std::time::Duration::from_millis(5000),
};

/// A PlutoSky R2, opened.
pub struct PlutoDevice {
    client: Mutex<Client>,
    caps: DeviceCapabilities,
    info: DeviceInfo,
    notes: Vec<String>,
    moved: Mutex<Option<(usize, f64)>>,
}

impl PlutoDevice {
    /// The R2 a listing names, at the default address.
    pub fn open(listing: &DeviceListing) -> anyhow::Result<Self> {
        Self::open_at(DEFAULT_ADDRESS, listing, SESSION)
    }

    pub(super) fn open_at(
        address: &str,
        listing: &DeviceListing,
        t: Timeouts,
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
        Ok(Self {
            client: Mutex::new(c),
            caps: caps(),
            info,
            notes,
            moved: Mutex::new(None),
        })
    }

    fn client(&self) -> MutexGuard<'_, Client> {
        self.client.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn number(c: &mut Client, target: Target<'_>, attr: &str) -> anyhow::Result<f64> {
        let v = c.read(target, attr)?;
        tx::parse_db(&v).ok_or_else(|| anyhow::anyhow!("{target} {attr} reads {v:?}"))
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
        gain: GainModel::new(
            vec![StageSpec::ranged(
                "hardwaregain",
                GAIN_MIN_DB,
                GAIN_MAX_DB,
                1.0,
            )],
            "Gain",
            "GAIN",
        ),
        samples_per_transfer: 0,
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

    fn start_rx(&self, _ctx: Arc<RxContext>) -> anyhow::Result<()> {
        anyhow::bail!(
            "sdrtop's own PlutoSky backend opens and describes the R2 but cannot stream from it \
             yet. For samples, open it with --device soapy=driver=plutosdr"
        )
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        false
    }

    fn set_frequency(&self, hz: u64) -> anyhow::Result<u64> {
        let mut c = self.client();
        let lo = Target::Output(PHY, "altvoltage0");
        c.write(lo, "frequency", &hz.to_string())?;
        Ok(Self::number(&mut c, lo, "frequency")? as u64)
    }

    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        let mut c = self.client();
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
        Ok(RateSet::new(hz, Some(rate), bw as u32))
    }

    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()> {
        self.set_stage_gain(0, "hardwaregain", f64::from(db))
    }

    fn set_stage_gain(&self, index: usize, _name: &str, db: f64) -> anyhow::Result<()> {
        if index != 0 {
            return Ok(());
        }
        let mut c = self.client();
        let rx = Target::Input(PHY, "voltage0");
        let want = db.round();
        c.write(rx, "hardwaregain", &format!("{want}"))?;
        let got = Self::number(&mut c, rx, "hardwaregain")?;
        if (got - want).abs() > 0.01 {
            *self.moved.lock().unwrap_or_else(|e| e.into_inner()) = Some((0, got));
        }
        Ok(())
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
        put("ad9361-phy INPUT voltage0 rf_port_select", "A_BALANCED");
        put("ad9361-phy INPUT voltage0 sampling_frequency", "30720000");
        put("ad9361-phy INPUT voltage0 rf_bandwidth", "18000000");
        put("ad9361-phy OUTPUT altvoltage0 frequency", "2400000000");
        put("ad9361-phy calib_mode", "auto");
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
        let dev = PlutoDevice::open_at(&addr, &listing(), quick()).unwrap();
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
        let err = PlutoDevice::open_at(&addr, &listing(), quick())
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
        let dev = PlutoDevice::open_at(&addr, &listing(), quick()).unwrap();
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
        let dev = PlutoDevice::open_at(&addr, &listing(), quick()).unwrap();
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
        let dev = PlutoDevice::open_at(&addr, &listing(), quick()).unwrap();
        assert!(!written(&radio).iter().any(|w| w.contains("OUTPUT")));
        assert!(dev.open_notes()[0].contains("already silent"));
    }

    #[test]
    fn a_receiver_stuck_in_its_agc_refuses_the_open() {
        let mut r = r2_after_power_up();
        r.ignores
            .insert("ad9361-phy INPUT voltage0 gain_control_mode".into());
        let (addr, _) = fake::serve(r);
        let err = PlutoDevice::open_at(&addr, &listing(), quick())
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
        assert!(PlutoDevice::open_at(&addr, &l, quick()).is_err());
        assert!(written(&radio).is_empty(), "nothing written to a stranger");
    }

    /// The radio's readback is the setting: the floor written one higher,
    /// the filter following the rate, a gain the band clamps told once.
    #[test]
    fn settings_are_what_the_radio_reads_back() {
        let (addr, radio) = fake::serve(r2_after_power_up());
        let dev = PlutoDevice::open_at(&addr, &listing(), quick()).unwrap();
        let set = dev.set_sample_rate(2_083_333.0).unwrap();
        assert_eq!(
            set.rate_hz, 2_083_334.0,
            "the stand-in keeps what it is given"
        );
        assert_eq!(set.bb_filter_hz, 2_083_334);
        assert!(written(&radio)
            .contains(&"ad9361-phy INPUT voltage0 sampling_frequency=2083334".to_string()));
        assert_eq!(dev.set_frequency(433_500_000).unwrap(), 433_500_000);

        radio
            .lock()
            .unwrap()
            .ignores
            .insert("ad9361-phy INPUT voltage0 hardwaregain".into());
        dev.set_stage_gain(0, "hardwaregain", 20.0).unwrap();
        assert_eq!(dev.take_moved_stage(), Some((0, 71.0)), "the radio's value");
        assert_eq!(dev.take_moved_stage(), None, "told once");
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
