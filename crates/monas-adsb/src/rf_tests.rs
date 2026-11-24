// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Reproducible RF stimuli for the envelope-input receiver.

use std::f64::consts::TAU;

use monas_dsp::fir::{design_lowpass, StreamingDecimator};
use monas_dsp::testkit::Rng;
use num_complex::Complex;

use crate::ppm::tests::{pulse_at, ARCHIVED_FRAME, PUBLISHED_FRAME};
use crate::{FrameOutcome, Receiver, StreamEvent};

const R2_RATES: [f64; 2] = [2_500_000.0, 10_000_000.0];
const OVERSAMPLE: usize = 8;
const FILTER_TAPS: usize = 129;
const BLOCK: usize = 511;

struct Burst {
    frame: [u8; 14],
    edge: f64,
    gain: f32,
    offset_hz: f64,
}

struct FrontEnd {
    cutoff_hz: f64,
    dc: Complex<f32>,
    noise_power: f64,
    clip: f32,
}

impl FrontEnd {
    fn at_rate(rate: f64) -> Self {
        Self {
            cutoff_hz: 0.44 * rate,
            dc: Complex::new(0.0, 0.0),
            noise_power: 0.0,
            clip: 1.0,
        }
    }

    fn capture(
        &self,
        rate: f64,
        pairs: usize,
        bursts: &[Burst],
        seed: u64,
    ) -> (Vec<Complex<f32>>, usize) {
        let taps = design_lowpass(FILTER_TAPS, self.cutoff_hz / (rate * OVERSAMPLE as f64));
        let mut filter = StreamingDecimator::new(taps, OVERSAMPLE);
        let delay = filter.delay();
        let source: Vec<_> = (0..((pairs - 1) * OVERSAMPLE + FILTER_TAPS))
            .map(|index| {
                // Center the FIR and the input sample cell on the same instant.
                let sample = (index as f64 - delay) / OVERSAMPLE as f64 + 0.5;
                bursts.iter().fold(Complex::new(0.0, 0.0), |sum, burst| {
                    let seconds = (sample - burst.edge) / rate;
                    if pulse_at(&burst.frame, seconds * 1e6) {
                        let angle = (TAU * burst.offset_hz * seconds) as f32;
                        sum + Complex::from_polar(burst.gain, angle)
                    } else {
                        sum
                    }
                })
            })
            .collect();
        let mut iq = Vec::new();
        filter.process(&source, &mut iq);
        assert_eq!(iq.len(), pairs);

        let noise = Rng::new(seed).noise(pairs, self.noise_power);
        let mut clipped = 0;
        for (sample, noise) in iq.iter_mut().zip(noise) {
            *sample += self.dc + noise;
            clipped += usize::from(sample.re.abs() > self.clip || sample.im.abs() > self.clip);
            sample.re = sample.re.clamp(-self.clip, self.clip);
            sample.im = sample.im.clamp(-self.clip, self.clip);
        }
        (iq, clipped)
    }
}

fn render_pairs(rate: f64, last_edge: f64) -> usize {
    (last_edge + 120e-6 * rate).ceil() as usize + 32
}

fn receive(iq: &[Complex<f32>], rate: f64, dc: Complex<f32>) -> Vec<StreamEvent> {
    let mut receiver = Receiver::new(rate).unwrap();
    let mut events = Vec::new();
    for (block, iq) in iq.chunks(BLOCK).enumerate() {
        let power: Vec<_> = iq.iter().map(|sample| (*sample - dc).norm_sqr()).collect();
        receiver
            .push_power((block * BLOCK) as u64, &power, |event| events.push(event))
            .unwrap();
    }
    receiver.finish(|event| events.push(event));
    events
}

fn valid_frames(events: &[StreamEvent]) -> impl Iterator<Item = (&[u8; 14], f64)> {
    events.iter().filter_map(|event| match &event.outcome {
        FrameOutcome::Accepted(frame) => Some((
            frame.bytes(),
            event.first_pair as f64 + event.preamble.start_sample,
        )),
        _ => None,
    })
}

#[test]
fn filtered_cfo_and_noise_sweep_on_r2_grids() {
    for rate in R2_RATES {
        for snr_db in [24, 18, 12, 6] {
            let mut hits = 0;
            let mut unexpected = 0;
            let mut detected = 0;
            let mut crc_rejected = 0;
            for phase in 0..64 {
                let burst = Burst {
                    frame: PUBLISHED_FRAME,
                    edge: 96.0 + f64::from(phase) / 64.0,
                    gain: 0.7,
                    offset_hz: 150_000.0,
                };
                let front_end = FrontEnd {
                    noise_power: f64::from(burst.gain).powi(2)
                        / 10.0_f64.powf(f64::from(snr_db) / 10.0),
                    ..FrontEnd::at_rate(rate)
                };
                let pairs = render_pairs(rate, burst.edge);
                let (iq, _) =
                    front_end.capture(rate, pairs, std::slice::from_ref(&burst), phase as u64);
                let events = receive(&iq, rate, Complex::ZERO);
                let near_start = |event: &StreamEvent| {
                    let edge = event.first_pair as f64 + event.preamble.start_sample;
                    (edge - burst.edge).abs() <= 2.0
                };
                detected += usize::from(events.iter().any(near_start));
                crc_rejected += usize::from(events.iter().any(|event| {
                    near_start(event) && matches!(event.outcome, FrameOutcome::CrcRejected { .. })
                }));
                for (frame, edge) in valid_frames(&events) {
                    if *frame == burst.frame && (edge - burst.edge).abs() <= 2.0 {
                        hits += 1;
                    } else {
                        unexpected += 1;
                    }
                }
            }
            eprintln!(
                "{rate:.0} pairs/s, pulse SNR {snr_db:>2} dB: {hits}/64 hits, {detected} detected, {crc_rejected} CRC rejected near start, {unexpected} unexpected"
            );
            assert_eq!(unexpected, 0);
            assert!(hits <= 64);
            if snr_db == 24 {
                assert!(hits >= 60, "{rate} pairs/s, 24 dB: {hits}/64");
            }
        }
    }
}

#[test]
fn quiet_mean_recovers_the_archived_dc_leakage_fixture() {
    for rate in [2_500_000.0, 6_000_000.0, 10_000_000.0] {
        // kajot's PR #22 offset and signal amplitude, with a quiet prefix
        // to estimate the offset and the same 511-pair block size.
        let front_end = FrontEnd {
            dc: Complex::new(0.2, 0.1),
            ..FrontEnd::at_rate(rate)
        };
        let burst = Burst {
            frame: ARCHIVED_FRAME,
            edge: 192.25,
            gain: 0.7,
            offset_hz: 0.0,
        };
        let pairs = render_pairs(rate, burst.edge);
        let (iq, _) = front_end.capture(rate, pairs, &[burst], 42);
        let estimated = iq[..128].iter().copied().sum::<Complex<f32>>() / 128.0;
        assert!((estimated - front_end.dc).norm() < 1e-6);
        let events = receive(&iq, rate, estimated);
        assert_eq!(
            valid_frames(&events)
                .map(|(frame, _)| *frame)
                .collect::<Vec<_>>(),
            vec![ARCHIVED_FRAME]
        );
    }
}

#[test]
fn strong_center_leakage_can_mask_pulses_before_dc_removal() {
    let rate = 10_000_000.0;
    let front_end = FrontEnd {
        dc: Complex::new(-0.45, 0.1),
        ..FrontEnd::at_rate(rate)
    };
    let burst = Burst {
        frame: ARCHIVED_FRAME,
        edge: 192.25,
        gain: 0.7,
        offset_hz: 0.0,
    };
    let pairs = render_pairs(rate, burst.edge);
    let (iq, _) = front_end.capture(rate, pairs, &[burst], 42);
    assert!(valid_frames(&receive(&iq, rate, Complex::ZERO))
        .next()
        .is_none());
    let estimated = iq[..128].iter().copied().sum::<Complex<f32>>() / 128.0;
    let corrected = receive(&iq, rate, estimated);
    assert_eq!(
        valid_frames(&corrected)
            .map(|(frame, _)| *frame)
            .collect::<Vec<_>>(),
        vec![ARCHIVED_FRAME]
    );
}

#[test]
fn captures_clipped_strong_and_weaker_overlapping_bursts() {
    let rate = 10_000_000.0;
    let front_end = FrontEnd {
        clip: 0.5,
        ..FrontEnd::at_rate(rate)
    };
    let bursts = [
        Burst {
            frame: ARCHIVED_FRAME,
            edge: 48.25,
            gain: 0.9,
            offset_hz: 100_000.0,
        },
        Burst {
            frame: PUBLISHED_FRAME,
            edge: 400.0,
            gain: 0.08,
            offset_hz: -200_000.0,
        },
        Burst {
            frame: PUBLISHED_FRAME,
            edge: 1680.375,
            gain: 0.08,
            offset_hz: -200_000.0,
        },
    ];
    let (iq, clipped) = front_end.capture(rate, render_pairs(rate, bursts[2].edge), &bursts, 42);
    assert!(clipped > 0);
    let events = receive(&iq, rate, Complex::ZERO);
    let frames: Vec<_> = valid_frames(&events).collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].0, &ARCHIVED_FRAME);
    assert!((frames[0].1 - bursts[0].edge).abs() <= 2.0);
    assert_eq!(frames[1].0, &PUBLISHED_FRAME);
    assert!((frames[1].1 - bursts[2].edge).abs() <= 2.0);
}

#[test]
fn seeded_noise_and_impulses_have_no_crc_valid_events() {
    for rate in R2_RATES {
        let mut iq = Rng::new(0x1090).noise(32_768, 0.02);
        for index in [256, 511, 2_048, 4_097, 20_013] {
            iq[index] += Complex::new(0.7, 0.2);
        }
        let events = receive(&iq, rate, Complex::ZERO);
        assert!(valid_frames(&events).next().is_none());
    }
}
