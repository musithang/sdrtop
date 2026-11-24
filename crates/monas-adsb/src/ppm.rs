// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

const PREAMBLE_HALF_CELLS: usize = 16;
const FRAME_BITS: usize = 112;

/// Half-cell integrals in input-power times sample periods.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BitEvidence {
    pub early_energy: f64,
    pub late_energy: f64,
}

impl BitEvidence {
    /// Signed contrast in [-1, 1]. Zero represents a tie or no energy.
    pub fn margin(self) -> f64 {
        let total = self.early_energy + self.late_energy;
        if total > 0.0 {
            (self.early_energy - self.late_energy) / total
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PpmError {
    InvalidSampleRate,
    InvalidStart,
    IncompleteFrame,
    InvalidPower { index: usize },
}

/// Mode S pulse-position slicer for a candidate with known preamble timing.
#[derive(Clone, Copy, Debug)]
pub struct PpmSlicer {
    half_cell_samples: f64,
}

impl PpmSlicer {
    /// Initialize from the actual IQ pair rate, in pairs per second.
    /// This integrator requires at least one sample period per half-cell.
    pub fn new(sample_rate_hz: f64) -> Result<Self, PpmError> {
        if !sample_rate_hz.is_finite() || sample_rate_hz < 2_000_000.0 {
            return Err(PpmError::InvalidSampleRate);
        }
        Ok(Self {
            half_cell_samples: sample_rate_hz / 2_000_000.0,
        })
    }

    /// Pack a 112-bit candidate MSB-first, starting 8 µs after the given edge.
    ///
    /// The input is power (I² + Q²). Slice-local sample n represents a
    /// constant-power cell [n, n+1), centered at n+0.5. Fractional windows
    /// use the overlapping portion of each cell. An exact tie packs a zero.
    /// Returned bytes still need a downlink-format and CRC check.
    pub fn slice_frame(
        &self,
        power: &[f32],
        preamble_start: f64,
        trace: Option<&mut [BitEvidence; FRAME_BITS]>,
    ) -> Result<[u8; 14], PpmError> {
        if !preamble_start.is_finite() || preamble_start < 0.0 {
            return Err(PpmError::InvalidStart);
        }
        let boundary = |half_cell| preamble_start + half_cell as f64 * self.half_cell_samples;
        let payload_end = boundary(PREAMBLE_HALF_CELLS + 2 * FRAME_BITS);
        if payload_end > power.len() as f64 {
            return Err(PpmError::IncompleteFrame);
        }
        let mut bytes = [0; 14];
        let mut evidence = trace
            .is_some()
            .then(|| [BitEvidence::default(); FRAME_BITS]);
        for bit in 0..FRAME_BITS {
            let first_half = PREAMBLE_HALF_CELLS + 2 * bit;
            let start = boundary(first_half);
            let middle = boundary(first_half + 1);
            let end = boundary(first_half + 2);
            let early_energy = integrate(power, start, middle)?;
            let late_energy = integrate(power, middle, end)?;
            if early_energy > late_energy {
                bytes[bit / 8] |= 0x80 >> (bit % 8);
            }
            if let Some(evidence) = &mut evidence {
                evidence[bit] = BitEvidence {
                    early_energy,
                    late_energy,
                };
            }
        }
        if let (Some(trace), Some(evidence)) = (trace, evidence) {
            *trace = evidence;
        }
        Ok(bytes)
    }
}

fn integrate(power: &[f32], start: f64, end: f64) -> Result<f64, PpmError> {
    let first = start.floor() as usize;
    let last = end.ceil() as usize;
    let mut energy = 0.0;
    for (offset, &sample) in power[first..last].iter().enumerate() {
        let index = first + offset;
        if !sample.is_finite() || sample < 0.0 {
            return Err(PpmError::InvalidPower { index });
        }
        let left = index as f64;
        let overlap = end.min(left + 1.0) - start.max(left);
        energy += f64::from(sample) * overlap;
    }
    Ok(energy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Df17Frame;

    // PR #22, src/signal/adsb.rs, REFERENCE_FRAME and modulate_reference_frame.
    const ARCHIVED_FRAME: [u8; 14] = [
        0x8D, 0x4B, 0xB4, 0x63, 0x00, 0x3D, 0x10, 0x00, 0x00, 0x00, 0x00, 0x1B, 0x5B, 0xEC,
    ];
    // https://mode-s.org/1090mhz/content/ads-b/8-error-control.html
    const PUBLISHED_FRAME: [u8; 14] = [
        0x8D, 0x40, 0x6B, 0x90, 0x20, 0x15, 0xA6, 0x78, 0xD4, 0xD2, 0x20, 0xAA, 0x4B, 0xDA,
    ];

    /// Point-sample the rectangular pulses or average the complex voltage
    /// over a cell with 16 sub-samples. Convert those IQ values into power.
    fn sampled_power(bytes: &[u8; 14], rate: f64, start: f64, subsamples: usize) -> Vec<f32> {
        let samples_per_us = rate / 1_000_000.0;
        let len = (start + 120.0 * samples_per_us).ceil() as usize;
        (0..len)
            .map(|index| {
                let on = (0..subsamples)
                    .filter(|&sub| {
                        let fraction = (sub as f64 + 0.5) / subsamples as f64;
                        let time_us = (index as f64 + fraction - start) / samples_per_us;
                        pulse_at(bytes, time_us)
                    })
                    .count();
                let voltage = on as f32 / subsamples as f32;
                let (i, q) = (0.6 * voltage, 0.8 * voltage);
                i * i + q * q
            })
            .collect()
    }

    fn pulse_at(bytes: &[u8; 14], time_us: f64) -> bool {
        if time_us < 8.0 {
            return [0.0, 1.0, 3.5, 4.5]
                .iter()
                .any(|&start| (start..start + 0.5).contains(&time_us));
        }
        let data_us = time_us - 8.0;
        let bit = data_us.floor() as usize;
        if let Some(byte) = bytes.get(bit / 8) {
            let one = byte & (0x80 >> (bit % 8)) != 0;
            (data_us.fract() < 0.5) == one
        } else {
            false
        }
    }

    #[test]
    fn slices_both_reference_frames_across_fractional_phases_and_rates() {
        for rate in [2_400_000.0, 2_500_000.0, 6_000_000.0, 10_000_000.0] {
            let slicer = PpmSlicer::new(rate).unwrap();
            for expected in [ARCHIVED_FRAME, PUBLISHED_FRAME] {
                for phase in 0..64 {
                    let start = 5.0 + f64::from(phase) / 64.0;
                    for subsamples in [1, 16] {
                        let power = sampled_power(&expected, rate, start, subsamples);
                        let actual = slicer.slice_frame(&power, start, None).unwrap();
                        assert_eq!(actual, expected, "{rate} Hz, phase {phase}, {subsamples}x");
                        assert!(Df17Frame::try_from(actual.as_slice()).is_ok());
                    }
                }
            }
        }
    }

    #[test]
    fn reports_the_fractional_overlap_of_each_half_cell() {
        let slicer = PpmSlicer::new(2_500_000.0).unwrap();
        let mut power = [0.0; 301];
        power[20..23].copy_from_slice(&[2.0, 4.0, 8.0]);
        let mut trace = [BitEvidence::default(); FRAME_BITS];
        let bytes = slicer.slice_frame(&power, 0.25, Some(&mut trace)).unwrap();
        assert_eq!(trace[0].early_energy, 3.5);
        assert_eq!(trace[0].late_energy, 8.0);
        assert!((trace[0].margin() - (-4.5 / 11.5)).abs() < 1e-12);
        assert_eq!(bytes[0] & 0x80, 0);
        assert_eq!(trace[111].margin(), 0.0);
        assert!(matches!(
            slicer.slice_frame(&power[..300], 0.25, None),
            Err(PpmError::IncompleteFrame)
        ));
    }

    #[test]
    fn consumes_the_exact_length_at_an_integral_boundary() {
        let slicer = PpmSlicer::new(10_000_000.0).unwrap();
        let mut trace = [BitEvidence::default(); FRAME_BITS];
        assert_eq!(
            slicer.slice_frame(&[1.0; 1200], 0.0, Some(&mut trace)),
            Ok([0; 14])
        );
        assert!(trace
            .iter()
            .all(|bit| bit.early_energy == 5.0 && bit.late_energy == 5.0));
        assert_eq!(
            slicer.slice_frame(&[0.0; 1199], 0.0, None),
            Err(PpmError::IncompleteFrame)
        );
    }

    #[test]
    fn rejects_invalid_rate_start_and_used_power() {
        for rate in [f64::NAN, f64::INFINITY, -1.0, 1_999_999.0] {
            assert!(matches!(
                PpmSlicer::new(rate),
                Err(PpmError::InvalidSampleRate)
            ));
        }
        let slicer = PpmSlicer::new(2_500_000.0).unwrap();
        for start in [f64::NAN, f64::INFINITY, -0.25] {
            assert_eq!(
                slicer.slice_frame(&[], start, None),
                Err(PpmError::InvalidStart)
            );
        }
        let mut power = [0.0; 300];
        let marker = BitEvidence {
            early_energy: 5.0,
            late_energy: 0.0,
        };
        let mut trace = [marker; FRAME_BITS];
        for invalid in [f32::NAN, f32::INFINITY, -0.25] {
            power[299] = invalid;
            assert_eq!(
                slicer.slice_frame(&power, 0.0, Some(&mut trace)),
                Err(PpmError::InvalidPower { index: 299 })
            );
            assert_eq!(trace, [marker; FRAME_BITS]);
        }
    }
}
