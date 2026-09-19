// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What kind of address a BLE device is advertising with.
//!
//! Read from the Core specification's own text, 5.4 Vol 6 Part B, before a
//! line of this was written:
//!
//! - 2.3.1.1: "The TxAdd in the advertising physical channel PDU header
//!   indicates whether the advertiser's address in the AdvA field is public
//!   (TxAdd = 0) or random (TxAdd = 1)."
//! - 1.3.2: "The specific sub-type is indicated by the two most significant
//!   bits of the random device address as shown in Table 1.2", which reads
//!   `Address [47:46]`: `0b00` non-resolvable private, `0b01` resolvable
//!   private, `0b10` reserved for future use, `0b11` static device address.
//!
//! **What this does not claim.** The specification also puts obligations on
//! the random part (1.3.2.1, 1.3.2.2: at least one bit 0 and one bit 1; a
//! non-resolvable address "shall not be equal to the public address"). Those
//! are the sender's to keep, and the last cannot be checked by a receiver at
//! all, so this reports what the two bits say and nothing more. Whether a
//! resolvable address belongs to a device we have seen before needs its IRK,
//! which a passive receiver does not have: no key is needed here and none is
//! claimed (POLICY rule 1).
//!
//! The address is in the written order, most significant octet first
//! (`super::pdu::air_octets`), so bits `[47:46]` are the top of `addr[0]`.

/// An advertiser's address kind: public, or which sub-type of random.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressKind {
    /// TxAdd = 0: an IEEE-assigned address, whose top three octets are an
    /// OUI.
    Public,
    /// `0b11`: random, fixed for at least a power cycle.
    Static,
    /// `0b01`: random, generated from an IRK; the device's own peers can
    /// resolve it, nobody else can.
    ResolvablePrivate,
    /// `0b00`: random, and meant to be linked to nothing.
    NonResolvablePrivate,
    /// `0b10`: a sub-type the specification reserves. No compliant device
    /// sends it, so seeing one says the packet or the device is not what it
    /// claims, and it is shown as that rather than forced into a kind.
    Reserved,
}

impl AddressKind {
    /// The short form a table column has room for.
    pub fn label(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Static => "static",
            Self::ResolvablePrivate => "RPA",
            Self::NonResolvablePrivate => "NRPA",
            Self::Reserved => "reserved",
        }
    }
}

/// The kind of `addr`, sent with TxAdd = `random`.
pub fn kind(addr: [u8; 6], random: bool) -> AddressKind {
    if !random {
        return AddressKind::Public;
    }
    match addr[0] >> 6 {
        0b11 => AddressKind::Static,
        0b01 => AddressKind::ResolvablePrivate,
        0b00 => AddressKind::NonResolvablePrivate,
        _ => AddressKind::Reserved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table 1.2, row by row, on hand-built addresses whose top octet
    /// carries each sub-type and whose other bits are deliberately the
    /// opposite pattern, so a function reading the wrong octet or the wrong
    /// end of it cannot pass.
    #[test]
    fn each_sub_type_is_the_two_top_bits_table_1_2_names() {
        let with_top = |top: u8| [top, 0xff, 0x00, 0xff, 0x00, 0x3f];
        assert_eq!(kind(with_top(0b1100_0000), true), AddressKind::Static);
        assert_eq!(
            kind(with_top(0b0100_0000), true),
            AddressKind::ResolvablePrivate
        );
        assert_eq!(
            kind(with_top(0b0000_0000), true),
            AddressKind::NonResolvablePrivate
        );
        assert_eq!(kind(with_top(0b1000_0000), true), AddressKind::Reserved);
        // Only the top two bits decide; the rest of the octet is the random
        // part.
        assert_eq!(kind(with_top(0b1111_1111), true), AddressKind::Static);
        assert_eq!(
            kind(with_top(0b0011_1111), true),
            AddressKind::NonResolvablePrivate
        );
    }

    /// TxAdd = 0 is public whatever the bits, because a public address's top
    /// octet is part of an OUI and says nothing about sub-types.
    #[test]
    fn a_public_address_is_public_whatever_its_bits() {
        for top in [0x00, 0x40, 0x80, 0xc0] {
            assert_eq!(kind([top, 1, 2, 3, 4, 5], false), AddressKind::Public);
        }
    }

    /// **A real advertiser, from the air.** The ADV_NONCONN_IND fixture in
    /// `pdu`'s tests was sent with TxAdd = 1 and AdvA `d1:9a:7e:91:27:9e` in
    /// the written order. Its top bits are `11`, a static address. Read in
    /// the order the octets arrive, the same address would start `0x9e`,
    /// `10`, the one sub-type no compliant device sends: the real packet
    /// settles the octet order as well as the kind.
    #[test]
    fn a_real_random_address_reads_as_a_sub_type_that_exists() {
        let addr = [0xd1, 0x9a, 0x7e, 0x91, 0x27, 0x9e];
        assert_eq!(kind(addr, true), AddressKind::Static);
        assert_eq!(
            kind(super::super::pdu::air_octets(addr), true),
            AddressKind::Reserved
        );
    }
}
