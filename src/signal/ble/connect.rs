// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! B20's own primitive layer: parsing a CONNECT_IND/AUX_CONNECT_REQ's own
//! `LLData`, and Channel Selection Algorithm #1 - "capture a `CONNECT_IND`,
//! learn the access address, CRC init, hop increment, channel map and
//! connection interval, then hop along with the connection" (design
//! section 1.3), scoped down before landing anything.
//!
//! **Read directly from the primary source, the same page B18's own
//! module doc already cites** (the Bluetooth SIG's own public Core
//! Specification, Core-54, Volume 6 Part B, Link Layer Specification):
//! section 2.3.3.1 for the `CONNECT_IND`/`AUX_CONNECT_REQ` payload's own
//! ten `LLData` fields and what each one means, and section 4.5.8.2 for
//! Channel Selection Algorithm #1 itself, quoted rather than paraphrased
//! in [`Csa1::next`]'s own doc.
//!
//! **Field byte widths are the one part of this reasoned rather than
//! read directly - `LLData`'s own diagram (Figure 2.13) is an image, not
//! text this session could fetch and quote.** What is directly quoted:
//! "The LLData consists of 10 fields" and each field's own meaning and
//! unit (`WinSize`, `Interval`, `Latency` and `Timeout` are all defined in
//! terms of a multiplier the text states exactly). The byte width of
//! each field (`AA` 4, `CRCInit` 3, `WinSize` 1, `WinOffset` 2, `Interval`
//! 2, `Latency` 2, `Timeout` 2, `ChM` 5, `Hop`+`SCA` packed into 1) is the
//! standard, widely-corroborated layout - checked here for internal
//! consistency (they sum to exactly 22 bytes, the well-known `LLData`
//! length, and `InitA`(6) + `AdvA`(6) + `LLData`(22) sum to exactly 34
//! bytes, `CONNECT_IND`'s own well-known payload length) rather than
//! read from the figure itself. `SCA`'s own 3-bit width is directly
//! confirmed by Table 2.11's own eight rows (values 0 through 7); `Hop`'s
//! 5-bit width is confirmed by the text's own stated range, "5 to 16",
//! which needs at least 5 bits and no more.
//!
//! **Algorithm #2 was added later** ([`Csa2`], net-ux-polish-plan 5.4.b4, at
//! Viktor's request), held to the specification's own sample data; the
//! paragraph below is B20's original scoping, kept as the record of why it
//! was first left out.
//!
//! **Scope, decided before landing anything: Channel Selection Algorithm
//! #1 only, not Algorithm #2.** Algorithm #2 is real, separate work - a
//! PRNG-like permutation function with several more inputs, not a
//! generalisation of Algorithm #1's own plain modular arithmetic.
//! `CONNECT_IND`'s own header carries a `ChSel` bit saying which one a
//! connection actually uses (set to 1 only if both the initiator and the
//! advertiser support Algorithm #2); `pdu::decode` reads it since
//! net-ux-polish-plan 5.1, and the packet detail view predicts each
//! connection by the algorithm the bit names.
//!
//! **Read, not followed.** Since net-ux-polish-plan 5.4.b3 the packet
//! detail view decodes a received `CONNECT_IND` ([`decode_octets`]) and shows
//! its parameters and, for Algorithm #1, the first channels [`Csa1`]
//! predicts, labelled "predicted, not followed". No live receiver captures
//! a `CONNECT_IND` and starts hopping; `signal::ble::receive`
//! only ever demodulates whichever one channel the radio is tuned to.
//! The same honest scope every large piece of this arc has landed with
//! first (B14's own `access_code`, B16's own `header`, B18's own
//! `coded`), for the reason this checkpoint's own write-up in
//! `bluetooth-bench-plan.md` gives in full: B19's own retune latency has
//! never been measured against a real backend, so whether live hopping
//! is even physically possible on the hardware this app actually runs on
//! is still unknown.

/// One `CONNECT_IND`/`AUX_CONNECT_REQ` PDU's own payload, decoded - the
/// initiator's and the advertiser's own addresses, and the ten `LLData`
/// fields section 2.3.3.1 names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectIndData {
    pub init_a: [u8; 6],
    pub adv_a: [u8; 6],
    /// The connection's own Access Address - distinct from, and replacing,
    /// the fixed advertising one for every packet on this connection from
    /// here on.
    pub access_address: u32,
    /// The connection's own CRC-24 initial value - `libbtbb`-style
    /// classic Bluetooth aside, this is the first place in this whole
    /// arc's own BLE side that a CRC does not start from a fixed constant
    /// (the advertising channels' whitening and CRC both being channel-
    /// keyed, not connection-keyed).
    pub crc_init: u32,
    pub win_size: u8,
    pub win_offset: u16,
    pub interval: u16,
    pub latency: u16,
    pub timeout: u16,
    /// Bit `i` set means data channel index `i` is used - bits 37 to 39
    /// are reserved and always read as unset here, matching the cited
    /// text's own "reserved for future use."
    pub channel_map: u64,
    /// The value [`Csa1::new`] itself is built from - 5 bits, 5 to 16 per
    /// the cited text.
    pub hop_increment: u8,
    /// 3 bits, Table 2.11's own eight-row encoding of a worst-case sleep
    /// clock accuracy - carried through unread by anything in this module,
    /// since nothing here needs it yet.
    pub sca: u8,
}

/// `CONNECT_IND`/`AUX_CONNECT_REQ`'s own fixed payload length: `InitA`(6)
/// + `AdvA`(6) + `LLData`(22), in bits.
pub const PAYLOAD_BITS: usize = 34 * 8;

fn byte_at(bits: &[bool], bit_offset: usize) -> u8 {
    let mut b = 0u8;
    for i in 0..8 {
        if bits[bit_offset + i] {
            b |= 1 << i;
        }
    }
    b
}

fn u16_at(bits: &[bool], byte_offset: usize) -> u16 {
    (byte_at(bits, byte_offset * 8) as u16) | ((byte_at(bits, (byte_offset + 1) * 8) as u16) << 8)
}

fn u24_at(bits: &[bool], byte_offset: usize) -> u32 {
    (0..3)
        .map(|i| (byte_at(bits, (byte_offset + i) * 8) as u32) << (8 * i))
        .fold(0, |a, b| a | b)
}

fn u32_at(bits: &[bool], byte_offset: usize) -> u32 {
    (0..4)
        .map(|i| (byte_at(bits, (byte_offset + i) * 8) as u32) << (8 * i))
        .fold(0, |a, b| a | b)
}

fn u40_at(bits: &[bool], byte_offset: usize) -> u64 {
    (0..5)
        .map(|i| (byte_at(bits, (byte_offset + i) * 8) as u64) << (8 * i))
        .fold(0, |a, b| a | b)
}

/// Decode a `CONNECT_IND`/`AUX_CONNECT_REQ`'s own payload - the bits
/// *after* the 16-bit advertising channel PDU header, in the same
/// dewhitened, transmission-order convention every other decode in this
/// arc uses.
///
/// `None` only when there are not even [`PAYLOAD_BITS`] of them - the
/// same "an incomplete PDU is not a wrong one" refusal `pdu::decode`
/// already holds itself to, not a claim that a full-length payload is
/// necessarily a genuine one.
pub fn decode(payload_bits: &[bool]) -> Option<ConnectIndData> {
    if payload_bits.len() < PAYLOAD_BITS {
        return None;
    }
    // Sent least significant octet first, held in the written order: see
    // `pdu::air_octets`.
    let mut init_a = [0u8; 6];
    for (i, slot) in init_a.iter_mut().enumerate() {
        *slot = byte_at(payload_bits, i * 8);
    }
    let init_a = super::pdu::air_octets(init_a);
    let mut adv_a = [0u8; 6];
    for (i, slot) in adv_a.iter_mut().enumerate() {
        *slot = byte_at(payload_bits, (6 + i) * 8);
    }
    let adv_a = super::pdu::air_octets(adv_a);
    let access_address = u32_at(payload_bits, 12);
    let crc_init = u24_at(payload_bits, 16);
    let win_size = byte_at(payload_bits, 19 * 8);
    let win_offset = u16_at(payload_bits, 20);
    let interval = u16_at(payload_bits, 22);
    let latency = u16_at(payload_bits, 24);
    let timeout = u16_at(payload_bits, 26);
    let channel_map = u40_at(payload_bits, 28) & 0x1F_FFFF_FFFF;
    let hop_sca = byte_at(payload_bits, 33 * 8);
    let hop_increment = hop_sca & 0x1F;
    let sca = (hop_sca >> 5) & 0x07;
    Some(ConnectIndData {
        init_a,
        adv_a,
        access_address,
        crc_init,
        win_size,
        win_offset,
        interval,
        latency,
        timeout,
        channel_map,
        hop_increment,
        sca,
    })
}

/// A `CONNECT_IND` payload as the packet ring keeps it, octets in the order
/// they were sent: [`decode`] over its bits, least significant first in each
/// octet, which is the order the air carried them.
pub fn decode_octets(payload: &[u8]) -> Option<ConnectIndData> {
    let bits: Vec<bool> = payload
        .iter()
        .flat_map(|b| (0..8).map(move |i| (b >> i) & 1 == 1))
        .collect();
    decode(&bits)
}

/// The central's worst-case sleep clock accuracy an SCA value states, as the
/// range in ppm, from Core 5.4 Vol 6 Part B Table 2.11, read on the SIG's
/// site 2026-09-23: 0 is 251 to 500 ppm, down to 7, 0 to 20 ppm.
pub fn sca_ppm(sca: u8) -> (u16, u16) {
    match sca & 0x07 {
        0 => (251, 500),
        1 => (151, 250),
        2 => (101, 150),
        3 => (76, 100),
        4 => (51, 75),
        5 => (31, 50),
        6 => (21, 30),
        _ => (0, 20),
    }
}

/// Channel Selection Algorithm #1's own persistent state - one
/// `lastUnmappedChannel`, carried from connection event to connection
/// event, exactly the cited text's own "shall be 0 for the first
/// connection event" through "when a connection event closes, the
/// `lastUnmappedChannel` shall be set to the value of the
/// `unmappedChannel`."
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Csa1 {
    last_unmapped: u8,
    hop_increment: u8,
    channel_map: u64,
    /// Every used channel, ascending - section 4.5.8.2's own "remapping
    /// table... built [from] all the used channels in ascending order,
    /// indexed from zero," computed once rather than rebuilt every
    /// connection event.
    used_channels: Vec<u8>,
}

impl Csa1 {
    /// `None` when `channel_map` names no used channel at all - not
    /// something a real transmitter is ever supposed to send (the cited
    /// text's own "minimum number of used channels shall be 2"), but a
    /// channel map this session did not itself confirm reached us intact,
    /// and dividing by zero used channels the first time an unmapped
    /// channel needs remapping would otherwise be this function's own
    /// silent way of finding out.
    pub fn new(hop_increment: u8, channel_map: u64) -> Option<Self> {
        let used_channels: Vec<u8> = (0u8..37)
            .filter(|&c| channel_map & (1u64 << c) != 0)
            .collect();
        if used_channels.is_empty() {
            return None;
        }
        Some(Self {
            last_unmapped: 0,
            hop_increment,
            channel_map,
            used_channels,
        })
    }

    /// Advance to the next connection event and return its data channel
    /// index - the first call is the first connection event's own
    /// channel, since [`Self::new`] starts `lastUnmappedChannel` at 0
    /// exactly as the cited text requires.
    ///
    /// **The algorithm, quoted rather than paraphrased.** "At the start
    /// of a connection event, `unmappedChannel` shall be calculated...:
    /// `unmappedChannel = (lastUnmappedChannel + hopIncrement) mod 37`.
    /// ...If the `unmappedChannel` is a used channel according to the
    /// channel map, Channel Selection Algorithm #1 shall use the
    /// `unmappedChannel` as the data channel index... If the
    /// `unmappedChannel` is an unused channel..., [it] shall be re-mapped
    /// to one of the used channels... using the following algorithm:
    /// `remappingIndex = unmappedChannel mod numUsedChannels`... The
    /// `remappingIndex` is then used to select the data channel index...
    /// from the remapping table."
    pub fn next(&mut self) -> u8 {
        let unmapped = ((self.last_unmapped as u16 + self.hop_increment as u16) % 37) as u8;
        self.last_unmapped = unmapped;
        if self.channel_map & (1u64 << unmapped) != 0 {
            unmapped
        } else {
            let remapping_index = (unmapped as usize) % self.used_channels.len();
            self.used_channels[remapping_index]
        }
    }
}

/// Channel Selection Algorithm #2 (Core 5.4 Vol 6 Part B 4.5.8.3), for the
/// event channel of an ACL connection: net-ux-polish-plan 5.4.b4, scoped out
/// of B20 and added at Viktor's request.
///
/// **What was read, and what was reconstructed.** Read from the text on the
/// SIG's site, 2026-09-23: `channelIdentifier = (Access Address 31-16) XOR
/// (Access Address 15-0)`; the permutation "separately bit-reversing the
/// lower 8 input bits and upper 8 input bits"; MAM's `output = (17 x a + b)
/// mod 2^16`; `unmappedChannel = prn_e mod 37`; a used unmapped channel is
/// the event's, an unused one is remapped by `remappingIndex = floor(N x
/// prn_e / 2^16)` into the ascending table of used channels. **Not text:**
/// Figure 4.53, the order the event PRN is built in, is an image. The order
/// here is the one public descriptions give (counter XOR the identifier,
/// three rounds of permute-then-MAM with the identifier, XOR the identifier
/// again), and it is **held to the specification's own sample data** (Vol 6
/// Part C 3, both tables, read the same day): every `prn_e`, unmapped and
/// mapped channel there, for 37 and for 9 used channels. That is what makes
/// the reconstruction trustworthy rather than plausible.
///
/// Subevent selection (isochronous, PAwR) is left out: nothing here has
/// subevents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Csa2 {
    channel_id: u16,
    channel_map: u64,
    used_channels: Vec<u8>,
}

impl Csa2 {
    /// `None` when the map names no used channel, for [`Csa1::new`]'s reason.
    pub fn new(access_address: u32, channel_map: u64) -> Option<Self> {
        let used_channels: Vec<u8> = (0u8..37)
            .filter(|&c| channel_map & (1u64 << c) != 0)
            .collect();
        if used_channels.is_empty() {
            return None;
        }
        Some(Self {
            channel_id: ((access_address >> 16) as u16) ^ (access_address as u16),
            channel_map,
            used_channels,
        })
    }

    fn perm(x: u16) -> u16 {
        let lo = (x as u8).reverse_bits() as u16;
        let hi = ((x >> 8) as u8).reverse_bits() as u16;
        (hi << 8) | lo
    }

    fn mam(a: u16, b: u16) -> u16 {
        (17u32 * a as u32 + b as u32) as u16
    }

    /// The event pseudo-random number for connection event `counter`.
    pub fn prn_e(&self, counter: u16) -> u16 {
        let id = self.channel_id;
        let mut x = counter ^ id;
        for _ in 0..3 {
            x = Self::mam(Self::perm(x), id);
        }
        x ^ id
    }

    /// Connection event `counter`'s data channel index, and its unmapped
    /// channel before any remapping.
    pub fn channel(&self, counter: u16) -> (u8, u8) {
        let prn = self.prn_e(counter);
        let unmapped = (prn % 37) as u8;
        if self.channel_map & (1u64 << unmapped) != 0 {
            (unmapped, unmapped)
        } else {
            let n = self.used_channels.len() as u32;
            let index = (n * prn as u32) >> 16;
            (self.used_channels[index as usize], unmapped)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic `CONNECT_IND` payload's own bits directly, LSB-
    /// first per byte and least-significant-octet-first across a
    /// multi-byte field - this arc's own established transmission-order
    /// convention throughout.
    fn synthetic_payload(data: &ConnectIndData) -> Vec<bool> {
        let mut bytes = Vec::with_capacity(34);
        bytes.extend_from_slice(&super::super::pdu::air_octets(data.init_a));
        bytes.extend_from_slice(&super::super::pdu::air_octets(data.adv_a));
        bytes.extend_from_slice(&data.access_address.to_le_bytes());
        bytes.extend_from_slice(&data.crc_init.to_le_bytes()[..3]);
        bytes.push(data.win_size);
        bytes.extend_from_slice(&data.win_offset.to_le_bytes());
        bytes.extend_from_slice(&data.interval.to_le_bytes());
        bytes.extend_from_slice(&data.latency.to_le_bytes());
        bytes.extend_from_slice(&data.timeout.to_le_bytes());
        bytes.extend_from_slice(&data.channel_map.to_le_bytes()[..5]);
        bytes.push((data.sca << 5) | (data.hop_increment & 0x1F));
        assert_eq!(bytes.len(), 34);
        bytes
            .iter()
            .flat_map(|&byte| (0..8).map(move |i| (byte >> i) & 1 != 0))
            .collect()
    }

    /// A round trip through [`decode`] recovers every field exactly.
    #[test]
    fn decode_recovers_every_field() {
        let data = ConnectIndData {
            init_a: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            adv_a: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            access_address: 0x8E4C_2A17,
            crc_init: 0x00_5A_C3,
            win_size: 4,
            win_offset: 10,
            interval: 40, // 50 ms
            latency: 0,
            timeout: 200,                     // 2 s
            channel_map: 0x00_00_1F_FF_FF_FF, // low 37 bits set except the top few, see below
            hop_increment: 11,
            sca: 5,
        };
        let bits = synthetic_payload(&data);
        let decoded = decode(&bits).expect("should decode");
        assert_eq!(decoded, data);
    }

    /// Too few bits is refused, not padded or read past the end.
    #[test]
    fn too_few_bits_is_refused() {
        assert!(decode(&vec![false; PAYLOAD_BITS - 1]).is_none());
    }

    /// Reserved bits 37 to 39 of the channel map are always read as unset,
    /// even when the raw bytes set them - the cited text's own "reserved
    /// for future use."
    #[test]
    fn reserved_channel_map_bits_are_masked_off() {
        let data = ConnectIndData {
            init_a: [0; 6],
            adv_a: [0; 6],
            access_address: 0x1234_5678,
            crc_init: 0x11_2233,
            win_size: 2,
            win_offset: 3,
            interval: 40,
            latency: 0,
            timeout: 100,
            channel_map: 0xFF_FF_FF_FF_FF, // every one of the 40 raw bits set
            hop_increment: 9,
            sca: 0,
        };
        let bits = synthetic_payload(&data);
        let decoded = decode(&bits).expect("should decode");
        // 37 bits set (0..36), bits 37-39 masked off.
        assert_eq!(decoded.channel_map, 0x1F_FF_FF_FF_FF);
    }

    /// Channel Selection Algorithm #1's own first connection event: the
    /// cited text's "`lastUnmappedChannel` shall be 0 for the first
    /// connection event," worked by hand - `hopIncrement` 7 gives
    /// `unmappedChannel = (0 + 7) mod 37 = 7`, and with every channel
    /// used, channel 7 needs no remapping.
    #[test]
    fn the_first_connection_event_matches_a_hand_worked_example() {
        let all_used = (0u64..37).fold(0u64, |acc, c| acc | (1 << c));
        let mut csa = Csa1::new(7, all_used).unwrap();
        assert_eq!(csa.next(), 7);
        // Second event: (7 + 7) mod 37 = 14.
        assert_eq!(csa.next(), 14);
    }

    /// A hand-worked remapping example: `hopIncrement` 40 wraps past 37
    /// immediately (`unmappedChannel = 40 mod 37 = 3`), and with channel 3
    /// excluded from the map, section 4.5.8.2's own remapping runs:
    /// `remappingIndex = 3 mod numUsedChannels`.
    #[test]
    fn remapping_an_unused_channel_matches_a_hand_worked_example() {
        // Channels 0..37 used, except channel 3 - 36 used channels, in
        // ascending order 0,1,2,4,5,...,36 (channel 3 skipped).
        let channel_map = ((0u64..37).fold(0u64, |acc, c| acc | (1 << c))) & !(1 << 3);
        let mut csa = Csa1::new(40, channel_map).unwrap();
        // unmappedChannel = (0 + 40) mod 37 = 3, which is unused.
        // remappingIndex = 3 mod 36 = 3. The remapping table, ascending,
        // is [0, 1, 2, 4, 5, ...] - index 3 is channel 4.
        assert_eq!(csa.next(), 4);
    }

    /// The actual guarantee the whole remapping step exists for: across
    /// many connection events, on a channel map that excludes several
    /// channels, the data channel index returned is never one of the
    /// excluded ones - checked over enough events to cycle through the
    /// full 37-wide unmapped-channel space several times, not asserted
    /// from the formula alone.
    #[test]
    fn an_excluded_channel_is_never_returned() {
        let excluded: [u8; 4] = [3, 17, 22, 36];
        let mut channel_map = (0u64..37).fold(0u64, |acc, c| acc | (1 << c));
        for &c in &excluded {
            channel_map &= !(1u64 << c);
        }
        let mut csa = Csa1::new(11, channel_map).unwrap();
        for _ in 0..500 {
            let ch = csa.next();
            assert!(
                !excluded.contains(&ch),
                "excluded channel {ch} was returned"
            );
            assert!(ch < 37);
        }
    }

    /// A channel map naming no used channel at all is refused, not a
    /// panic waiting to happen the first time an unmapped channel needs
    /// remapping against zero candidates.
    #[test]
    fn an_empty_channel_map_is_refused() {
        assert!(Csa1::new(11, 0).is_none());
    }

    /// **The octets the ring keeps decode the same as the bits the air
    /// carried**: built bit by bit here, least significant first, and read
    /// back through both entry points.
    #[test]
    fn octets_and_bits_decode_alike() {
        let payload: Vec<u8> = (0u8..34)
            .map(|i| i.wrapping_mul(37).wrapping_add(11))
            .collect();
        let bits: Vec<bool> = payload
            .iter()
            .flat_map(|b| (0..8).map(move |i| (b >> i) & 1 == 1))
            .collect();
        assert_eq!(decode_octets(&payload), decode(&bits));
        assert!(decode_octets(&payload).is_some());
        assert_eq!(decode_octets(&payload[..33]), None, "one octet short");
    }

    /// Table 2.11's eight rows, the ends of the scale checked.
    #[test]
    fn sca_reads_as_table_2_11_states() {
        assert_eq!(sca_ppm(0), (251, 500));
        assert_eq!(sca_ppm(3), (76, 100));
        assert_eq!(sca_ppm(7), (0, 20));
    }

    /// **The specification's own sample data, Vol 6 Part C 3.1**: Access
    /// Address 0x8E89BED6 (channelIdentifier 0x305F), all 37 channels used,
    /// events 0 to 3.
    #[test]
    fn csa2_matches_sample_data_1() {
        let csa = Csa2::new(0x8E89_BED6, 0x1F_FFFF_FFFF).unwrap();
        assert_eq!(csa.channel_id, 0x305F);
        let prn: Vec<u16> = (0..4).map(|c| csa.prn_e(c)).collect();
        assert_eq!(prn, [56857, 1685, 38301, 27475]);
        let ch: Vec<(u8, u8)> = (0..4).map(|c| csa.channel(c)).collect();
        assert_eq!(ch, [(25, 25), (20, 20), (6, 6), (21, 21)]);
    }

    /// **Sample data 2, Vol 6 Part C 3.2**: nine used channels, so the
    /// remapping is exercised: events 6 to 8 unmap to 23, 14, 17 and map to
    /// 23, 9, 34.
    #[test]
    fn csa2_matches_sample_data_2() {
        let map = 0b11110_00000000_11100000_00000110_00000000u64;
        let csa = Csa2::new(0x8E89_BED6, map).unwrap();
        assert_eq!(csa.used_channels, [9, 10, 21, 22, 23, 33, 34, 35, 36]);
        let prn: Vec<u16> = (6..9).map(|c| csa.prn_e(c)).collect();
        assert_eq!(prn, [10975, 5490, 46970]);
        let ch: Vec<(u8, u8)> = (6..9).map(|c| csa.channel(c)).collect();
        assert_eq!(ch, [(23, 23), (9, 14), (34, 17)]);
    }
}
