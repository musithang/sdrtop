// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Protocol primitives for 1090 MHz Extended Squitter.

mod cpr;
mod message;
mod ppm;
pub use cpr::{resolve_airborne_cpr, AirborneFix, CprObservation, CprPairError, CprTimestamp};
pub use message::{
    AirbornePosition, AirborneVelocity, AirspeedKind, Altitude, AltitudeSource, CallsignError,
    CprFormat, HorizontalVelocity, Identification, Message, VelocityValue, VerticalRate,
    VerticalRateSource,
};
pub use ppm::{BitEvidence, PpmError, PpmSlicer};

/// A complete DF17 squitter whose Mode S CRC has a zero remainder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Df17Frame([u8; 14]);

/// Why a candidate could not be accepted as DF17.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Df17Error {
    Length { bytes: usize },
    DownlinkFormat { format: u8 },
    Crc { remainder: u32 },
}

impl TryFrom<&[u8]> for Df17Frame {
    type Error = Df17Error;

    /// Check exactly 112 bits in on-air byte order, with the parity included.
    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let bytes: [u8; 14] = bytes
            .try_into()
            .map_err(|_| Df17Error::Length { bytes: bytes.len() })?;
        let format = bytes[0] >> 3;
        if format != 17 {
            return Err(Df17Error::DownlinkFormat { format });
        }
        let remainder = crc_remainder(&bytes);
        if remainder != 0 {
            return Err(Df17Error::Crc { remainder });
        }
        Ok(Self(bytes))
    }
}

impl Df17Frame {
    /// The original frame, including parity.
    pub fn bytes(&self) -> &[u8; 14] {
        &self.0
    }

    /// The three capability bits in the downlink header.
    pub fn capability(&self) -> u8 {
        self.0[0] & 0b111
    }

    /// The 24-bit ICAO aircraft address reported by the frame.
    pub fn icao_address(&self) -> u32 {
        u32::from_be_bytes([0, self.0[1], self.0[2], self.0[3]])
    }

    /// The five-bit type code at the start of the ME field.
    pub fn type_code(&self) -> u8 {
        self.0[4] >> 3
    }

    /// Decode the ME field of this CRC-checked frame.
    pub fn message(&self) -> Message {
        message::decode(self)
    }
}

/// Mode S polynomial long division. Shift in the most significant bit first
/// and cancel the degree-24 term each time it appears.
fn crc_remainder(bytes: &[u8; 14]) -> u32 {
    const GENERATOR: u32 = 0x1FF_F409;
    const LEADING_TERM: u32 = 1 << 24;

    let mut remainder = 0;
    for &byte in bytes {
        for bit in (0..8).rev() {
            remainder = (remainder << 1) | u32::from((byte >> bit) & 1);
            if remainder & LEADING_TERM != 0 {
                remainder ^= GENERATOR;
            }
        }
    }
    remainder
}

#[cfg(test)]
mod tests {
    use super::*;

    // Worked polynomial division in Junzi Sun's The 1090MHz Riddle.
    // https://mode-s.org/1090mhz/content/ads-b/8-error-control.html
    const DF17: [u8; 14] = [
        0x8D, 0x40, 0x6B, 0x90, 0x20, 0x15, 0xA6, 0x78, 0xD4, 0xD2, 0x20, 0xAA, 0x4B, 0xDA,
    ];

    #[test]
    fn accepts_published_df17_and_preserves_its_bytes() {
        let frame = Df17Frame::try_from(DF17.as_slice()).unwrap();
        assert_eq!(frame.bytes(), &DF17);
        assert_eq!(frame.capability(), 5);
        assert_eq!(frame.icao_address(), 0x40_6B_90);
        assert_eq!(frame.type_code(), 4);
    }

    #[test]
    fn matches_published_parity_and_error_syndrome() {
        let mut frame = DF17;
        frame[11..].fill(0);
        assert_eq!(crc_remainder(&frame), 0xAA_4B_DA);

        let corrupt = [
            0x8D, 0x4C, 0xA2, 0x51, 0x20, 0x49, 0x94, 0xB1, 0xC3, 0x6E, 0x60, 0xA5, 0x34, 0x3D,
        ];
        assert_eq!(
            Df17Frame::try_from(corrupt.as_slice()),
            Err(Df17Error::Crc { remainder: 16 })
        );
    }

    #[test]
    fn rejects_every_single_bit_error_in_the_published_frame() {
        for bit in 0..112 {
            let mut corrupt = DF17;
            corrupt[bit / 8] ^= 0x80 >> (bit % 8);
            assert_ne!(crc_remainder(&corrupt), 0, "bit {bit}");
            assert!(
                Df17Frame::try_from(corrupt.as_slice()).is_err(),
                "bit {bit}"
            );
        }
    }

    #[test]
    fn checks_length_and_downlink_format_before_crc() {
        assert_eq!(
            Df17Frame::try_from(&DF17[..13]),
            Err(Df17Error::Length { bytes: 13 })
        );
        assert_eq!(
            Df17Frame::try_from([0; 15].as_slice()),
            Err(Df17Error::Length { bytes: 15 })
        );
        // A zero codeword has a zero CRC remainder.
        assert_eq!(
            Df17Frame::try_from([0; 14].as_slice()),
            Err(Df17Error::DownlinkFormat { format: 0 })
        );
    }
}
