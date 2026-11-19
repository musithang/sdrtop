// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! From a capture to a packet: where the packet ends, where its header
//! starts among the candidates, and the decode at each.

/// The longest a legacy advertising PDU can be: 2-byte header, up to 37
/// bytes of payload, 3-byte CRC.
use super::*;

pub(super) const MAX_PDU_BYTES: usize = 2 + 37 + 3;

/// How many symbols of context [`Receiver`] keeps *before* a trigger fires -
/// see `Receiver::history` and `Receiver::try_decode`'s own docs for why the
/// true header boundary can land on either side of the trigger sample
/// itself, not only after it.
pub(super) const LOOKBACK_SYMBOLS: usize = 8;
pub(super) const LOOKBACK_SAMPLES: usize = LOOKBACK_SYMBOLS * WORKING_SPS;

/// How many symbols either side of the nominal boundary
/// (`Receiver::history`'s own length) `Receiver::try_decode` searches for a
/// clean CRC. Generous relative to the few symbols of smearing `front_end`'s
/// anti-alias filter measurably costs at the sync/header boundary - see
/// `front_end`'s own doc - with room to spare rather than tuned to the exact
/// worst case measured so far.
pub(super) const HEADER_SEARCH_SYMBOLS: usize = 6;

/// Silence, in symbols, that ends a packet: well past a Gaussian pulse's own
/// tail (about a symbol) and short of the 150 us gap before a reply on the
/// same channel, so the end found is this packet's, not the next one's.
pub(super) const END_GAP_SYMBOLS: usize = 8;

/// How far a candidate's own end may sit from the measured one and still be
/// the packet that was there: a symbol of pulse tail either side, and a
/// symbol or two of where the silence detector calls the drop.
pub(super) const END_TOLERANCE_SYMBOLS: usize = 4;

/// Where the signal in `capture[from..]` stops: the start of the first run of
/// [`END_GAP_SYMBOLS`] symbols whose power is nearer the capture's noise than
/// its signal. `None` when there is no clear signal to have stopped (less than
/// 6 dB between the two) or it never stops inside the capture.
///
/// The levels come from the capture itself: the signal from the first forty
/// symbols after `from` (the header and address, which every PDU has), the
/// noise from the quietest tenth of all its symbols. The threshold between
/// them is their geometric mean.
pub(super) fn energy_end(capture: &[Complex<f32>], from: usize) -> Option<usize> {
    let sps = WORKING_SPS;
    let symbols: Vec<f32> = capture
        .chunks(sps)
        .map(|c| c.iter().map(|s| s.norm_sqr()).sum::<f32>() / c.len() as f32)
        .collect();
    let start = from / sps;
    let lead = symbols.get(start..start + 40)?;
    let median = |v: &[f32]| {
        let mut v = v.to_vec();
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let signal = median(lead);
    let mut all = symbols.clone();
    all.sort_by(f32::total_cmp);
    let noise = all[all.len() / 10];
    // A NaN level is no contrast either.
    if signal.is_nan() || signal <= 4.0 * noise {
        return None;
    }
    let threshold = (signal * noise).sqrt();
    let mut run = 0;
    for (i, &p) in symbols.iter().enumerate().skip(start) {
        if p < threshold {
            run += 1;
            if run == END_GAP_SYMBOLS {
                return Some((i + 1 - run) * sps);
            }
        } else {
            run = 0;
        }
    }
    None
}

/// How often a capture in progress is tried for a finished packet: once an
/// octet's worth of samples, not once a sample.
///
/// A PDU grows an octet at a time, so trying between octets can only find a
/// packet a few microseconds sooner than trying at them. Trying every sample
/// did find it sooner - and paid for it with a full search of
/// [`HEADER_SEARCH_SYMBOLS`] candidate boundaries, each a discriminator, a
/// phase search and a decode over the whole capture, on every one of the
/// ~1300 samples a capture runs to: tens of thousands of decodes per trigger.
/// On a busy real channel that put the receiver at around 280 times real
/// time.
pub(super) const DECODE_EVERY_SAMPLES: usize = 8 * WORKING_SPS;

impl Receiver {
    /// Search a small range of candidate header start positions around the
    /// trigger's own nominal boundary, and return the first one whose CRC
    /// actually passes. `None` means either "not enough captured yet for
    /// any candidate" or "no candidate in range has a clean CRC yet" - both
    /// are the same instruction to the caller: keep capturing.
    ///
    /// **Why a search, and not a single trusted position.** `decode_at`
    /// does the real work at one candidate boundary; this exists because
    /// the boundary itself is not a single sample, on this receiver. "The
    /// sample right after the trigger is the header's first" holds with no
    /// filtering in the path, and not once `front_end` has its anti-alias
    /// filter: a filter
    /// with any real transition band smears the sync word's own energy into
    /// its neighbours over roughly its own settling time, on both sides of
    /// the true boundary, and a matched filter's own peak inside that
    /// smeared region can land on whichever nearby sample happens to
    /// correlate best for a given capture's own noise and content - a real,
    /// data-dependent few symbols, not a fixed offset a formula could give
    /// back. Measured directly while chasing this: the same construction,
    /// changed only in incidental ways (adding sixteen realistic symbols of
    /// lead-in before the sync word, which no earlier step's synthetic
    /// tests ever included), moved the boundary from two symbols early to
    /// three. `find_phase` already solves the identical problem one layer
    /// down, for the *sub-symbol* phase within one candidate; this is that
    /// same idea at the symbol grid above it, over [`HEADER_SEARCH_SYMBOLS`]
    /// either side of [`LOOKBACK_SAMPLES`], which is `self.capture`'s own
    /// nominal boundary once `push` starts seeding it from
    /// [`Receiver::history`] rather than empty.
    pub(super) fn try_decode(&mut self) -> Option<Heard> {
        self.candidates()
            .into_iter()
            .find(|(.., heard)| heard.crc_ok())
            .map(|(skip, phase, heard)| self.measured(skip, phase, heard))
    }

    /// A finished capture's packet to where it belongs: an advertising one
    /// to the caller, stamped with its trigger, a data one to
    /// [`Self::take_data`].
    pub(super) fn keep(&mut self, heard: Heard, found: &mut Vec<Packet>) {
        match heard {
            Heard::Advertising(mut packet) => {
                packet.at_pair = Some(self.trigger_pair);
                found.push(*packet);
            }
            Heard::Data(pdu, Some(timing)) => self.data.push((pdu, timing)),
            // Not placed in the stream is not heard: nothing could time it.
            Heard::Data(_, None) => {}
        }
    }

    /// Every alignment [`Self::try_decode`] searches, in order, decoded: the
    /// header start it was read from, the phase it was sliced at, and the
    /// packet, CRC passed or not, not yet measured ([`Self::measured`]).
    pub(super) fn candidates(&mut self) -> Vec<(usize, f64, Heard)> {
        let center = LOOKBACK_SAMPLES as isize;
        let step = WORKING_SPS as isize;
        let span = HEADER_SEARCH_SYMBOLS as isize;
        // One discriminator pass for every candidate (see `decode_at`),
        // grown by what arrived since the last try.
        self.grow();
        // And one phase search. The candidates are whole symbols apart, so
        // they share where inside a symbol to sample; searching from the
        // earliest one uses every symbol any of them will read. Thirteen
        // searches, each over nearly the same samples, were most of what a
        // capture that never passed its CRC cost; one search from scratch
        // on every try was most of what was left.
        let earliest = (center - span * step).max(0) as usize;
        let from_earliest = &self.inst[earliest.min(self.inst.len())..];
        let phase = self
            .search
            .phase(from_earliest, from_earliest.len() / WORKING_SPS);
        let mut out = Vec::new();
        // From the nominal boundary outwards, where the right one nearly
        // always is: each alignment that decodes costs a slice of the whole
        // packet, and at most one passes its CRC, so the order only decides
        // how many are sliced before it.
        let order = (0..=span).flat_map(|d| if d == 0 { vec![0] } else { vec![-d, d] });
        for k in order {
            let skip = center + k * step;
            if skip < 0 {
                continue;
            }
            let skip = skip as usize;
            if skip >= self.inst.len() {
                continue;
            }
            // The capture's own mean from here on: see `decode_at`.
            let n = self.inst.len();
            let mean = (self.inst_sums[n] - self.inst_sums[skip]) / (n - skip) as f64;
            if let Some(heard) = self.decode_at(&self.inst, skip, phase, mean as f32) {
                let passed = heard.crc_ok();
                out.push((skip, phase, heard));
                // The search stops at the first CRC that passes, as it
                // always has: later alignments cannot beat a passing one.
                if passed {
                    break;
                }
            }
        }
        out
    }

    /// At give-up, the one alignment the capture itself vouches for, reported
    /// as the packet it decodes to, CRC and all.
    ///
    /// **No alignment passed its CRC, so which to believe is chosen by
    /// something other than the CRC: the energy.** Each candidate's header
    /// says how long its packet is, which says where the packet ends; the
    /// capture says where the signal actually stopped ([`energy_end`]). The
    /// candidate whose end agrees, within [`END_TOLERANCE_SYMBOLS`], is the
    /// packet that was on the air with a bit error in it, and is counted and
    /// listed as a failed CRC. If none agrees (a false trigger, or energy that
    /// never stops because the next transmission follows), nothing is
    /// reported: choosing among alignments that nothing vouches for would be
    /// an invented packet.
    ///
    /// The one this replaced decoded at the nominal boundary alone, a few
    /// symbols from where the receiver's own measurements put the real one.
    /// On the 2026-09-19 channel 37 recording it reported 41 failed CRCs, every
    /// one a reserved PDU type from a recurring non-BLE source, and none of
    /// the three bit-error packets from a device heard fifteen times cleanly
    /// on the same recording. This reports those three (one address bit
    /// flipped in each) and 9 of the 41, where their length agrees with the
    /// signal; the CRC-good output is byte-identical on both recordings.
    pub(super) fn best_failed_candidate(&mut self) -> Option<Heard> {
        let end = energy_end(&self.capture, LOOKBACK_SAMPLES)?;
        let tolerance = END_TOLERANCE_SYMBOLS * WORKING_SPS;
        self.candidates()
            .into_iter()
            .filter_map(|(skip, phase, heard)| {
                let bits = match &heard {
                    Heard::Advertising(p) => pdu::used_bits(p.length),
                    Heard::Data(d, _) => {
                        data::HEADER_BITS
                            + 8 * d.cte_info.is_some() as usize
                            + d.payload.len() * 8
                            + data::CRC_BITS
                    }
                };
                let ends = skip + bits * WORKING_SPS;
                let off = ends.abs_diff(end);
                (off <= tolerance).then_some((off, skip, phase, heard))
            })
            .min_by_key(|(off, ..)| *off)
            .map(|(_, skip, phase, heard)| self.measured(skip, phase, heard))
    }

    /// The discriminator over the capture, extended to its end.
    pub(super) fn grow(&mut self) {
        let rate = working_rate_hz(self.phy);
        for i in self.inst.len()..self.capture.len().saturating_sub(1) {
            let f = instantaneous_freq_hz(self.capture[i], self.capture[i + 1], rate);
            self.inst.push(f);
            let total = self.inst_sums[self.inst_sums.len() - 1] + f as f64;
            self.inst_sums.push(total);
        }
    }

    /// The decode at one candidate header start, `skip` samples into the
    /// capture, given the whole capture already discriminated.
    ///
    /// **The discriminator of a capture started `skip` samples in is exactly
    /// the whole capture's discriminator from `skip` on** - each reading is a
    /// function of two neighbouring samples and nothing else - so the search
    /// over candidate boundaries slices one pass rather than making thirteen.
    ///
    /// `phase` is the sub-symbol sampling phase found for the capture (see
    /// `candidates`), and `threshold` the mean of the readings from `skip`
    /// on.
    pub(super) fn decode_at(
        &self,
        whole: &[f32],
        skip: usize,
        phase: f64,
        threshold: f32,
    ) -> Option<Heard> {
        if skip >= self.capture.len() {
            return None;
        }
        let inst = &whole[skip.min(whole.len())..];
        let symbols = inst.len() / WORKING_SPS;
        if symbols < pdu::HEADER_BITS {
            return None;
        }
        // A tuning sitting exactly on the channel's own centre - as it does
        // on the advertising views - is exactly where a real
        // front end's LO leakage and IQ DC offset concentrate, and where a
        // real transmitter's own crystal error shows up too: both are a
        // constant added to every discriminator sample, indistinguishable
        // from each other at this stage and not present in the synthetic
        // tests, which is why slicing against a fixed zero passed
        // every one of them and no real capture. The capture's own mean is
        // the honest estimate of that constant - GFSK data is balanced over
        // any real stretch of bits - and this only needs a point estimate:
        // a threshold decision does not need a calibrated uncertainty, only
        // the displayed reading below does, and gets its own.
        let sps = WORKING_SPS as f64;
        // The header first, alone. Most attempts come before the packet has
        // finished arriving, and its length says so from sixteen bits;
        // slicing and decoding the rest only to find it short was most of
        // what a busy channel cost. The same bits either way - at a fixed
        // phase and threshold each symbol is sliced on its own - so this
        // only skips work whose answer was already `None`.
        let (mut header, _) =
            crate::signal::ble::sync::slice_at(inst, sps, pdu::HEADER_BITS, threshold, phase);
        whiten(&mut header, self.channel);
        let wanted = match self.link {
            Link::Advertising | Link::Auxiliary => pdu::used_bits(pdu::length(&header)?),
            Link::Data { .. } => data::used_bits(&header)?,
        };
        if wanted > symbols {
            return None;
        }
        // The packet's own bits, not the whole capture: a candidate whose
        // header reads a short length would otherwise slice everything
        // captured so far on every try, which a busy channel paid for with
        // the square of the capture. `decode` reads no further than this.
        let (mut bits, _) = crate::signal::ble::sync::slice_at(inst, sps, wanted, threshold, phase);
        // The modulation-quality measurement needs the physically
        // transmitted (still-whitened) symbols - exactly what `bits` is
        // before the next line undoes whitening to recover the data
        // underneath them.
        // See `measure`'s own module doc for why the physical bits, not the
        // decoded ones, are what a Gaussian filter's settling depends on.
        let raw_bits = bits.clone();
        whiten(&mut bits, self.channel);
        // Where the PDU's first bit is centred, in pairs: `inst[i]` stands
        // for capture instant `i + 0.5`, and working sample `w` for raw
        // instant `delay + w * d` after the receiver's first.
        let pdu_pair = self.first_pair.map(|first| {
            let working = self.capture_origin as f64 + skip as f64 + phase + 0.5;
            first as f64 + self.decim.delay() + working * self.decim.factor() as f64
        });
        if let Link::Data { crc_init, .. } = self.link {
            let pdu = data::decode(&bits, crc_init)?;
            // A bit is WORKING_SPS working samples; back half a bit to its
            // start, then over the access address and the preamble.
            let bit = (WORKING_SPS * self.decim.factor()) as f64;
            let timing = pdu_pair.map(|centre| {
                let first_bit = centre - 0.5 * bit;
                let sync = (self.phy.preamble_bits_len() + 32) as f64;
                DataTiming {
                    start_pair: first_bit - sync * bit,
                    end_pair: first_bit + wanted as f64 * bit,
                }
            });
            return Some(Heard::Data(pdu, timing));
        }
        let mut packet = pdu::decode(&bits)?;
        // Exactly this packet's own bits: the capture runs on past it
        // (see this struct's own `push`), and letting a measurement wander
        // into trailing noise or the next packet's preamble would mix an
        // unrelated signal's deviation into this one's own reading.
        let used = pdu::used_bits(packet.length).min(raw_bits.len());
        let raw_bits = &raw_bits[..used];
        // Where the PDU sits in the stream, for the measurement to find it
        // again in the raw samples: `inst[i]` stands for capture instant
        // `i + 0.5`, and working sample `w` for raw instant `delay + w * d`
        // after the receiver's first.
        packet.air = raw_bits.to_vec();
        packet.pdu_pair = pdu_pair;
        Some(Heard::Advertising(Box::new(packet)))
    }
}
