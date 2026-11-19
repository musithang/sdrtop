// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The sample vocabulary: how a radio's raw bytes encode I/Q, what full scale
//! means in them, the blocks the stream arrives in, and the decoder from wire
//! bytes to complex samples. Every signal crate speaks it; none of it knows
//! which radio sent the bytes.

use num_complex::Complex;

/// How raw USB bytes encode each I/Q component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    /// Interleaved signed 8-bit (HackRF). Decode `b as i8 as f32 / 128.0`.
    Int8,
    /// Interleaved unsigned 8-bit, DC bias 127.5 (RTL-SDR).
    /// Decode `(b as f32 - 127.5) / 127.5`.
    Uint8,
    /// Interleaved signed 16-bit little endian, two bytes per component, which
    /// is what SoapySDR calls `CS16`. The container is 16 bits wide; how many of
    /// them the converter actually fills is
    /// [`SampleGeometry::bits`], from the full scale the driver reports.
    ///
    Int16,
    /// Interleaved signed 32-bit little endian, four bytes per component, for
    /// a library that hands floats a radio's integer words became after its
    /// own processing: libairspyhf corrects and fine-tunes the HF+'s 16-bit
    /// USB words in floating point, and rounding them back to 16 bits lifted
    /// the measured noise floor by up to half a dB wherever the radio's own
    /// noise was under one word. The words are carried with fraction bits
    /// below them, so the full scale is the converter's times a power of two
    /// and [`SampleGeometry::converter_bits`] says how deep the converter is.
    ///
    /// A block's per-sample sums are `i64`: a sample's square is under
    /// 2^48 for anything within twice the full scale of a 24-bit container,
    /// so a block may hold 2^15 such pairs; the backend hands blocks well
    /// under that.
    Int32,
}

impl Default for SampleFormat {
    /// Only so [`SampleGeometry`] can derive `Default` for the RX accumulators.
    /// Nothing chooses a format this way: every device states its own.
    fn default() -> Self {
        SampleFormat::Int8
    }
}

/// How a device's raw bytes decode, and what "full scale" means in them.
///
/// The two questions travel together because every caller needs both: reading a
/// sample and knowing how loud it is allowed to get are the same question asked
/// twice. Keeping them apart is how a decoder ends up correct and a histogram
/// ends up drawn against the wrong ceiling.
///
/// Both shipped radios are 8-bit, so `full_scale` is 128.0 for each. That stays
/// true even for the RTL-SDR, whose analogue zero is at 127.5: the decoder
/// centres on 128 because its accumulators are integers, and
/// [`SampleGeometry::centre_bias`] is what anything reporting an offset adds
/// back.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SampleGeometry {
    pub format: SampleFormat,
    /// The sample count that means 0 dBFS.
    pub full_scale: f32,
    /// The converter's own depth, when a library has scaled its samples into
    /// a wider container than the full scale says. libairspy shifts the R2's
    /// 12-bit samples left by four and filters them into int16, so its full
    /// scale is 16384 counts while the converter behind it has 12 bits. `None`
    /// for every device whose full scale is its converter's (all the others).
    pub converter_bits: Option<u8>,
}

impl SampleGeometry {
    /// Wire bytes per I/Q pair.
    ///
    /// Two for every 8-bit format, one byte each for I and Q. Anything wider
    /// costs more, and the USB link ceiling and the block-to-sample arithmetic
    /// both need to know: a 16-bit stream at the same sample rate is twice the
    /// traffic, and calling it the same would report a device as comfortably
    /// inside a budget it is actually at the edge of.
    pub fn bytes_per_pair(&self) -> usize {
        match self.format {
            SampleFormat::Int8 | SampleFormat::Uint8 => 2,
            SampleFormat::Int16 => 4,
            SampleFormat::Int32 => 8,
        }
    }

    /// Significant bits in the converter.
    ///
    /// **Derived, not stored.** A converter's depth and its full scale are one
    /// fact written two ways, and two fields would eventually disagree. This is
    /// also the honest answer for a 12-bit ADC delivering 16-bit containers: a
    /// driver reporting a full scale of 2048 is telling us it has 12 bits, and
    /// the ADC bench would be wrong to call it 16.
    ///
    /// **Capped by the container**, because a driver can report a full scale
    /// that does not fit in the format it is sending. SoapySDR's `audio` module
    /// reports `CS16 [full-scale=65536]`, which derives to 17 bits, and there is
    /// no such thing as a 17-bit sample in a 16-bit container. Found by probing
    /// a real driver, not by imagining one.
    ///
    /// **Unless the converter's depth is stated**: a library that scales its
    /// samples up (libairspy puts 12 bits into 16384 counts) has made the
    /// full scale say more than the converter has.
    pub fn bits(&self) -> u8 {
        let derived = (self.full_scale.max(1.0).log2().round() as i32 + 1).clamp(1, 32) as u8;
        self.converter_bits
            .unwrap_or(derived)
            .min(self.container_bits())
    }

    /// Counts between the decoder's zero and the converter's own zero.
    ///
    /// The RX accumulators centre `Uint8` by subtracting 128, because
    /// the accumulators are integers and 127.5 is not one. But the converter's
    /// analogue zero sits *between* code 127 and code 128, so a perfectly
    /// centred CU8 stream decodes to a mean of -0.5 counts. Anything reporting a
    /// DC offset has to add this back or it reports the decode convention as a
    /// fault in the radio.
    ///
    /// Half an LSB is 0.0039 of full scale on an 8-bit radio, which is 78 % of
    /// the offset the IQ bench warns at and puts a -45 dBFS floor under a spike
    /// it grades from -40 dBFS. Every other format's zero is a code, so there is
    /// nothing to add back.
    pub fn centre_bias(&self) -> f64 {
        match self.format {
            SampleFormat::Uint8 => 0.5,
            SampleFormat::Int8 | SampleFormat::Int16 | SampleFormat::Int32 => 0.0,
        }
    }

    /// Counts in one step of the converter's own words: where a library's
    /// processing left fractions of a word in a wider container, the
    /// converter's top word is that many counts below the full scale, and
    /// that, not one count below it, is where its samples rail.
    ///
    /// Only [`SampleFormat::Int32`] carries such fractions. Every other format
    /// holds whole words, one count each, and its rail stays a count below
    /// the full scale whatever the converter's depth: libairspy's 12-bit R2 in
    /// int16 is filtered, not scaled by a whole number of counts, so its words
    /// have no such step to find.
    pub fn counts_per_word(&self) -> i64 {
        match self.format {
            SampleFormat::Int32 => {
                let fs = self.full_scale.max(1.0) as i64;
                (fs >> (u32::from(self.bits()).saturating_sub(1))).max(1)
            }
            SampleFormat::Int8 | SampleFormat::Uint8 | SampleFormat::Int16 => 1,
        }
    }

    /// How many bits the wire format can carry per component, whatever the
    /// converter behind it actually fills.
    fn container_bits(&self) -> u8 {
        match self.format {
            SampleFormat::Int8 | SampleFormat::Uint8 => 8,
            SampleFormat::Int16 => 16,
            SampleFormat::Int32 => 32,
        }
    }
}

/// A block of samples, plus the continuity facts a consumer that spans blocks
/// needs.
///
/// Named rather than a tuple because the two scalars answer the same question
/// from opposite sides, and only one of them is visible from the consumer.
///
/// `seq` catches a block lost **after** this point: the channels that carry
/// these are bounded and deliberately lossy, so a gap in the numbers is the only
/// record that a block existed at all. `gap_before` carries the loss the
/// sequence number cannot see, because it happened **before** the block was ever
/// stamped - samples the driver itself lost, from a HackRF short transfer or a
/// SoapySDR overflow. Both mean the samples here do not continue the samples in
/// the block before them, and anything whose state spans blocks has to be told
/// so: a CTCSS detector needs half a second of unbroken audio to tell two tones
/// two hertz apart from each other, and a preamble correlator needs to know that
/// the run it is in the middle of is a run.
pub struct StreamBlock {
    /// Monotonic across every block stamped, forwarded or not.
    pub seq: u64,
    /// The driver reported lost samples immediately before this block.
    pub gap_before: bool,
    pub bytes: Vec<u8>,
    /// Where this block's first pair sits in the stream: every pair the
    /// radio has delivered since the stream began, lost ones included (see
    /// the receive context's running count). A block continues the one before it
    /// exactly when this equals the previous block's position plus its
    /// length; anything else is a gap, or - smaller than expected - a new
    /// stream.
    pub first_pair: u64,
    /// The tuning these samples were captured at.
    ///
    /// **Stamped at capture, not read at processing.** A worker behind the
    /// stream processes a block after the tuner may have moved on - a survey
    /// retunes every tenth of a second - and reading the tuning from the state
    /// at that point attributes the block to where the radio is now rather
    /// than where it was. On BLE that meant decoding one channel's samples
    /// with another channel's whitening.
    pub centre_hz: u64,
    /// The sample rate these samples were captured at, for the same reason.
    pub rate_hz: f64,
}

/// A [`StreamBlock`] with the wall clock at its arrival, for the one worker
/// that must know when its samples were taken: FT8's cycles start on the UTC
/// quarter minute. Stamped on the driver's thread, in
/// `process::arrive`, before any queue could hold the block.
pub struct TimedBlock {
    pub block: StreamBlock,
    /// Unix seconds when the block reached the intake: after its last sample
    /// was taken, by however long the transfer and the driver held it.
    pub arrived_unix: f64,
}

/// Decode raw wire bytes into complex samples, taking at most `max_pairs`.
///
/// Deliberately unwindowed - this is a time-domain signal path, not an FFT input.
pub fn decode(buf: &[u8], geometry: SampleGeometry, max_pairs: usize, out: &mut Vec<Complex<f32>>) {
    out.clear();
    // Bytes per pair, not a constant 2: a 16-bit block holds half as many pairs
    // as its length suggests, and slicing it as though it held twice as many
    // would read past the end.
    let stride = geometry.bytes_per_pair();
    let pairs = (buf.len() / stride).min(max_pairs);
    out.reserve(pairs);
    let bytes = &buf[..pairs * stride];
    match geometry.format {
        SampleFormat::Int8 => {
            let fs = geometry.full_scale;
            for pair in bytes.as_chunks::<2>().0 {
                out.push(Complex {
                    re: pair[0] as i8 as f32 / fs,
                    im: pair[1] as i8 as f32 / fs,
                });
            }
        }
        // Half a count below mid-scale, for the reason spelled out in
        // `signal::fft::frame::decode_into`.
        SampleFormat::Uint8 => {
            let bias = geometry.full_scale - 0.5;
            for pair in bytes.as_chunks::<2>().0 {
                out.push(Complex {
                    re: (pair[0] as f32 - bias) / bias,
                    im: (pair[1] as f32 - bias) / bias,
                });
            }
        }
        SampleFormat::Int16 => {
            let fs = geometry.full_scale;
            for quad in bytes.as_chunks::<4>().0 {
                out.push(Complex {
                    re: i16::from_le_bytes([quad[0], quad[1]]) as f32 / fs,
                    im: i16::from_le_bytes([quad[2], quad[3]]) as f32 / fs,
                });
            }
        }
        // Exact in f32 up to 2^24 counts, and the full scale is a power of
        // two, so nothing the container carries is rounded here.
        SampleFormat::Int32 => {
            let fs = geometry.full_scale;
            for oct in bytes.as_chunks::<8>().0 {
                out.push(Complex {
                    re: i32::from_le_bytes([oct[0], oct[1], oct[2], oct[3]]) as f32 / fs,
                    im: i32::from_le_bytes([oct[4], oct[5], oct[6], oct[7]]) as f32 / fs,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only an unsigned format has a zero that is not a code. Half a count, and
    /// half a count is 0.0039 of full scale on an 8-bit radio: enough to sit at
    /// 78 % of the DC offset the IQ bench warns at, which is why it is added
    /// back rather than shrugged off.
    #[test]
    fn only_an_unsigned_format_is_centred_off_a_code() {
        let g = |format| SampleGeometry {
            format,
            full_scale: 128.0,
            converter_bits: None,
        };
        assert_eq!(g(SampleFormat::Uint8).centre_bias(), 0.5);
        assert_eq!(g(SampleFormat::Int8).centre_bias(), 0.0);
        assert_eq!(g(SampleFormat::Int16).centre_bias(), 0.0);
        assert_eq!(g(SampleFormat::Int32).centre_bias(), 0.0);
    }

    /// A 16-bit converter's words with eight fraction bits under them: eight
    /// bytes a pair, the converter's 16 bits (not the 24 the full scale would
    /// derive), 256 counts to one of its words. Every other format's word is
    /// one count, whatever its converter, so nothing shipped moves.
    #[test]
    fn a_wide_container_carries_its_converters_words_with_fractions() {
        let wide = SampleGeometry {
            format: SampleFormat::Int32,
            full_scale: 8_388_608.0,
            converter_bits: Some(16),
        };
        assert_eq!(wide.bytes_per_pair(), 8);
        assert_eq!(wide.bits(), 16);
        assert_eq!(wide.counts_per_word(), 256);
        let undeclared = SampleGeometry {
            converter_bits: None,
            ..wide
        };
        assert_eq!(undeclared.bits(), 24, "the full scale's own depth");
        assert_eq!(undeclared.counts_per_word(), 1);
        let r2 = SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 16384.0,
            converter_bits: Some(12),
        };
        assert_eq!(r2.counts_per_word(), 1, "whole words, filtered, not scaled");
        for format in [SampleFormat::Int8, SampleFormat::Uint8] {
            let g = SampleGeometry {
                format,
                full_scale: 128.0,
                converter_bits: None,
            };
            assert_eq!(g.counts_per_word(), 1);
        }
    }

    /// The depth follows the full scale, which is the number a SoapySDR driver
    /// actually reports. The 2048 case is the one that matters: a 12-bit ADC
    /// handing over 16-bit containers is 12 bits, and saying 16 would put a
    /// wrong bit count on the RF bench for every such radio.
    #[test]
    fn bit_depth_follows_full_scale() {
        let wide = |fs| SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: fs,
            converter_bits: None,
        };
        assert_eq!(
            SampleGeometry {
                format: SampleFormat::Int8,
                full_scale: 128.0,
                converter_bits: None,
            }
            .bits(),
            8,
            "both shipped radios"
        );
        assert_eq!(
            wide(2048.0).bits(),
            12,
            "a 12-bit ADC in a 16-bit container"
        );
        assert_eq!(wide(8192.0).bits(), 14);
        assert_eq!(wide(32768.0).bits(), 16);
    }

    fn geom(format: SampleFormat) -> SampleGeometry {
        SampleGeometry {
            format,
            full_scale: 128.0,
            converter_bits: None,
        }
    }

    #[test]
    fn decode_respects_the_slice_cap_and_format() {
        let bytes: Vec<u8> = (0..200u16).map(|v| v as u8).collect();
        let mut out = Vec::new();
        decode(&bytes, geom(SampleFormat::Int8), 10, &mut out);
        assert_eq!(out.len(), 10, "must stop at the slice cap");

        // Uint8 decodes around the 127.5 bias: 0x80 is ~zero.
        decode(&[0x80, 0x80], geom(SampleFormat::Uint8), 8, &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].re.abs() < 0.01 && out[0].im.abs() < 0.01);
    }

    /// A 16-bit block holds half as many pairs as its length suggests. The cap
    /// and the slice both have to use the stride, or the decoder reads past the
    /// end of what it was given.
    #[test]
    fn decode_counts_sixteen_bit_pairs_by_stride() {
        let wide = SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 32768.0,
            converter_bits: None,
        };
        // 16 bytes: four 16-bit pairs, or eight 8-bit ones.
        let bytes = vec![0u8; 16];
        let mut out = Vec::new();
        decode(&bytes, wide, 100, &mut out);
        assert_eq!(out.len(), 4);
        decode(&bytes, geom(SampleFormat::Int8), 100, &mut out);
        assert_eq!(out.len(), 8);
        let deep = SampleGeometry {
            format: SampleFormat::Int32,
            full_scale: 8_388_608.0,
            converter_bits: Some(16),
        };
        decode(&bytes, deep, 100, &mut out);
        assert_eq!(out.len(), 2, "two 32-bit pairs");
        let mut one = (-(1i32 << 21)).to_le_bytes().to_vec();
        one.extend_from_slice(&(1i32 << 22).to_le_bytes());
        decode(&one, deep, 100, &mut out);
        assert_eq!((out[0].re, out[0].im), (-0.25, 0.5));
    }

    /// The normalisation follows the device's full scale rather than a constant.
    /// Exercised with a fabricated wider geometry, so the scaling is proven
    /// before any real device reports one.
    #[test]
    fn decode_normalises_against_the_declared_full_scale() {
        let wide = SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 512.0,
            converter_bits: None,
        };
        let mut out = Vec::new();
        // 0x40 is +64 counts. Against 128 that is half scale; against 512 it is
        // an eighth. Same bytes, different device, different answer.
        decode(&[0x40, 0x40], geom(SampleFormat::Int8), 1, &mut out);
        assert!((out[0].re - 0.5).abs() < 1e-6, "got {}", out[0].re);
        decode(&[0x40, 0x40], wide, 1, &mut out);
        assert!((out[0].re - 0.125).abs() < 1e-6, "got {}", out[0].re);
    }
}
