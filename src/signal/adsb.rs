// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Mode S / ADS-B reception from RTL-SDR-rate IQ samples.
//!
//! `rs1090` owns the 2.4 MS/s PPM demodulator, Mode S parity validation and
//! message parser. This module adapts sdrtop's complex-f32 stream to that API
//! and keeps enough trailing samples to carry frames across input blocks.

use num_complex::Complex;
use rs1090::decode::adsb::ME;
use rs1090::decode::bds::bds09::AirborneVelocitySubType;
use rs1090::decode::cpr::{airborne_position, Position};
use rs1090::decode::DF;
use rs1090::source::demod::demod2400::demodulate2400;
use rs1090::source::demod::demod6000::demodulate6000;
use rs1090::source::demod::{
    convert_f32_to_i16_iq, magnitude_u16, ModeSMessage, MODES_LONG_MSG_BYTES,
};

pub mod gate;
pub mod worker;

/// The input rate used by `rs1090`'s RTL-SDR demodulator.
pub const RTL_SAMPLE_RATE_HZ: f64 = 2_400_000.0;
/// The input rate used by `rs1090`'s HackRF-capable demodulator.
pub const HIGH_RATE_SAMPLE_RATE_HZ: f64 = 6_000_000.0;
pub const CENTER_FREQUENCY_HZ: u64 = 1_090_000_000;
pub const SECTION: &str = "adsb";

/// The 2.4-MS/s demodulator requires this many trailing samples.
const DEMOD_2400_TRAILING_SAMPLES: usize = 326;
/// At 6 MS/s a long frame is 48 preamble + 672 data samples; keep extra margin.
const DEMOD_6000_TRAILING_SAMPLES: usize = 800;
/// Track only the near-DC carrier; ADS-B's 1 µs pulse structure is far above it.
const DC_TRACK_ALPHA: f32 = 0.0002;

/// A validated Mode S frame and the ADS-B identity fields commonly shown first.
#[derive(Clone, Debug, PartialEq)]
pub struct AdsbFrame {
    pub bytes: [u8; MODES_LONG_MSG_BYTES],
    pub downlink_format: u8,
    pub icao_address: Option<u32>,
    pub signal_level: f64,
    /// The decoded payload, kept so the worker can read fields the summary
    /// string flattens away (callsign, altitude, velocity, CPR position).
    pub message: Option<Box<DF>>,
    /// The fields worth a column of their own, read once at decode time.
    pub details: AdsbDetails,
}

/// The fields a Mode S frame carries that are worth a column of their own.
///
/// Every one is `Option` because a frame only carries what its type code says
/// it carries: a position frame has no callsign, an identity frame has no
/// altitude. `None` is printed as an absence, never as a plausible default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdsbDetails {
    pub callsign: Option<String>,
    pub category: Option<String>,
    pub altitude_ft: Option<i32>,
    pub altitude_source: Option<String>,
    pub groundspeed_kt: Option<f64>,
    pub track_deg: Option<f64>,
    pub vertical_rate_fpm: Option<i16>,
    pub squawk: Option<String>,
    pub emergency: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl AdsbDetails {
    /// Read every field this frame actually carries. A frame that carries none
    /// of them yields an all-`None` value, which the panel renders as blanks.
    pub fn from_message(message: &DF) -> Self {
        let mut details = Self::default();
        let DF::ExtendedSquitterADSB(adsb) = message else {
            return details;
        };
        match &adsb.message {
            ME::BDS08 { inner, .. } => {
                let callsign = inner.callsign.trim();
                if !callsign.is_empty() {
                    details.callsign = Some(callsign.to_string());
                }
                details.category = Some(inner.wake_vortex.to_string());
            }
            ME::BDS05 { inner, .. } => {
                details.altitude_ft = inner.alt;
                details.altitude_source = Some(inner.source.to_string());
            }
            ME::BDS09(velocity) => {
                details.vertical_rate_fpm = velocity.vertical_rate;
                match &velocity.velocity {
                    AirborneVelocitySubType::GroundSpeedDecoding(ground) => {
                        details.groundspeed_kt = Some(ground.groundspeed);
                        details.track_deg = Some(ground.track);
                    }
                    AirborneVelocitySubType::AirspeedSubsonic(airspeed) => {
                        details.groundspeed_kt = airspeed.airspeed.map(f64::from);
                    }
                    AirborneVelocitySubType::AirspeedSupersonic(airspeed) => {
                        details.groundspeed_kt = airspeed.airspeed.map(f64::from);
                    }
                    _ => {}
                }
            }
            ME::BDS61(status) => {
                details.squawk = Some(format!("{:?}", status.squawk));
                details.emergency = Some(status.emergency_state.to_string());
            }
            _ => {}
        }
        details
    }
}

/// The last even and odd airborne position a single aircraft sent, so a CPR
/// pair can be resolved into a latitude and longitude.
#[derive(Clone, Copy, Debug, Default)]
struct CprPair {
    even: Option<rs1090::decode::bds::bds05::AirbornePosition>,
    odd: Option<rs1090::decode::bds::bds05::AirbornePosition>,
}

/// Per-aircraft CPR memory, keyed by ICAO address.
///
/// CPR encodes a position in two halves that only mean something together, so
/// the receiver has to remember the previous half per aircraft. This is the
/// smallest state that can do that and nothing else.
#[derive(Default)]
struct CprTracker {
    aircraft: std::collections::HashMap<u32, CprPair>,
}

impl CprTracker {
    fn clear(&mut self) {
        self.aircraft.clear();
    }

    /// Record one airborne position frame and return a position if this frame
    /// completed a pair. The pair is consumed so a stale half is never reused.
    fn observe(
        &mut self,
        icao: u32,
        position: &rs1090::decode::bds::bds05::AirbornePosition,
    ) -> Option<Position> {
        let pair = self.aircraft.entry(icao).or_default();
        match position.parity {
            rs1090::decode::cpr::CPRFormat::Even => pair.even = Some(*position),
            rs1090::decode::cpr::CPRFormat::Odd => pair.odd = Some(*position),
        }
        let (even, odd) = (pair.even?, pair.odd?);
        let resolved = airborne_position(&even, &odd);
        if resolved.is_some() {
            *pair = CprPair::default();
        }
        resolved
    }
}

/// Stateful 2.4 or 6 MS/s receiver. It retains each demodulator's trailing
/// window and only returns frames whose start position has not been scanned.
#[derive(Default)]
pub struct AdsbReceiver {
    magnitude_tail: Vec<u16>,
    iq_tail: Vec<i16>,
    total_samples: u64,
    scanned_until: u64,
    mode: Option<DemodMode>,
    dc_i: f32,
    dc_q: f32,
    dc_initialized: bool,
    cpr: CprTracker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DemodMode {
    Rtl2400,
    HighRate6000,
}

pub fn supports_sample_rate(sample_rate_hz: f64) -> bool {
    (sample_rate_hz - RTL_SAMPLE_RATE_HZ).abs() <= 1.0
        || (sample_rate_hz - HIGH_RATE_SAMPLE_RATE_HZ).abs() <= 1.0
}

impl AdsbReceiver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.magnitude_tail.clear();
        self.iq_tail.clear();
        self.total_samples = 0;
        self.scanned_until = 0;
        self.mode = None;
        self.dc_i = 0.0;
        self.dc_q = 0.0;
        self.dc_initialized = false;
        self.cpr.clear();
    }

    /// Feed one contiguous 2.4 or 6 MS/s block. `None` means the rate is
    /// unsupported; an empty `Some` means no valid frame was found in it.
    pub fn push_iq(
        &mut self,
        samples: &mut [Complex<f32>],
        sample_rate_hz: f64,
    ) -> Option<Vec<AdsbFrame>> {
        let mode = if (sample_rate_hz - RTL_SAMPLE_RATE_HZ).abs() <= 1.0 {
            DemodMode::Rtl2400
        } else if (sample_rate_hz - HIGH_RATE_SAMPLE_RATE_HZ).abs() <= 1.0 {
            DemodMode::HighRate6000
        } else {
            return None;
        };
        if samples.is_empty() {
            return Some(Vec::new());
        }

        if self.mode.is_some_and(|previous| previous != mode) {
            self.reset();
        }
        self.mode = Some(mode);
        self.remove_dc(samples);

        let (messages, base, scanned_end) = match mode {
            DemodMode::Rtl2400 => self.demodulate_2400(samples),
            DemodMode::HighRate6000 => self.demodulate_6000(samples),
        };
        let first_new = self.scanned_until.max(base);

        let frames = messages
            .into_iter()
            .filter_map(|message| {
                let absolute_start = base + message.sample_position as u64;
                (absolute_start >= first_new && absolute_start < scanned_end)
                    .then(|| self.decode_frame(message))
                    .flatten()
            })
            .collect();

        self.scanned_until = self.scanned_until.max(scanned_end);
        self.total_samples += samples.len() as u64;

        Some(frames)
    }

    fn remove_dc(&mut self, samples: &mut [Complex<f32>]) {
        if !self.dc_initialized {
            self.dc_i = samples[0].re;
            self.dc_q = samples[0].im;
            self.dc_initialized = true;
        }

        for sample in samples {
            let raw_i = sample.re;
            let raw_q = sample.im;
            sample.re = raw_i - self.dc_i;
            sample.im = raw_q - self.dc_q;
            self.dc_i += DC_TRACK_ALPHA * (raw_i - self.dc_i);
            self.dc_q += DC_TRACK_ALPHA * (raw_q - self.dc_q);
        }
    }

    fn demodulate_2400(&mut self, samples: &[Complex<f32>]) -> (Vec<ModeSMessage>, u64, u64) {
        let mut magnitudes = self.magnitude_tail.clone();
        magnitudes.extend(magnitude_u16(samples));
        let base = self.total_samples - self.magnitude_tail.len() as u64;
        let scanned_end =
            base + magnitudes.len().saturating_sub(DEMOD_2400_TRAILING_SAMPLES) as u64;
        let messages = demodulate2400(&magnitudes);
        let keep = magnitudes.len().min(DEMOD_2400_TRAILING_SAMPLES);
        self.magnitude_tail.clear();
        self.magnitude_tail
            .extend_from_slice(&magnitudes[magnitudes.len() - keep..]);
        (messages, base, scanned_end)
    }

    fn demodulate_6000(&mut self, samples: &[Complex<f32>]) -> (Vec<ModeSMessage>, u64, u64) {
        let mut iq = self.iq_tail.clone();
        iq.extend(convert_f32_to_i16_iq(samples));
        let tail_samples = (self.iq_tail.len() / 2) as u64;
        let total_samples = (iq.len() / 2) as u64;
        let base = self.total_samples - tail_samples;
        let trailing = DEMOD_6000_TRAILING_SAMPLES as u64;
        let scanned_end = base + total_samples.saturating_sub(trailing);
        let messages = demodulate6000(&iq);
        let keep_samples = total_samples.min(trailing) as usize;
        self.iq_tail.clear();
        self.iq_tail
            .extend_from_slice(&iq[(total_samples as usize - keep_samples) * 2..]);
        (messages, base, scanned_end)
    }

    fn decode_frame(&mut self, message: ModeSMessage) -> Option<AdsbFrame> {
        let downlink_format = message.msg[0] >> 3;
        let frame_len = if downlink_format & 0x10 == 0 { 7 } else { 14 };
        let decoded = rs1090::decode::Message::try_from(&message.msg[..frame_len]).ok()?;
        let icao_address = match downlink_format {
            17 | 18 => Some(
                (u32::from(message.msg[1]) << 16)
                    | (u32::from(message.msg[2]) << 8)
                    | u32::from(message.msg[3]),
            ),
            _ => None,
        };

        let mut details = AdsbDetails::from_message(&decoded.df);
        if let (Some(icao), DF::ExtendedSquitterADSB(adsb)) = (icao_address, &decoded.df) {
            if let ME::BDS05 { inner, .. } = &adsb.message {
                if let Some(position) = self.cpr.observe(icao, inner) {
                    details.latitude = Some(position.latitude);
                    details.longitude = Some(position.longitude);
                }
            }
        }

        Some(AdsbFrame {
            bytes: message.msg,
            downlink_format,
            icao_address,
            signal_level: message.signal_level,
            message: Some(Box::new(decoded.df)),
            details,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE_FRAME: [u8; 14] = [
        0x8d, 0x4b, 0xb4, 0x63, 0x00, 0x3d, 0x10, 0x00, 0x00, 0x00, 0x00, 0x1b, 0x5b, 0xec,
    ];

    fn modulate_reference_frame(sample_rate_hz: f64) -> Vec<Complex<f32>> {
        let samples_per_us = sample_rate_hz / 1e6;
        let count = (300.0 * samples_per_us) as usize;
        let mut samples = vec![Complex::new(0.02, 0.0); count];
        for (index, sample) in samples.iter_mut().enumerate() {
            let time_us = index as f64 / samples_per_us - (1.0 / samples_per_us);
            let preamble = [0.0, 1.0, 3.5, 4.5]
                .iter()
                .any(|&start| (start..start + 0.5).contains(&time_us));
            let data_time = time_us - 8.0;
            let data_bit = if data_time >= 0.0 {
                let bit_index = data_time.floor() as usize;
                (bit_index < REFERENCE_FRAME.len() * 8).then(|| {
                    let byte = REFERENCE_FRAME[bit_index / 8];
                    let one = byte & (1 << (7 - bit_index % 8)) != 0;
                    let phase = data_time - bit_index as f64;
                    if one {
                        phase < 0.5
                    } else {
                        (0.5..1.0).contains(&phase)
                    }
                })
            } else {
                None
            };
            if preamble || data_bit == Some(true) {
                *sample = Complex::new(0.7, 0.0);
            }
        }
        samples
    }

    #[test]
    fn decodes_a_known_adsb_frame_across_iq_block_boundaries_at_both_rates() {
        for rate in [RTL_SAMPLE_RATE_HZ, HIGH_RATE_SAMPLE_RATE_HZ] {
            let mut iq = modulate_reference_frame(rate);
            let mut receiver = AdsbReceiver::new();
            let mut frames = Vec::new();
            for block in iq.chunks_mut(137) {
                frames.extend(receiver.push_iq(block, rate).expect("supported rate"));
            }

            let frame = frames
                .iter()
                .find(|frame| frame.downlink_format == 17)
                .unwrap_or_else(|| panic!("no DF17 frame at {rate} samples/s"));
            assert_eq!(frame.icao_address, Some(0x4b_b463));
            assert_eq!(&frame.bytes[..REFERENCE_FRAME.len()], &REFERENCE_FRAME);
            assert!(frame.message.is_some());
        }
    }

    #[test]
    fn rejects_rates_not_supported_by_the_rtl_demodulator() {
        let mut receiver = AdsbReceiver::new();
        assert!(receiver.push_iq(&mut [], 2_000_000.0).is_none());
        assert_eq!(
            receiver.push_iq(&mut [], RTL_SAMPLE_RATE_HZ),
            Some(Vec::new())
        );
        assert_eq!(
            receiver.push_iq(&mut [], HIGH_RATE_SAMPLE_RATE_HZ),
            Some(Vec::new())
        );
    }

    #[test]
    fn removes_a_stationary_lo_leak_before_ppm_detection() {
        let mut iq = modulate_reference_frame(HIGH_RATE_SAMPLE_RATE_HZ);
        for sample in &mut iq {
            *sample += Complex::new(0.2, 0.1);
        }

        let mut receiver = AdsbReceiver::new();
        let mut frames = Vec::new();
        for block in iq.chunks_mut(511) {
            frames.extend(
                receiver
                    .push_iq(block, HIGH_RATE_SAMPLE_RATE_HZ)
                    .expect("supported HackRF rate"),
            );
        }

        assert!(
            frames
                .iter()
                .any(|frame| frame.icao_address == Some(0x4b_b463)),
            "stationary center-frequency leakage hid the known frame"
        );
    }

    /// The two halves of a CPR pair, from `rs1090`'s own reference vectors.
    /// Neither frame alone carries a position; only the pair does.
    const CPR_EVEN: [u8; 14] = [
        0x8d, 0x40, 0x05, 0x8b, 0x58, 0xc9, 0x01, 0x37, 0x51, 0x47, 0xef, 0xd0, 0x93, 0x57,
    ];
    const CPR_ODD: [u8; 14] = [
        0x8d, 0x40, 0x05, 0x8b, 0x58, 0xc9, 0x04, 0xa8, 0x7f, 0x40, 0x2d, 0x3b, 0x8c, 0x59,
    ];

    fn modulate_frame(frame: &[u8; 14], sample_rate_hz: f64) -> Vec<Complex<f32>> {
        let samples_per_us = sample_rate_hz / 1e6;
        let count = (300.0 * samples_per_us) as usize;
        let mut samples = vec![Complex::new(0.02, 0.0); count];
        for (index, sample) in samples.iter_mut().enumerate() {
            let time_us = index as f64 / samples_per_us - (1.0 / samples_per_us);
            let preamble = [0.0, 1.0, 3.5, 4.5]
                .iter()
                .any(|&start| (start..start + 0.5).contains(&time_us));
            let data_time = time_us - 8.0;
            let data_bit = if data_time >= 0.0 {
                let bit_index = data_time.floor() as usize;
                (bit_index < frame.len() * 8).then(|| {
                    let byte = frame[bit_index / 8];
                    let one = byte & (1 << (7 - bit_index % 8)) != 0;
                    let phase = data_time - bit_index as f64;
                    if one {
                        phase < 0.5
                    } else {
                        (0.5..1.0).contains(&phase)
                    }
                })
            } else {
                None
            };
            if preamble || data_bit == Some(true) {
                *sample = Complex::new(0.7, 0.0);
            }
        }
        samples
    }

    #[test]
    fn resolves_a_cpr_pair_into_a_position_across_two_frames() {
        let mut receiver = AdsbReceiver::new();
        let mut frames = Vec::new();
        for frame in [CPR_EVEN, CPR_ODD] {
            let mut iq = modulate_frame(&frame, RTL_SAMPLE_RATE_HZ);
            for block in iq.chunks_mut(137) {
                frames.extend(
                    receiver
                        .push_iq(block, RTL_SAMPLE_RATE_HZ)
                        .expect("supported rate"),
                );
            }
        }

        let positioned = frames
            .iter()
            .find(|frame| frame.details.latitude.is_some())
            .expect("the odd frame should complete the pair");
        let latitude = positioned.details.latitude.unwrap();
        let longitude = positioned.details.longitude.unwrap();
        assert!(
            (latitude - 49.81755).abs() < 0.01,
            "latitude {latitude} is not near the reference"
        );
        assert!(
            (longitude - 6.08442).abs() < 0.01,
            "longitude {longitude} is not near the reference"
        );
    }

    #[test]
    fn a_lone_position_frame_reports_no_position() {
        let mut receiver = AdsbReceiver::new();
        let mut iq = modulate_frame(&CPR_EVEN, RTL_SAMPLE_RATE_HZ);
        let mut frames = Vec::new();
        for block in iq.chunks_mut(137) {
            frames.extend(
                receiver
                    .push_iq(block, RTL_SAMPLE_RATE_HZ)
                    .expect("supported rate"),
            );
        }

        assert!(
            frames.iter().all(|frame| frame.details.latitude.is_none()),
            "half a CPR pair must not be reported as a position"
        );
    }
}
