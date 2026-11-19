// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A DSSS transmitter for the tests, built as clauses 15 and 16 build one:
//! the preamble and header scrambled, DBPSK or DQPSK, each symbol spread
//! by the Barker chips.

use num_complex::Complex;

use super::chips::chips;
use super::header::{bits, scramble, Header, Preamble, LONG_SFD, SHORT_SFD};

/// A frame's symbols: the preamble and `header` as `preamble` sends them,
/// then `data` DBPSK symbols of random data standing in for the PSDU.
pub fn symbols(
    preamble: Preamble,
    header: &Header,
    data: usize,
    rng: &mut crate::signal::dsp::testkit::Rng,
) -> Vec<Complex<f64>> {
    let (sync, sfd, seed) = match preamble {
        Preamble::Long => (
            true,
            LONG_SFD,
            [true, true, false, true, true, false, false],
        ),
        Preamble::Short => (
            false,
            SHORT_SFD,
            [false, false, true, true, false, true, true],
        ),
    };
    let sync_len = if sync { 128 } else { 56 };
    let mut plain: Vec<bool> = vec![sync; sync_len];
    plain.extend(sfd);
    plain.extend(bits(header));
    plain.extend((0..data).map(|_| rng.next_u64() & 1 == 1));
    let sent = scramble(&plain, seed);
    let header_at = sync_len + 16;
    let mut phase = 0.0f64;
    let mut out = Vec::new();
    let mut i = 0;
    while i < sent.len() {
        let dqpsk = preamble == Preamble::Short && i >= header_at && i < header_at + 48;
        if dqpsk {
            // Dibits d0 d1, d0 first: 00 0, 01 pi/2, 11 pi, 10 -pi/2
            // (Table 15-8).
            phase += match (sent[i], sent[i + 1]) {
                (false, false) => 0.0,
                (false, true) => std::f64::consts::FRAC_PI_2,
                (true, true) => std::f64::consts::PI,
                (true, false) => -std::f64::consts::FRAC_PI_2,
            };
            i += 2;
        } else {
            if sent[i] {
                phase += std::f64::consts::PI;
            }
            i += 1;
        }
        out.push(Complex::from_polar(1.0, phase));
    }
    out
}

/// The frame at 20 Msps through our capture's +-10 MHz, with `pad`
/// symbols of silence either side.
pub fn frame(
    preamble: Preamble,
    header: &Header,
    data: usize,
    pad: usize,
    rng: &mut crate::signal::dsp::testkit::Rng,
) -> Vec<Complex<f64>> {
    chips(&symbols(preamble, header, data, rng), pad, true)
}
