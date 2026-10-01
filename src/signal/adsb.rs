// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Mode S / ADS-B reception from RTL-SDR-rate IQ samples.
//!
//! `rs1090` owns the 2.4 MS/s PPM demodulator, Mode S parity validation and
//! message parser. This module adapts sdrtop's complex-f32 stream to that API
//! and keeps enough trailing samples to carry frames across input blocks.

use num_complex::Complex;
use rs1090::source::demod::demod2400::demodulate2400;
use rs1090::source::demod::{magnitude_u16, ModeSMessage, MODES_LONG_MSG_BYTES};

pub mod gate;
pub mod worker;

/// The input rate used by `rs1090`'s RTL-SDR demodulator.
pub const RTL_SAMPLE_RATE_HZ: f64 = 2_400_000.0;
pub const CENTER_FREQUENCY_HZ: u64 = 1_090_000_000;
pub const SECTION: &str = "adsb";

/// The `rs1090` demodulator requires this many samples after a preamble to
/// decode the longest Mode S frame. Retaining them lets a frame cross blocks.
const DEMOD_TRAILING_SAMPLES: usize = 326;

/// A validated Mode S frame and the ADS-B identity fields commonly shown first.
#[derive(Clone, Debug, PartialEq)]
pub struct AdsbFrame {
    pub bytes: [u8; MODES_LONG_MSG_BYTES],
    pub downlink_format: u8,
    pub icao_address: Option<u32>,
    pub signal_level: f64,
    pub summary: String,
}

/// Stateful 2.4 MS/s receiver. It retains the demodulator's trailing window and
/// only returns frames whose start position has not been scanned before.
#[derive(Default)]
pub struct AdsbReceiver {
    tail: Vec<u16>,
    total_samples: u64,
    scanned_until: u64,
}

impl AdsbReceiver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.tail.clear();
        self.total_samples = 0;
        self.scanned_until = 0;
    }

    /// Feed one contiguous 2.4 MS/s block. `None` means the block has the wrong
    /// sample rate; an empty `Some` means no valid frame was found in it.
    pub fn push_iq(
        &mut self,
        samples: &[Complex<f32>],
        sample_rate_hz: f64,
    ) -> Option<Vec<AdsbFrame>> {
        if (sample_rate_hz - RTL_SAMPLE_RATE_HZ).abs() > 1.0 {
            return None;
        }
        if samples.is_empty() {
            return Some(Vec::new());
        }

        let mut magnitudes = self.tail.clone();
        magnitudes.extend(magnitude_u16(samples));
        let base = self.total_samples - self.tail.len() as u64;
        let scanned_end = base + magnitudes.len().saturating_sub(DEMOD_TRAILING_SAMPLES) as u64;
        let first_new = self.scanned_until.max(base);

        let frames = demodulate2400(&magnitudes)
            .into_iter()
            .filter_map(|message| {
                let absolute_start = base + message.sample_position as u64;
                (absolute_start >= first_new && absolute_start < scanned_end)
                    .then(|| decode_frame(message))
                    .flatten()
            })
            .collect();

        self.scanned_until = self.scanned_until.max(scanned_end);
        self.total_samples += samples.len() as u64;
        let keep = magnitudes.len().min(DEMOD_TRAILING_SAMPLES);
        self.tail.clear();
        self.tail
            .extend_from_slice(&magnitudes[magnitudes.len() - keep..]);

        Some(frames)
    }
}

fn decode_frame(message: ModeSMessage) -> Option<AdsbFrame> {
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

    Some(AdsbFrame {
        bytes: message.msg,
        downlink_format,
        icao_address,
        signal_level: message.signal_level,
        summary: decoded.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE_FRAME: [u8; 14] = [
        0x8d, 0x4b, 0xb4, 0x63, 0x00, 0x3d, 0x10, 0x00, 0x00, 0x00, 0x00, 0x1b, 0x5b, 0xec,
    ];

    fn modulate_reference_frame() -> Vec<Complex<f32>> {
        let mut samples = vec![Complex::new(0.02, 0.0); 2_400];
        for (index, sample) in samples.iter_mut().enumerate() {
            let time_us = index as f64 / 2.4 - (1.0 / 2.4);
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
                *sample = Complex::new(0.9, 0.0);
            }
        }
        samples
    }

    #[test]
    fn decodes_a_known_adsb_frame_across_iq_block_boundaries() {
        let iq = modulate_reference_frame();
        let mut receiver = AdsbReceiver::new();
        let mut frames = Vec::new();
        for block in iq.chunks(137) {
            frames.extend(
                receiver
                    .push_iq(block, RTL_SAMPLE_RATE_HZ)
                    .expect("supported rate"),
            );
        }

        let frame = frames
            .iter()
            .find(|frame| frame.downlink_format == 17)
            .expect("reference DF17 frame");
        assert_eq!(frame.icao_address, Some(0x4b_b463));
        assert_eq!(&frame.bytes[..REFERENCE_FRAME.len()], &REFERENCE_FRAME);
        assert!(frame.summary.contains("Extended Squitter"));
    }

    #[test]
    fn rejects_rates_not_supported_by_the_rtl_demodulator() {
        let mut receiver = AdsbReceiver::new();
        assert!(receiver.push_iq(&[], 2_000_000.0).is_none());
        assert_eq!(receiver.push_iq(&[], RTL_SAMPLE_RATE_HZ), Some(Vec::new()));
    }
}
