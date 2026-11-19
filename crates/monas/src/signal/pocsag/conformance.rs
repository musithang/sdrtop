// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The yardstick the POCSAG receiver is held to: a grid of transmissions with
//! known rate, shift, offset, clock error and noise, each made in closed form
//! by the test transmitter (`demod::tests::fsk`), and the receiver's pages and
//! readings checked against what was sent.
//!
//! Test-only. Every row prints, so a change to the receiver is judged by the
//! whole table, not by whether one assertion still passes: a shortcut is
//! accepted only if no row gets worse.
//!
//! Noise is stated as the SNR over the channel's whole sample rate
//! (`dsp::testkit::at_snr`), 38.4 kHz, not per bit and not in the signal's
//! own bandwidth.

use super::demod::tests::{air_bits, alpha, fsk, Air};
use super::demod::{Decoder, Received, RATES};
use super::page::tests::transmit;
use super::page::Function;
use crate::signal::dsp::testkit::{at_snr, Rng};

const RATE: f64 = 38_400.0;

/// A page long enough to run into a third batch, so the clock is measured
/// over more than one.
const TEXT: &str = "THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG 0123456789. \
                    PACK MY BOX WITH FIVE DOZEN LIQUOR JUGS, THEN SEND IT ON.";

struct Row {
    baud: u32,
    offset: f64,
    ppm: f64,
    snr_db: Option<f64>,
    read: usize,
    of: usize,
    shift_err_pct: f64,
    offset_err_hz: f64,
    ppm_err: f64,
    decision_snr_db: f64,
}

fn run(baud: u32, offset: f64, ppm: f64, snr_db: Option<f64>, trials: usize) -> Row {
    let air = Air {
        offset_hz: offset,
        ppm,
        ..Air::at(baud)
    };
    let batches = transmit(&[(200_001, Function::D, alpha(TEXT))]);
    let clean = fsk(&air_bits(&batches, 576), air, RATE, 4_000, 4_000);
    let mut read = 0;
    let (mut shift_err, mut offset_err, mut ppm_err, mut dsnr) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for seed in 0..trials as u64 {
        let mut rng = Rng::new(1_000 + seed);
        let iq = match snr_db {
            Some(snr) => at_snr(&clean, snr, &mut rng),
            None => clean.clone(),
        };
        let mut d = Decoder::new(RATE);
        let mut got: Vec<Received> = iq.chunks(4_096).flat_map(|b| d.push(b)).collect();
        got.extend(d.reset());
        let Some(r) = got.iter().find(|r| r.page.ric == 200_001) else {
            continue;
        };
        if r.page.text() != TEXT {
            continue;
        }
        read += 1;
        let l = &r.link;
        shift_err = shift_err.max(((l.shift_hz / air.shift_hz) - 1.0).abs() * 100.0);
        offset_err = offset_err.max((l.offset_hz - offset).abs());
        ppm_err = ppm_err.max((l.clock_ppm - ppm).abs());
        dsnr += l.decision_snr_db.unwrap_or(f64::NAN);
    }
    Row {
        baud,
        offset,
        ppm,
        snr_db,
        read,
        of: trials,
        shift_err_pct: shift_err,
        offset_err_hz: offset_err,
        ppm_err,
        decision_snr_db: dsnr / read.max(1) as f64,
    }
}

fn print(rows: &[Row]) {
    eprintln!(" baud  offset Hz    ppm   SNR dB   read   shift err %   offset err Hz   ppm err   decision SNR dB");
    for r in rows {
        eprintln!(
            "{:>5}  {:>9.0}  {:>5.0}   {:>6}   {:>2}/{:<2}   {:>11.2}   {:>13.1}   {:>7.1}   {:>15.1}",
            r.baud,
            r.offset,
            r.ppm,
            r.snr_db.map_or("clean".to_string(), |s| format!("{s:.0}")),
            r.read,
            r.of,
            r.shift_err_pct,
            r.offset_err_hz,
            r.ppm_err,
            r.decision_snr_db,
        );
    }
}

/// Every rate, offset and clock error, clean and at 15 dB: every page read,
/// every reading inside its bound. The bounds are what the receiver was
/// measured to do (2026-10-24: shift within 0.15 %, offset within 5.5 Hz,
/// clock within 1.8 ppm), with headroom; a change that makes any row worse
/// than these fails here.
#[test]
fn the_receiver_reads_and_measures_the_grid() {
    let mut rows = Vec::new();
    for baud in RATES {
        for (offset, ppm) in [(0.0, 0.0), (2_500.0, 300.0), (-2_500.0, -300.0)] {
            rows.push(run(baud, offset, ppm, None, 1));
            rows.push(run(baud, offset, ppm, Some(15.0), 4));
        }
    }
    print(&rows);
    for r in &rows {
        assert_eq!(
            r.read, r.of,
            "{} baud, {} Hz, {} ppm: pages lost",
            r.baud, r.offset, r.ppm
        );
        assert!(
            r.shift_err_pct < 0.5,
            "{} baud: shift {:.2} %",
            r.baud,
            r.shift_err_pct
        );
        assert!(
            r.offset_err_hz < 12.0,
            "{} baud: offset {:.1} Hz",
            r.baud,
            r.offset_err_hz
        );
        assert!(
            r.ppm_err < 5.0,
            "{} baud: clock {:.1} ppm",
            r.baud,
            r.ppm_err
        );
    }
}

/// How far down the noise the receiver still reads a page, at each rate:
/// printed as a table, and held where it was measured (2026-10-24: every
/// rate 10 of 10 at 3 dB over the channel's 38.4 kHz).
#[test]
fn sensitivity() {
    let mut rows = Vec::new();
    for baud in RATES {
        for snr in [12.0, 9.0, 6.0, 3.0] {
            rows.push(run(baud, 0.0, 0.0, Some(snr), 10));
        }
    }
    print(&rows);
    for r in &rows {
        let snr = r.snr_db.unwrap_or(f64::INFINITY);
        assert!(
            r.read >= 9,
            "{} baud at {snr} dB: {} of {}",
            r.baud,
            r.read,
            r.of
        );
    }
}
