// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What a decoded packet is measured as: its SNR, its carrier offset with
//! the uncertainty it has, and the advertising channel's figures.

use super::*;

impl Receiver {
    /// The carrier offset, read from the sync word this capture was triggered
    /// on.
    ///
    /// **Data-aided, because the data is not balanced.** The mean of the
    /// packet's own symbols assumes that whitened data has as many ones as
    /// zeros. Over a real stretch of bits it has roughly as many,
    /// and a short packet's surplus of either moves the mean by the deviation
    /// times the surplus fraction: a synthetic packet sent 15 kHz off was
    /// reported at 35. The preamble and access address are known exactly.
    ///
    /// **And read as a tone, not as a mean of frequencies.** Multiplying the
    /// received sync word by the conjugate of the reference strips the
    /// modulation off and leaves `z[k]`, a constant-amplitude tone at the
    /// carrier offset. Its frequency is the slope of its phase, fitted by
    /// least squares - every sample of the sync word contributing, rather
    /// than one discriminator
    /// reading per symbol, and precise enough that turning the sync word back
    /// by it leaves the coherence the SNR is read from intact (see
    /// [`Self::corrected_snr_db`]). A first version read the offset from one
    /// discriminator reading per symbol; its ±20 kHz of scatter was enough
    /// to spin the phase across the window again and report CRC-clean
    /// packets at -7 dB.
    ///
    /// The uncertainty is the fit's: the scatter of the tone's phase about the
    /// line, through as many independent points as the residuals'
    /// autocorrelation says there are. Two earlier versions each got this
    /// wrong in a different direction - the scatter of per-sample phase steps
    /// overstated it several times (neighbouring steps share a sample, so
    /// their errors cancel along the line), and one point per symbol
    /// overstated it by two thirds. `the_offset_s_uncertainty_matches_its_
    /// scatter` holds the stated figure to the measured one.
    pub(super) fn sync_offset(&self) -> Option<crate::signal::dsp::uncertainty::Uncertain> {
        let n = self.reference.len();
        if self.sync_window.len() != n || n < 2 * WORKING_SPS {
            return None;
        }
        let tone: Vec<Complex<f64>> = self
            .sync_window
            .iter()
            .zip(&self.reference)
            .map(|(w, r)| {
                Complex::new(w.re as f64, w.im as f64) * Complex::new(r.re as f64, -(r.im as f64))
            })
            .collect();
        let steps: Vec<Complex<f64>> = tone.windows(2).map(|p| p[1] * p[0].conj()).collect();
        let sum: Complex<f64> = steps.iter().sum();
        if sum.norm() == 0.0 {
            return None;
        }
        let mean_step = sum.arg();
        // The tone's phase, unwrapped about the mean step so a large offset
        // never wraps, then a least-squares line through it: its slope is the
        // offset, its scatter about the line the noise the slope was read
        // through.
        let mut phase = Vec::with_capacity(tone.len());
        let mut acc = 0.0f64;
        phase.push(0.0);
        for (k, step) in steps.iter().enumerate() {
            acc += (step * Complex::from_polar(1.0, -mean_step)).arg();
            phase.push(acc + mean_step * (k + 1) as f64);
        }
        let n_pts = phase.len() as f64;
        let k_mean = (n_pts - 1.0) / 2.0;
        let p_mean = phase.iter().sum::<f64>() / n_pts;
        let sxx: f64 = (0..phase.len()).map(|k| (k as f64 - k_mean).powi(2)).sum();
        let sxy: f64 = phase
            .iter()
            .enumerate()
            .map(|(k, p)| (k as f64 - k_mean) * (p - p_mean))
            .sum();
        let slope = sxy / sxx;
        let residual: Vec<f64> = phase
            .iter()
            .enumerate()
            .map(|(k, p)| p - p_mean - slope * (k as f64 - k_mean))
            .collect();
        let residual_var = residual.iter().map(|r| r * r).sum::<f64>() / (n_pts - 2.0).max(1.0);
        if residual_var <= 0.0 {
            return Some(crate::signal::dsp::uncertainty::Uncertain::exact(
                slope * working_rate_hz(self.phy) / std::f64::consts::TAU,
            ));
        }
        // How many independent points the line was really fitted through.
        // The samples are not independent - the front end and the Gaussian
        // shaping both spread each error over its neighbours - and assuming
        // they were understates the uncertainty; assuming one per symbol
        // overstated it. The residuals' own autocorrelation says: the usual
        // effective sample size, `n / (1 + 2 sum rho_lag)`, over the lags a
        // symbol or two spans.
        let lag_sum: f64 = (1..=2 * WORKING_SPS)
            .map(|lag| {
                residual
                    .iter()
                    .zip(&residual[lag..])
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
                    / ((residual.len() - lag) as f64 * residual_var)
            })
            .sum();
        let n_eff = (n_pts / (1.0 + 2.0 * lag_sum)).clamp(2.0, n_pts);
        // A line's slope through `n_eff` independent points spread over the
        // same span has variance `12 sigma^2 / (n (n^2 - 1))` per spacing
        // squared; the spacing is `n_pts / n_eff` samples.
        let spacing = n_pts / n_eff;
        let per_sample = (12.0 * residual_var / (n_eff * (n_eff * n_eff - 1.0))).sqrt() / spacing;
        let rate = working_rate_hz(self.phy);
        let to_hz = rate / std::f64::consts::TAU;
        Some(crate::signal::dsp::uncertainty::Uncertain::from_sigma(
            slope * to_hz,
            per_sample * to_hz,
        ))
    }

    /// The packet's SNR, from the sync word it was triggered on, once its
    /// carrier offset is known.
    ///
    /// The sync word's samples are turned back by `offset_hz` and correlated
    /// coherently with the reference, and the coherence goes through
    /// `snr_from_metric`. Before the offset was taken out, the
    /// coherence - and so the SNR - was pulled down by the transmitter's own
    /// crystal error, reporting an offset device as a weak one.
    ///
    /// `snr_from_metric` was derived for `Coherence::metric` - two noisy
    /// copies of the same unknown signal correlated against each other - and
    /// this is a noisy signal against a known, noiseless reference. For a
    /// unit-power reference at this window length the two converge to the
    /// same `rho = snr / (1 + snr)` relationship, so the tested inverse is
    /// reused: a close approximation here, not proven exact.
    pub(super) fn corrected_snr_db(&self, offset_hz: f64) -> Option<f64> {
        let n = self.reference.len();
        if self.sync_window.len() != n || self.reference_energy <= 0.0 {
            return None;
        }
        let rate = working_rate_hz(self.phy);
        let mut value = Complex::new(0.0f64, 0.0);
        let mut window_energy = 0.0f64;
        for (k, (r, w)) in self.reference.iter().zip(&self.sync_window).enumerate() {
            let ph = -std::f64::consts::TAU * offset_hz * k as f64 / rate;
            let w = Complex::new(w.re as f64, w.im as f64) * Complex::new(ph.cos(), ph.sin());
            value += Complex::new(r.re as f64, -(r.im as f64)) * w;
            window_energy += w.norm_sqr();
        }
        if window_energy <= 0.0 {
            return None;
        }
        let coherence = value.norm_sqr() / (self.reference_energy * window_energy);
        snr_from_metric(coherence, n).map(|snr| 10.0 * snr.log10())
    }

    /// `packet`, decoded at `skip` and `phase`, with its offset, SNR, modulation and drift
    /// read: once, for the packet the search settled on, never for the
    /// alignments it tried on the way. Building the rebuilt waveform for
    /// every alignment that decoded, CRC or not, on every attempt while a
    /// packet was still arriving, put a clean channel at 35 times real time.
    ///
    /// The figures are read on the slicer's own bit grid, but from the
    /// rebuilt waveform, not from a straight line between two readings a
    /// quarter of a symbol apart (`Oversampled`'s doc for what that cost),
    /// and as the test suites define them (`dsp::deviation::suite_readings`).
    /// The slicer keeps the plain readings: a bit is decided by which side
    /// of the line it falls, and that the chord gets right. `inst[i]` sits
    /// halfway between capture samples `i` and `i + 1`, scaled by the PHY's
    /// own symbol rate.
    ///
    /// **The modulation and drift on LE 2M only.** LE 1M is read by the
    /// worker from the raw samples
    /// (`signal::net::measure`), through the measurement filter and timed by
    /// the packet's known bits, and reading it here as well built the
    /// rebuilt waveform twice for every packet, a fifth of what a busy
    /// channel cost, for figures the worker then replaced.
    pub(super) fn measured(&self, skip: usize, phase: f64, heard: Heard) -> Heard {
        match heard {
            Heard::Advertising(packet) => {
                Heard::Advertising(Box::new(self.measured_advertising(skip, phase, *packet)))
            }
            // A data packet's figures are its timing, already read.
            data @ Heard::Data(..) => data,
        }
    }

    pub(super) fn measured_advertising(
        &self,
        skip: usize,
        phase: f64,
        mut packet: Packet,
    ) -> Packet {
        // The *reported* offset is read against the sync word, not the
        // packet's data: see `sync_offset`. The slicing threshold never had
        // to be exact, only good enough to slice against; this one is what a
        // reader sees a `+/-` on. It and the SNR depend on the sync word
        // alone, not on the alignment: worked out for every alignment that
        // decoded, they were a sine and a cosine a sample of the sync word,
        // many times a packet.
        let offset = self.sync_offset();
        packet.freq_offset_hz = offset;
        packet.snr_db = offset.and_then(|o| self.corrected_snr_db(o.value()));
        if self.phy == Phy::OneM {
            return packet;
        }
        let fine = Oversampled::new(&self.capture, working_rate_hz(self.phy));
        let sps = WORKING_SPS as f64;
        // Bit `x` periods into the PDU, as a capture instant: bit `k`'s
        // centre, `k + 0.5`, is where the slicer sampled it.
        let at = |x: f64| fine.at(skip as f64 + phase + (x - 0.5) * sps + 0.5);
        // Every bit read once, for the modulation and the carrier both.
        let readings = crate::signal::dsp::deviation::BitReadings::read(packet.air.len(), at);
        packet.modulation =
            crate::signal::dsp::deviation::suite_readings_from(&packet.air, &readings).and_then(
                |(settled, alternating)| {
                    crate::signal::ble::measure::modulation_from(&settled, &alternating, self.phy)
                },
            );
        // The preamble is not in the capture: the first block stands for f0.
        let carrier = crate::signal::dsp::carrier::by_bit_from(&packet.air, &readings);
        let crc_start = packet.air.len().saturating_sub(pdu::CRC_BITS);
        let blocks = crate::signal::dsp::carrier::ten_bit_blocks(&carrier, 1, crc_start);
        packet.drift = crate::signal::ble::measure::drift_from(None, &blocks, self.phy);
        packet
    }
}
