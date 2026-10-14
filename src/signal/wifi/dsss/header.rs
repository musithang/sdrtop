// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The DSSS PHY header: SIGNAL, SERVICE and LENGTH under a CRC-16, and the
//! scrambler every bit goes through.

/// The CRC register after `bit` (Figure 15-2): shifted towards x15, the
/// feedback the input against x15, folded in at x12, x5 and x0.
fn step(register: u16, bit: bool) -> u16 {
    let feedback = bit ^ (register & 0x8000 != 0);
    let shifted = register << 1;
    if feedback {
        shifted ^ 0x1021
    } else {
        shifted
    }
}

/// The CRC-16 of 15.3.3.7 over `bits` in transmit order: preset to ones,
/// the remainder's ones' complement, x15 first.
pub fn crc16(bits: &[bool]) -> [bool; 16] {
    let r = !bits.iter().fold(0xffff, |r, &b| step(r, b));
    std::array::from_fn(|i| r >> (15 - i) & 1 == 1)
}

/// The feedthrough descrambler of 15.3.4 (z^-7 + z^-4 + 1): each bit out
/// is the bit in against the received bits four and seven before, so it
/// needs no seed, only the seven bits before.
#[derive(Clone, Copy, Debug, Default)]
pub struct Descrambler {
    received: u8,
}

impl Descrambler {
    /// The next bit, descrambled.
    pub fn bit(&mut self, scrambled: bool) -> bool {
        // Bit 0 is the bit received last (Z1), bit 6 seven back (Z7).
        let out = scrambled ^ (self.received >> 3 & 1 == 1) ^ (self.received >> 6 & 1 == 1);
        self.received = (self.received << 1 | scrambled as u8) & 0x7f;
        out
    }
}

/// The scrambler, for the tests' transmitter: seeded with Z1 to Z7 as the
/// standard writes them (16.2.3.2).
#[cfg(test)]
pub fn scramble(bits: &[bool], seed: [bool; 7]) -> Vec<bool> {
    // Bit 0 is Z1, the bit sent last; bit 6 is Z7.
    let mut sent = seed
        .iter()
        .enumerate()
        .fold(0u8, |r, (i, &b)| r | (b as u8) << i);
    bits.iter()
        .map(|&b| {
            let out = b ^ (sent >> 3 & 1 == 1) ^ (sent >> 6 & 1 == 1);
            sent = (sent << 1 | out as u8) & 0x7f;
            out
        })
        .collect()
}

/// The long SFD, X'F3A0', LSB first: in transmit order (15.3.3.3).
pub const LONG_SFD: [bool; 16] = {
    let mut out = [false; 16];
    let mut i = 0;
    while i < 16 {
        out[i] = 0xf3a0u16 >> i & 1 == 1;
        i += 1;
    }
    out
};

/// The short SFD, the long one's time reverse (16.2.3.10).
pub const SHORT_SFD: [bool; 16] = {
    let mut out = [false; 16];
    let mut i = 0;
    while i < 16 {
        out[i] = LONG_SFD[15 - i];
        i += 1;
    }
    out
};

/// The four rates (16.2.3.4), named by SIGNAL in units of 100 kb/s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rate {
    M1,
    M2,
    M5_5,
    M11,
}

impl Rate {
    /// The SIGNAL value.
    pub fn signal(self) -> u8 {
        match self {
            Rate::M1 => 0x0A,
            Rate::M2 => 0x14,
            Rate::M5_5 => 0x37,
            Rate::M11 => 0x6E,
        }
    }

    /// The rate in Mb/s, as it is said.
    pub fn mbps(self) -> &'static str {
        match self {
            Rate::M1 => "1",
            Rate::M2 => "2",
            Rate::M5_5 => "5.5",
            Rate::M11 => "11",
        }
    }
}

/// The two preambles (15.3.2, 16.2.2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preamble {
    /// 144 us of SYNC and SFD, the header at 1 Mb/s: 192 us.
    Long,
    /// 72 us of shortSYNC and shortSFD, the header at 2 Mb/s: 96 us.
    Short,
}

impl Preamble {
    /// The preamble and header together, in microseconds.
    pub fn us(self) -> u32 {
        match self {
            Preamble::Long => 192,
            Preamble::Short => 96,
        }
    }
}

/// What a DSSS header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub rate: Rate,
    /// SERVICE bit 2: the transmitter's carrier and chip clock come from one
    /// oscillator (16.2.3.5).
    pub locked: bool,
    /// SERVICE bit 7, the length extension at 11 Mb/s.
    pub extension: bool,
    /// The PSDU's duration in microseconds.
    pub length_us: u16,
}

/// Why a header was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Crc,
    /// SIGNAL is none of the four rates.
    Rate,
}

impl Fault {
    /// The check that refused it, in a word or two.
    pub fn label(self) -> &'static str {
        match self {
            Fault::Crc => "CRC",
            Fault::Rate => "no such rate",
        }
    }
}

impl Header {
    /// The fields of 48 bits in transmit order, the CRC checked first.
    pub fn from_bits(bits: &[bool]) -> Result<Self, Fault> {
        if crc16(&bits[..32])[..] != bits[32..48] {
            return Err(Fault::Crc);
        }
        let number = |b: &[bool]| {
            b.iter()
                .enumerate()
                .fold(0u32, |n, (i, &x)| n | (x as u32) << i)
        };
        let signal = number(&bits[..8]) as u8;
        let rate = [Rate::M1, Rate::M2, Rate::M5_5, Rate::M11]
            .into_iter()
            .find(|r| r.signal() == signal)
            .ok_or(Fault::Rate)?;
        Ok(Self {
            rate,
            locked: bits[8 + 2],
            extension: bits[8 + 7],
            length_us: number(&bits[16..32]) as u16,
        })
    }

    /// The frame's time on the air (15.4.6.7, 16.3.4): preamble and header,
    /// then LENGTH.
    pub fn airtime_us(&self, preamble: Preamble) -> u32 {
        preamble.us() + self.length_us as u32
    }

    /// The PSDU's octets (16.2.3.6): LENGTH times the rate over 8, at 11
    /// Mb/s less one when the extension bit says so.
    pub fn octets(&self) -> u32 {
        let l = self.length_us as u32;
        match self.rate {
            Rate::M1 => l / 8,
            Rate::M2 => l / 4,
            Rate::M5_5 => l * 11 / 16,
            Rate::M11 => (l * 11 / 8).saturating_sub(self.extension as u32),
        }
    }
}

/// A header's 48 bits as sent, before scrambling, with its CRC.
#[cfg(test)]
pub fn bits(h: &Header) -> [bool; 48] {
    let mut b = [false; 48];
    for (i, bit) in b.iter_mut().take(8).enumerate() {
        *bit = h.rate.signal() >> i & 1 == 1;
    }
    b[8 + 2] = h.locked;
    b[8 + 7] = h.extension;
    for i in 0..16 {
        b[16 + i] = h.length_us >> i & 1 == 1;
    }
    let crc = crc16(&b[..32]);
    b[32..].copy_from_slice(&crc);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(s: &str) -> Vec<bool> {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c == '1')
            .collect()
    }

    /// Figure 15-3, every register state, and the FCS it ends in: SIGNAL
    /// 1 Mb/s, SERVICE 0, LENGTH 192 us.
    #[test]
    fn the_crc_is_figure_15_3_step_by_step() {
        let data = word("0101 0000 0000 0000 0000 0011 0000 0000");
        let trace = [
            "1110111111011111",
            "1101111110111110",
            "1010111101011101",
            "0101111010111010",
            "1011110101110100",
            "0110101011001001",
            "1101010110010010",
            "1011101100000101",
            "0110011000101011",
            "1100110001010110",
            "1000100010001101",
            "0000000100111011",
            "0000001001110110",
            "0000010011101100",
            "0000100111011000",
            "0001001110110000",
            "0010011101100000",
            "0100111011000000",
            "1001110110000000",
            "0010101100100001",
            "0101011001000010",
            "1010110010000100",
            "0101100100001000",
            "1010001000110001",
            "0101010001000011",
            "1010100010000110",
            "0100000100101101",
            "1000001001011010",
            "0001010010010101",
            "0010100100101010",
            "0101001001010100",
            "1010010010101000",
        ];
        let mut r = 0xffffu16;
        for (bit, want) in data.iter().zip(trace) {
            r = step(r, *bit);
            assert_eq!(format!("{r:016b}"), want);
        }
        assert_eq!(crc16(&data).to_vec(), word("0101 1011 0101 0111"));
    }

    /// The example's header, read back: 1 Mb/s, 192 us, 24 octets.
    #[test]
    fn the_examples_header_is_read() {
        let mut b = word("0101 0000 0000 0000 0000 0011 0000 0000");
        b.extend(word("0101 1011 0101 0111"));
        let h = Header::from_bits(&b).unwrap();
        assert_eq!(
            h,
            Header {
                rate: Rate::M1,
                locked: false,
                extension: false,
                length_us: 192
            }
        );
        assert_eq!(h.octets(), 24);
        assert_eq!(h.airtime_us(Preamble::Long), 384);
        assert_eq!(h.airtime_us(Preamble::Short), 288);
    }

    /// Table 16-2's lengths at 11 Mb/s, and each rate's octets.
    #[test]
    fn octets_are_table_16_2s() {
        let at11 = |length_us, extension| Header {
            rate: Rate::M11,
            locked: true,
            extension,
            length_us,
        };
        for (length, ext, octets) in [
            (744, false, 1023),
            (745, false, 1024),
            (746, false, 1025),
            (747, true, 1026),
        ] {
            assert_eq!(at11(length, ext).octets(), octets, "{length}");
        }
        let at = |rate, length_us| Header {
            rate,
            locked: false,
            extension: false,
            length_us,
        };
        assert_eq!(at(Rate::M2, 400).octets(), 100);
        // 100 octets at 5.5 Mb/s: ceil(800 / 5.5) = 146 us; 146 x 5.5 / 8 =
        // 100.375, so 100.
        assert_eq!(at(Rate::M5_5, 146).octets(), 100);
    }

    /// Every field round-trips; a flipped bit fails the CRC; a SIGNAL that
    /// is no rate, with its CRC made good, is refused for that.
    #[test]
    fn fields_go_round_and_faults_are_named() {
        for rate in [Rate::M1, Rate::M2, Rate::M5_5, Rate::M11] {
            for (locked, extension, length_us) in
                [(true, false, 1), (false, true, 65_535), (true, true, 1234)]
            {
                let h = Header {
                    rate,
                    locked,
                    extension,
                    length_us,
                };
                assert_eq!(Header::from_bits(&bits(&h)), Ok(h));
            }
        }
        let good = bits(&Header {
            rate: Rate::M2,
            locked: true,
            extension: false,
            length_us: 300,
        });
        for i in 0..48 {
            let mut b = good;
            b[i] = !b[i];
            assert_eq!(Header::from_bits(&b), Err(Fault::Crc), "bit {i}");
        }
        let mut b = good;
        b[..8].copy_from_slice(&[true, false, false, false, false, false, false, false]);
        let crc = crc16(&b[..32]);
        b[32..].copy_from_slice(&crc);
        assert_eq!(Header::from_bits(&b), Err(Fault::Rate));
    }

    /// The descrambler undoes the scrambler from any seed after its first
    /// seven bits, and the long SYNC's scrambled ones come back as ones.
    #[test]
    fn the_descrambler_needs_no_seed() {
        let bits: Vec<bool> = (0..200).map(|i| (i * 7 + i / 3) % 5 < 2).collect();
        for seed in [
            [true, true, false, true, true, false, false],
            [false, false, true, true, false, true, true],
        ] {
            let sent = scramble(&bits, seed);
            assert_ne!(sent, bits);
            let mut d = Descrambler::default();
            let back: Vec<bool> = sent.iter().map(|&b| d.bit(b)).collect();
            assert_eq!(&back[7..], &bits[7..]);
        }
        let ones = scramble(&[true; 128], [true, true, false, true, true, false, false]);
        let mut d = Descrambler::default();
        let back: Vec<bool> = ones.iter().map(|&b| d.bit(b)).collect();
        assert!(back[7..].iter().all(|&b| b));
    }
}
