// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The LE Coded sensitivity bench: packet error rate against Eb/N0, hard
//! decisions against soft, on the same noisy packets.
//!
//! Run by hand, in release, because it is slow and prints a table rather
//! than passing or failing:
//!
//! ```text
//! cargo test --release coded_bench -- --ignored --nocapture
//! ```
//!
//! **What it measures and what it does not.** Each packet goes through the
//! chain a live one will: GFSK at 20 Msps with a crystal offset, noise over
//! the whole band, the BLE front end, the discriminator, one reading a
//! symbol with the offset taken from the preamble. Where the packet starts
//! is given, not found (the strongest correlation with the known sync
//! symbols, near where it was put): this compares two decoders on identical
//! readings, not detection. A packet counts as received when block 1 gives
//! the access address and the scheme it was sent with and block 2's CRC
//! passes on the payload it was sent with.
//!
//! The reference point is a packet error rate of 30.8 %, the one the Core
//! states receiver sensitivity at (Vol 6 Part A 4.1).

use num_complex::Complex;

use super::coded::{self, Coding, Decisions};
use super::{coded_rx, detect, gfsk, pdu, receive, Phy};
use crate::signal::dsp::discriminate::discriminate;
use crate::signal::dsp::testkit::Rng;

const RAW_RATE: f64 = 20e6;
const RAW_SPS: usize = 20;
const WORKING_RATE: f64 = 4e6;
const WORKING_SPS: usize = 4;
const CHANNEL: u8 = 38;
const CARRIER_HZ: f64 = 2.426e9;
/// The Core's ±50 ppm crystal (Vol 6 Part A 3.1), drawn per packet.
const CRYSTAL_PPM: f64 = 50.0;
/// Random symbols either side, so the filters settle on signal, not silence.
const MARGIN_SYMBOLS: usize = 48;
const PACKETS: usize = 200;
const PER_REFERENCE: f64 = 0.308;

/// One noisy packet, decoded both ways: whether each received it.
fn trial(
    clean: &[Complex<f32>],
    payload: &[u8],
    coding: Coding,
    ebn0_db: f64,
    rng: &mut Rng,
) -> [bool; 2] {
    let offset_hz = (rng.unit() * 2.0 - 1.0) * CRYSTAL_PPM * 1e-6 * CARRIER_HZ;
    // Eb/N0 to the per-sample SNR `at_snr` would mean: the signal has unit
    // power, and an information bit lasts `symbols_per_bit` microseconds.
    let bit_rate = 1e6 / coding.symbols_per_bit() as f64;
    let snr = 10f64.powf(ebn0_db / 10.0) * bit_rate / RAW_RATE;
    let noise = rng.noise(clean.len(), 1.0 / snr);
    let step = std::f64::consts::TAU * offset_hz / RAW_RATE;
    let rx: Vec<Complex<f32>> = clean
        .iter()
        .zip(&noise)
        .enumerate()
        .map(|(n, (s, z))| {
            let rot = Complex::from_polar(1.0, (step * n as f64) as f32);
            s * rot + z
        })
        .collect();

    let mut working = Vec::new();
    receive::front_end(RAW_RATE, Phy::OneM)
        .expect("20 Msps is a working-rate multiple")
        .process(&rx, &mut working);
    let mut track = Vec::new();
    discriminate(&working, WORKING_RATE, &mut track);

    // Given timing: the strongest correlation with the sync symbols near
    // where the packet was put. Each pattern of Table 3.1 is balanced, so
    // the carrier offset adds nothing to it.
    let sync = coded::sync_symbols(detect::ADVERTISING_ACCESS_ADDRESS);
    let nominal = MARGIN_SYMBOLS * WORKING_SPS;
    let score = |at: usize| -> f32 {
        sync.iter()
            .enumerate()
            .map(|(i, &b)| {
                let v = track.get(at + i * WORKING_SPS).copied().unwrap_or(0.0);
                if b {
                    v
                } else {
                    -v
                }
            })
            .sum()
    };
    let Some(at) =
        (nominal.saturating_sub(64)..nominal + 64).max_by(|&a, &b| score(a).total_cmp(&score(b)))
    else {
        return [false, false];
    };

    let symbols = (track.len().saturating_sub(at)) / WORKING_SPS;
    let raw: Vec<f32> = (0..symbols).map(|i| track[at + i * WORKING_SPS]).collect();
    // The preamble's `00111100` is balanced: its mean is the offset.
    let offset =
        raw[..coded::PREAMBLE_SYMBOLS].iter().sum::<f32>() / coded::PREAMBLE_SYMBOLS as f32;
    let readings: Vec<f32> = raw.iter().map(|r| (r - offset) / 250e3).collect();

    let body = &readings[coded::PREAMBLE_SYMBOLS..];
    [Decisions::Hard, Decisions::Soft].map(|d| received(body, payload, coding, d))
}

fn received(body: &[f32], payload: &[u8], coding: Coding, decisions: Decisions) -> bool {
    let Some(b1) = coded::read_block1_by(body, decisions) else {
        return false;
    };
    if b1.access_address != detect::ADVERTISING_ACCESS_ADDRESS || b1.coding != Some(coding) {
        return false;
    }
    let block2 = &body[coded::BLOCK1_SYMBOLS..];
    coded::read_block2_by(block2, coding, CHANNEL, decisions)
        .and_then(|(bits, _)| pdu::decode(&bits))
        .is_some_and(|p| p.crc_ok && p.payload == payload)
}

/// Where a falling PER curve crosses `PER_REFERENCE`, in dB, by linear
/// interpolation between the two points either side.
fn crossing(curve: &[(f64, f64)]) -> Option<f64> {
    curve.windows(2).find_map(|w| {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        (y0 >= PER_REFERENCE && y1 < PER_REFERENCE)
            .then(|| x0 + (y0 - PER_REFERENCE) / (y0 - y1) * (x1 - x0))
    })
}

/// One curve pair: PER hard and soft from 0 dB up in half-dB steps, until
/// both have been at zero for two steps running.
fn curves(coding: Coding, payload_len: usize, seed: u64) -> String {
    let mut rng = Rng::new(seed);
    let payload: Vec<u8> = (0..payload_len).map(|_| rng.next_u64() as u8).collect();
    let symbols = coded::transmit(
        detect::ADVERTISING_ACCESS_ADDRESS,
        coding,
        CHANNEL,
        0x07,
        &payload,
    );
    let mut bits: Vec<bool> = (0..MARGIN_SYMBOLS)
        .map(|_| rng.next_u64() & 1 == 1)
        .collect();
    bits.extend(&symbols);
    bits.extend((0..MARGIN_SYMBOLS).map(|_| rng.next_u64() & 1 == 1));
    let clean = gfsk::modulate(&bits, RAW_SPS, 250e3, RAW_RATE, 0.5);

    let mut hard = Vec::new();
    let mut soft = Vec::new();
    let mut out = format!(
        "\n{} PDU {} octets\n  Eb/N0   PER hard   PER soft\n",
        coding.label(),
        payload_len + 2
    );
    let mut zeros = 0;
    let mut db = 0.0;
    while zeros < 2 && db <= 14.0 {
        let mut lost = [0usize; 2];
        for _ in 0..PACKETS {
            let got = trial(&clean, &payload, coding, db, &mut rng);
            for (l, ok) in lost.iter_mut().zip(got) {
                *l += usize::from(!ok);
            }
        }
        let per = lost.map(|l| l as f64 / PACKETS as f64);
        out += &format!("  {db:5.1}   {:8.3}   {:8.3}\n", per[0], per[1]);
        hard.push((db, per[0]));
        soft.push((db, per[1]));
        zeros = if per == [0.0, 0.0] { zeros + 1 } else { 0 };
        db += 0.5;
    }
    let at =
        |c: &[(f64, f64)]| crossing(c).map_or("not crossed".to_string(), |x| format!("{x:.2} dB"));
    out += &format!(
        "  PER {:.1} % at: hard {}, soft {}\n",
        PER_REFERENCE * 100.0,
        at(&hard),
        at(&soft)
    );
    out
}

#[test]
#[ignore = "a bench: slow, prints a table; run by hand in release"]
fn coded_bench() {
    let runs = [
        (Coding::S8, 8, 1),
        (Coding::S8, 48, 2),
        (Coding::S2, 8, 3),
        (Coding::S2, 48, 4),
    ];
    let tables: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = runs
            .iter()
            .map(|&(coding, len, seed)| s.spawn(move || curves(coding, len, seed)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for t in tables {
        println!("{t}");
    }
}

/// The bench's own plumbing, checked fast: a clean packet is received both
/// ways, so a PER of 1 in the table is the noise, not the bench.
#[test]
fn a_clean_packet_is_received_both_ways() {
    let mut rng = Rng::new(5);
    let payload = [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80];
    for coding in [Coding::S8, Coding::S2] {
        let symbols = coded::transmit(
            detect::ADVERTISING_ACCESS_ADDRESS,
            coding,
            CHANNEL,
            0x07,
            &payload,
        );
        let mut bits: Vec<bool> = (0..MARGIN_SYMBOLS)
            .map(|_| rng.next_u64() & 1 == 1)
            .collect();
        bits.extend(&symbols);
        bits.extend((0..MARGIN_SYMBOLS).map(|_| rng.next_u64() & 1 == 1));
        let clean = gfsk::modulate(&bits, RAW_SPS, 250e3, RAW_RATE, 0.5);
        assert_eq!(
            trial(&clean, &payload, coding, 30.0, &mut rng),
            [true, true],
            "{coding:?}"
        );
    }
}

/// One noisy packet through every candidate channel filter: whether each
/// received it, reading each symbol's mean and deciding soft. The same
/// samples go through all of them, so the comparison is paired.
fn trial_filters(
    clean: &[Complex<f32>],
    payload: &[u8],
    coding: Coding,
    ebn0_db: f64,
    rng: &mut Rng,
) -> Vec<bool> {
    let offset_hz = (rng.unit() * 2.0 - 1.0) * CRYSTAL_PPM * 1e-6 * CARRIER_HZ;
    let bit_rate = 1e6 / coding.symbols_per_bit() as f64;
    let snr = 10f64.powf(ebn0_db / 10.0) * bit_rate / RAW_RATE;
    let noise = rng.noise(clean.len(), 1.0 / snr);
    let step = std::f64::consts::TAU * offset_hz / RAW_RATE;
    let rx: Vec<Complex<f32>> = clean
        .iter()
        .zip(&noise)
        .enumerate()
        .map(|(n, (s, z))| s * Complex::from_polar(1.0, (step * n as f64) as f32) + z)
        .collect();
    let sync = coded::sync_symbols(detect::ADVERTISING_ACCESS_ADDRESS);
    let nominal = MARGIN_SYMBOLS * WORKING_SPS;
    coded_rx::CANDIDATES
        .iter()
        .map(|&filter| {
            let mut working = Vec::new();
            coded_rx::front_end(RAW_RATE, filter)
                .expect("20 Msps is a working-rate multiple")
                .process(&rx, &mut working);
            let mut track = Vec::new();
            discriminate(&working, WORKING_RATE, &mut track);
            let score = |at: usize| -> f32 {
                sync.iter()
                    .enumerate()
                    .map(|(i, &b)| {
                        let v = track.get(at + i * WORKING_SPS).copied().unwrap_or(0.0);
                        if b {
                            v
                        } else {
                            -v
                        }
                    })
                    .sum()
            };
            let Some(at) = (nominal.saturating_sub(64)..nominal + 64)
                .max_by(|&a, &b| score(a).total_cmp(&score(b)))
            else {
                return false;
            };
            let raw = coded_rx::symbol_means(&track, at as f64, usize::MAX);
            if raw.len() <= coded::PREAMBLE_SYMBOLS {
                return false;
            }
            let offset =
                raw[..coded::PREAMBLE_SYMBOLS].iter().sum::<f32>() / coded::PREAMBLE_SYMBOLS as f32;
            let readings: Vec<f32> = raw.iter().map(|r| (r - offset) / 250e3).collect();
            received(
                &readings[coded::PREAMBLE_SYMBOLS..],
                payload,
                coding,
                Decisions::Soft,
            )
        })
        .collect()
}

/// The channel filter bench: PER against Eb/N0 for every candidate, each
/// symbol's mean read and soft decisions, a 50-octet PDU; then each
/// candidate's taps and the time its front end takes per millisecond of
/// 20 Msps signal.
#[test]
#[ignore = "a bench: slow, prints a table; run by hand in release"]
fn coded_filter_bench() {
    const POINTS: [f64; 19] = [
        8.0, 8.5, 9.0, 9.5, 10.0, 10.5, 11.0, 11.5, 12.0, 12.5, 13.0, 13.5, 14.0, 14.5, 15.0, 15.5,
        16.0, 16.5, 17.0,
    ];
    const FILTER_PACKETS: usize = 150;
    for (coding, seed) in [(Coding::S8, 21u64), (Coding::S2, 22)] {
        let mut rng = Rng::new(seed);
        let payload: Vec<u8> = (0..48).map(|_| rng.next_u64() as u8).collect();
        let symbols = coded::transmit(
            detect::ADVERTISING_ACCESS_ADDRESS,
            coding,
            CHANNEL,
            0x07,
            &payload,
        );
        let mut bits: Vec<bool> = (0..MARGIN_SYMBOLS)
            .map(|_| rng.next_u64() & 1 == 1)
            .collect();
        bits.extend(&symbols);
        bits.extend((0..MARGIN_SYMBOLS).map(|_| rng.next_u64() & 1 == 1));
        let clean = gfsk::modulate(&bits, RAW_SPS, 250e3, RAW_RATE, 0.5);

        let rows: Vec<(f64, Vec<f64>)> = std::thread::scope(|s| {
            let handles: Vec<_> = POINTS
                .iter()
                .enumerate()
                .map(|(k, &db)| {
                    let (clean, payload) = (&clean, &payload);
                    s.spawn(move || {
                        let mut rng = Rng::new(seed * 1000 + k as u64);
                        let mut lost = vec![0usize; coded_rx::CANDIDATES.len()];
                        for _ in 0..FILTER_PACKETS {
                            let got = trial_filters(clean, payload, coding, db, &mut rng);
                            for (l, ok) in lost.iter_mut().zip(got) {
                                *l += usize::from(!ok);
                            }
                        }
                        let per = lost
                            .iter()
                            .map(|&l| l as f64 / FILTER_PACKETS as f64)
                            .collect();
                        (db, per)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let mut out = format!(
            "\n{} PDU 50 octets, PER by channel filter (cutoff kHz)\n  Eb/N0",
            coding.label()
        );
        for f in coded_rx::CANDIDATES {
            out += &format!("  {:>6.0}", f.cutoff_hz / 1e3);
        }
        out += "\n";
        for (db, per) in &rows {
            out += &format!("  {db:5.1}");
            for p in per {
                out += &format!("  {p:6.3}");
            }
            out += "\n";
        }
        out += "  PER 30.8 % at:";
        for i in 0..coded_rx::CANDIDATES.len() {
            let curve: Vec<(f64, f64)> = rows.iter().map(|(db, per)| (*db, per[i])).collect();
            out += &crossing(&curve).map_or("  not crossed".to_string(), |x| format!("  {x:5.2}"));
        }
        println!("{out}");
    }

    let block: Vec<Complex<f32>> = Rng::new(1).noise(4_000_000, 1.0);
    println!("\nfilter     taps   ms per 1 ms of 20 Msps signal");
    for f in coded_rx::CANDIDATES {
        let taps = crate::signal::dsp::fir::design_lowpass_to_spec(
            f.cutoff_hz / RAW_RATE,
            f.transition_hz / RAW_RATE,
            40.0,
        )
        .len();
        let mut fe = coded_rx::front_end(RAW_RATE, f).unwrap();
        let mut out = Vec::new();
        let t = std::time::Instant::now();
        fe.process(&block, &mut out);
        let per_ms = t.elapsed().as_secs_f64() * 1e3 / (block.len() as f64 / RAW_RATE * 1e3);
        println!("{:>6.0} kHz  {taps:>4}   {per_ms:.3}", f.cutoff_hz / 1e3);
    }
}

/// The receiver bench: the same curves as [`coded_filter_bench`]'s chosen
/// filter, but through the whole `CodedReceiver`, detection and timing found
/// rather than given, so what the chain loses to finding the packet shows
/// beside what it would decode with the timing known.
#[test]
#[ignore = "a bench: slow, prints a table; run by hand in release"]
fn coded_receiver_bench() {
    const POINTS: [f64; 17] = [
        9.0, 9.5, 10.0, 10.5, 11.0, 11.5, 12.0, 12.5, 13.0, 13.5, 14.0, 14.5, 15.0, 15.5, 16.0,
        16.5, 17.0,
    ];
    const RX_PACKETS: usize = 150;
    for (coding, seed) in [(Coding::S8, 31u64), (Coding::S2, 32)] {
        let mut rng = Rng::new(seed);
        let payload: Vec<u8> = (0..48).map(|_| rng.next_u64() as u8).collect();
        let symbols = coded::transmit(
            detect::ADVERTISING_ACCESS_ADDRESS,
            coding,
            CHANNEL,
            0x07,
            &payload,
        );
        let mut bits: Vec<bool> = (0..MARGIN_SYMBOLS * 4)
            .map(|_| rng.next_u64() & 1 == 1)
            .collect();
        bits.extend(&symbols);
        bits.extend((0..MARGIN_SYMBOLS * 4).map(|_| rng.next_u64() & 1 == 1));
        let clean = gfsk::modulate(&bits, RAW_SPS, 250e3, RAW_RATE, 0.5);

        let rows: Vec<(f64, f64)> = std::thread::scope(|s| {
            let handles: Vec<_> = POINTS
                .iter()
                .enumerate()
                .map(|(k, &db)| {
                    let (clean, payload) = (&clean, &payload);
                    s.spawn(move || {
                        let mut rng = Rng::new(seed * 1000 + k as u64);
                        let bit_rate = 1e6 / coding.symbols_per_bit() as f64;
                        let snr = 10f64.powf(db / 10.0) * bit_rate / RAW_RATE;
                        let mut lost = 0usize;
                        for _ in 0..RX_PACKETS {
                            let offset_hz =
                                (rng.unit() * 2.0 - 1.0) * CRYSTAL_PPM * 1e-6 * CARRIER_HZ;
                            let step = std::f64::consts::TAU * offset_hz / RAW_RATE;
                            let noise = rng.noise(clean.len(), 1.0 / snr);
                            let rx: Vec<Complex<f32>> = clean
                                .iter()
                                .zip(&noise)
                                .enumerate()
                                .map(|(n, (s, z))| {
                                    s * Complex::from_polar(1.0, (step * n as f64) as f32) + z
                                })
                                .collect();
                            let mut receiver =
                                coded_rx::CodedReceiver::new(RAW_RATE, CHANNEL, CARRIER_HZ)
                                    .expect("channel 38 at 20 Msps");
                            let got = receiver.push_iq_at(&rx, 0);
                            let ok = got.iter().any(|p| p.crc_ok && p.payload == *payload);
                            lost += usize::from(!ok);
                        }
                        (db, lost as f64 / RX_PACKETS as f64)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut out = format!(
            "\n{} PDU 50 octets, through the receiver\n  Eb/N0   PER\n",
            coding.label()
        );
        for (db, per) in &rows {
            out += &format!("  {db:5.1}   {per:.3}\n");
        }
        out += &crossing(&rows).map_or("  PER 30.8 % not crossed\n".to_string(), |x| {
            format!("  PER 30.8 % at {x:.2} dB\n")
        });
        println!("{out}");
    }
}
