// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The link manager's messages: what two classic devices say to each other
//! to set a link up, read from the payloads that carry them.
//!
//! **Where one rides.** A link manager message is the body of a payload
//! whose LLID is 0b11 (Core 5.4 Vol 2 Part C 2.3): "LMP messages shall be
//! transmitted using DM1 packets, however if an HV1 SCO link is in use and
//! the length of the payload is no greater than 9 bytes then DV packets may
//! be used." The body's first byte holds the transaction ID in its least
//! significant bit and the first 7 bits of the opcode above it; "if the
//! initial 7 bits of the opcode have one of the special escape values 124
//! to 127 then an additional byte of opcode is located in the second byte
//! of the payload". The parameters follow.
//!
//! **Who began it is not who sent it.** The transaction ID "shall be 0 if
//! the PDU forms part of a transaction that was initiated by the Central
//! and 1 if the transaction was initiated by the Peripheral" (Part C 2.4).
//! The packet's own direction, from its CLK1 parity, says who sent this
//! one: an `accepted` the Peripheral sends in the Central's transaction
//! carries a 0.
//!
//! **The table is the Core's, transcribed once.** [`PDUS`] is Part C Table
//! 5.1, "Coding of the different LM PDUs": opcode, name (lower-cased, the
//! `LMP_` dropped), length in bytes with the opcode, packet type and the
//! possible direction. A test holds it to the table's own rules (every
//! opcode once, every length a DM1 body can carry, no retired opcode, every
//! escape one of 124 to 127), so a slip in the transcription that breaks
//! one of them fails the build rather than misnaming a message on screen.
//!
//! What the table cannot vouch for is said, not smoothed over: an opcode it
//! does not list, a body longer or shorter than it gives, and a message seen
//! going the way it forbids are each named in its own words.

use super::piconet::Direction;

/// An LMP opcode: seven bits, or an escape (124 to 127) and the byte after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opcode {
    Short(u8),
    Escaped(u8, u8),
}

/// Table 5.1's possible direction: `C ↔ P`, `C → P`, `C ← P`, or `B`, the
/// broadcast link, which only the Central sends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allowed {
    Both,
    ToPeripheral,
    ToCentral,
    Broadcast,
}

/// One row of Table 5.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pdu {
    pub opcode: Opcode,
    pub name: &'static str,
    /// Bytes, the opcode included.
    pub length: u8,
    /// DM1 only, or DM1 and DV.
    pub dm1_only: bool,
    pub direction: Allowed,
}

/// "The following opcodes were previously used: 22, 25 to 30" (Part C 5.1).
pub const PREVIOUSLY_USED: [u8; 7] = [22, 25, 26, 27, 28, 29, 30];

/// Who began the transaction a message belongs to: its transaction ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initiator {
    Central,
    Peripheral,
}

/// One link manager message, read from a payload body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LmpMessage {
    pub initiator: Initiator,
    pub opcode: Opcode,
    pub pdu: Option<Pdu>,
    /// The body's length in bytes, opcode included.
    pub length: usize,
    /// The parameters in words, or in hex.
    pub params: String,
    /// An escape whose second byte the body does not reach: the opcode is
    /// only half known.
    escape_cut: bool,
}

use Allowed::*;
use Opcode::*;

const DM1: bool = true;
const DM1_DV: bool = false;

const fn pdu(
    opcode: Opcode,
    name: &'static str,
    length: u8,
    dm1_only: bool,
    direction: Allowed,
) -> Pdu {
    Pdu {
        opcode,
        name,
        length,
        dm1_only,
        direction,
    }
}

/// Core 5.4 Vol 2 Part C Table 5.1, "Coding of the different LM PDUs":
/// opcode, name, length in bytes with the opcode, DM1 only or DM1/DV, and
/// the possible direction (`B` is the broadcast link, the Central's).
pub const PDUS: &[Pdu] = &[
    pdu(Short(3), "accepted", 2, DM1_DV, Both),
    pdu(Escaped(127, 1), "accepted_ext", 4, DM1, Both),
    pdu(Short(11), "au_rand", 17, DM1, Both),
    pdu(Short(35), "auto_rate", 1, DM1_DV, Both),
    pdu(
        Escaped(127, 17),
        "channel_classification",
        12,
        DM1,
        ToCentral,
    ),
    pdu(
        Escaped(127, 16),
        "channel_classification_req",
        7,
        DM1,
        ToPeripheral,
    ),
    pdu(Escaped(127, 5), "clk_adj", 15, DM1, Broadcast),
    pdu(Escaped(127, 6), "clk_adj_ack", 3, DM1, ToCentral),
    pdu(Escaped(127, 7), "clk_adj_req", 6, DM1, ToCentral),
    pdu(Short(5), "clkoffset_req", 1, DM1_DV, ToPeripheral),
    pdu(Short(6), "clkoffset_res", 3, DM1_DV, ToCentral),
    pdu(Short(9), "comb_key", 17, DM1, Both),
    pdu(Short(32), "decr_power_req", 2, DM1_DV, Both),
    pdu(Short(7), "detach", 2, DM1_DV, Both),
    pdu(Short(65), "dhkey_check", 17, DM1, Both),
    pdu(Short(61), "encapsulated_header", 4, DM1, Both),
    pdu(Short(62), "encapsulated_payload", 17, DM1, Both),
    pdu(
        Short(58),
        "encryption_key_size_mask_req",
        1,
        DM1,
        ToPeripheral,
    ),
    pdu(Short(59), "encryption_key_size_mask_res", 3, DM1, ToCentral),
    pdu(Short(16), "encryption_key_size_req", 2, DM1_DV, Both),
    pdu(Short(15), "encryption_mode_req", 2, DM1_DV, Both),
    pdu(Escaped(127, 12), "esco_link_req", 16, DM1, Both),
    pdu(Short(39), "features_req", 9, DM1_DV, Both),
    pdu(Escaped(127, 3), "features_req_ext", 12, DM1, Both),
    pdu(Short(40), "features_res", 9, DM1_DV, Both),
    pdu(Escaped(127, 4), "features_res_ext", 12, DM1, Both),
    pdu(Short(20), "hold", 7, DM1_DV, Both),
    pdu(Short(21), "hold_req", 7, DM1_DV, Both),
    pdu(Short(51), "host_connection_req", 1, DM1_DV, Both),
    pdu(Short(8), "in_rand", 17, DM1, Both),
    pdu(Short(31), "incr_power_req", 2, DM1_DV, Both),
    pdu(Escaped(127, 25), "io_capability_req", 5, DM1, Both),
    pdu(Escaped(127, 26), "io_capability_res", 5, DM1, Both),
    pdu(Escaped(127, 30), "keypress_notification", 3, DM1, Both),
    pdu(Short(33), "max_power", 1, DM1_DV, Both),
    pdu(Short(45), "max_slot", 2, DM1_DV, Both),
    pdu(Short(46), "max_slot_req", 2, DM1_DV, Both),
    pdu(Short(34), "min_power", 1, DM1_DV, Both),
    pdu(Short(1), "name_req", 2, DM1_DV, Both),
    pdu(Short(2), "name_res", 17, DM1, Both),
    pdu(Short(4), "not_accepted", 3, DM1_DV, Both),
    pdu(Escaped(127, 2), "not_accepted_ext", 5, DM1, Both),
    pdu(Escaped(127, 27), "numeric_comparison_failed", 2, DM1, Both),
    pdu(Escaped(127, 29), "oob_failed", 2, DM1, Both),
    pdu(Escaped(127, 11), "packet_type_table_req", 3, DM1, Both),
    pdu(Short(53), "page_mode_req", 3, DM1_DV, Both),
    pdu(Short(54), "page_scan_mode_req", 3, DM1_DV, Both),
    pdu(Escaped(127, 28), "passkey_failed", 2, DM1, Both),
    pdu(Short(66), "pause_encryption_aes_req", 17, DM1, Both),
    pdu(Escaped(127, 23), "pause_encryption_req", 2, DM1, Both),
    pdu(Escaped(127, 33), "ping_req", 2, DM1, Both),
    pdu(Escaped(127, 34), "ping_res", 2, DM1, Both),
    pdu(Escaped(127, 31), "power_control_req", 3, DM1_DV, Both),
    pdu(Escaped(127, 32), "power_control_res", 3, DM1_DV, Both),
    pdu(Short(36), "preferred_rate", 2, DM1_DV, Both),
    pdu(Short(41), "quality_of_service", 4, DM1_DV, ToPeripheral),
    pdu(Short(42), "quality_of_service_req", 4, DM1_DV, Both),
    pdu(Escaped(127, 13), "remove_esco_link_req", 4, DM1, Both),
    pdu(Short(44), "remove_sco_link_req", 3, DM1_DV, Both),
    pdu(Escaped(127, 24), "resume_encryption_req", 2, DM1, ToCentral),
    pdu(Escaped(127, 36), "sam_define_map", 17, DM1, Both),
    pdu(Escaped(127, 35), "sam_set_type0", 17, DM1, Both),
    pdu(Escaped(127, 37), "sam_switch", 9, DM1, Both),
    pdu(Short(43), "sco_link_req", 7, DM1_DV, Both),
    pdu(Short(60), "set_afh", 16, DM1, ToPeripheral),
    pdu(Short(49), "setup_complete", 1, DM1, Both),
    pdu(Short(63), "simple_pairing_confirm", 17, DM1, Both),
    pdu(Short(64), "simple_pairing_number", 17, DM1, Both),
    pdu(Short(52), "slot_offset", 9, DM1_DV, Both),
    pdu(Short(23), "sniff_req", 10, DM1, Both),
    pdu(Escaped(127, 21), "sniff_subrating_req", 9, DM1, Both),
    pdu(Escaped(127, 22), "sniff_subrating_res", 9, DM1, Both),
    pdu(Short(12), "sres", 5, DM1_DV, Both),
    pdu(Short(17), "start_encryption_req", 17, DM1, ToPeripheral),
    pdu(Short(18), "stop_encryption_req", 1, DM1_DV, ToPeripheral),
    pdu(Short(55), "supervision_timeout", 3, DM1_DV, ToPeripheral),
    pdu(Short(19), "switch_req", 5, DM1, Both),
    pdu(Short(14), "temp_key", 17, DM1, ToPeripheral),
    pdu(Short(13), "temp_rand", 17, DM1, ToPeripheral),
    pdu(Short(56), "test_activate", 1, DM1_DV, ToPeripheral),
    pdu(Short(57), "test_control", 10, DM1, ToPeripheral),
    pdu(Short(47), "timing_accuracy_req", 1, DM1_DV, Both),
    pdu(Short(48), "timing_accuracy_res", 3, DM1_DV, Both),
    pdu(Short(10), "unit_key", 17, DM1, Both),
    pdu(Short(24), "unsniff_req", 1, DM1_DV, Both),
    pdu(Short(50), "use_semi_permanent_key", 1, DM1_DV, ToPeripheral),
    pdu(Short(37), "version_req", 6, DM1_DV, Both),
    pdu(Short(38), "version_res", 6, DM1_DV, Both),
];

/// Table 5.1's row for an opcode, if it has one.
pub fn lookup(opcode: Opcode) -> Option<Pdu> {
    PDUS.iter().find(|p| p.opcode == opcode).copied()
}

/// Read a link manager message from a payload body. `None` only for an
/// empty body, which holds no opcode at all.
pub fn parse(body: &[u8]) -> Option<LmpMessage> {
    let first = *body.first()?;
    let initiator = if first & 1 == 0 {
        Initiator::Central
    } else {
        Initiator::Peripheral
    };
    let short = first >> 1;
    let (opcode, start) = if (124..=127).contains(&short) {
        match body.get(1) {
            Some(&ext) => (Opcode::Escaped(short, ext), 2),
            None => {
                return Some(LmpMessage {
                    initiator,
                    opcode: Opcode::Escaped(short, 0),
                    pdu: None,
                    length: body.len(),
                    params: String::new(),
                    escape_cut: true,
                });
            }
        }
    } else {
        (Opcode::Short(short), 1)
    };
    let pdu = lookup(opcode);
    // As far as both the bytes and the table allow.
    let end = pdu.map_or(body.len(), |p| body.len().min(p.length as usize));
    let params = hex(&body[start.min(end)..end]);
    Some(LmpMessage {
        initiator,
        opcode,
        pdu,
        length: body.len(),
        params,
        escape_cut: false,
    })
}

/// Bytes as `bf fe cf`, nothing for none.
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl LmpMessage {
    /// The PDU's name, or what can be said of an opcode Table 5.1 does not
    /// name.
    pub fn name(&self) -> String {
        if let Some(p) = self.pdu {
            return p.name.to_string();
        }
        match self.opcode {
            Opcode::Escaped(e, _) if self.escape_cut => format!("opcode {e}/?: cut short"),
            Opcode::Short(n) if PREVIOUSLY_USED.contains(&n) => {
                format!("opcode {n}: previously used (Table 5.1)")
            }
            Opcode::Short(n) => format!("opcode {n}: not in Table 5.1"),
            Opcode::Escaped(e, x) => format!("opcode {e}/{x}: not in Table 5.1"),
        }
    }

    /// The body's length beside Table 5.1's, when the two differ.
    pub fn length_note(&self) -> Option<String> {
        let p = self.pdu?;
        (self.length != p.length as usize)
            .then(|| format!("length {}, Table 5.1: {}", self.length, p.length))
    }

    /// Table 5.1's words when the message was sent the way the table does
    /// not allow it, `sender` being the packet's own direction.
    pub fn against(&self, sender: Direction) -> Option<&'static str> {
        match (self.pdu?.direction, sender) {
            (ToPeripheral, Direction::Slave) => Some("Table 5.1: C → P only"),
            (Broadcast, Direction::Slave) => Some("Table 5.1: broadcast, the Central's"),
            (ToCentral, Direction::Master) => Some("Table 5.1: C ← P only"),
            _ => None,
        }
    }

    /// One line: the name, the parameters, and the length note if any.
    pub fn words(&self) -> String {
        let mut out = self.name();
        if !self.params.is_empty() {
            out.push_str("  ");
            out.push_str(&self.params);
        }
        if let Some(note) = self.length_note() {
            out.push_str(&format!("  ({note})"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table keeps the Core's own rules: every opcode once, every
    /// length 1 to 17 (a DM1 body), no opcode the Core retired, every
    /// escape one of 124 to 127, and all 88 PDUs of Table 5.1.
    #[test]
    fn the_table_keeps_the_cores_rules() {
        assert_eq!(PDUS.len(), 88);
        for (i, a) in PDUS.iter().enumerate() {
            assert!((1..=17).contains(&a.length), "{}", a.name);
            match a.opcode {
                Opcode::Short(op) => {
                    assert!(op < 124, "{}", a.name);
                    assert!(!PREVIOUSLY_USED.contains(&op), "{}", a.name);
                }
                Opcode::Escaped(e, _) => assert!((124..=127).contains(&e), "{}", a.name),
            }
            for b in &PDUS[i + 1..] {
                assert_ne!(a.opcode, b.opcode, "{} and {}", a.name, b.name);
                assert_ne!(a.name, b.name);
            }
        }
    }

    /// The first byte: the transaction ID in bit 0, the opcode above it
    /// (Part C 2.3, 2.4); an escape reads the second byte too.
    #[test]
    fn the_first_byte_is_the_tid_and_the_opcode() {
        // features_req (39) in a transaction the Central began.
        let m = parse(&[39 << 1, 0xbf, 0xfe, 0xcf, 0xfe, 0xdb, 0xff, 0x7b, 0x87]).unwrap();
        assert_eq!(m.initiator, Initiator::Central);
        assert_eq!(m.opcode, Opcode::Short(39));
        assert_eq!(m.name(), "features_req");
        assert_eq!(m.length_note(), None);
        // accepted_ext (127/1) in the Peripheral's.
        let m = parse(&[127 << 1 | 1, 1, 127, 11]).unwrap();
        assert_eq!(m.initiator, Initiator::Peripheral);
        assert_eq!(m.opcode, Opcode::Escaped(127, 1));
        assert_eq!(m.name(), "accepted_ext");
    }

    /// An opcode outside the table is named as such, a retired one as
    /// retired, and an empty body is no message.
    #[test]
    fn an_opcode_outside_the_table_says_so() {
        // 67: the first short opcode after the table's last, 66.
        assert_eq!(
            parse(&[67 << 1]).unwrap().name(),
            "opcode 67: not in Table 5.1"
        );
        assert_eq!(
            parse(&[22 << 1]).unwrap().name(),
            "opcode 22: previously used (Table 5.1)"
        );
        assert_eq!(
            parse(&[127 << 1, 99]).unwrap().name(),
            "opcode 127/99: not in Table 5.1"
        );
        assert_eq!(parse(&[]), None);
        // An escape with no second byte.
        assert_eq!(
            parse(&[127 << 1]).unwrap().name(),
            "opcode 127/?: cut short"
        );
    }

    /// A body longer or shorter than Table 5.1 gives says both.
    #[test]
    fn a_length_the_table_disagrees_with_is_said() {
        let m = parse(&[3 << 1]).unwrap(); // accepted, 2 in the table
        assert_eq!(m.length_note().as_deref(), Some("length 1, Table 5.1: 2"));
        let m = parse(&[3 << 1, 15, 0]).unwrap();
        assert_eq!(m.length_note().as_deref(), Some("length 3, Table 5.1: 2"));
    }

    /// A PDU the table allows one way only, seen the other way by its
    /// CLK1 parity, is named with the table's words; both ways, never.
    #[test]
    fn a_direction_the_table_forbids_is_named() {
        use crate::signal::bt::piconet::Direction;
        let start = parse(&[17 << 1; 17]).unwrap(); // start_encryption_req, C -> P
        assert_eq!(start.against(Direction::Master), None);
        assert_eq!(
            start.against(Direction::Slave),
            Some("Table 5.1: C → P only")
        );
        let res = parse(&[6 << 1, 0, 0]).unwrap(); // clkoffset_res, C <- P
        assert_eq!(
            res.against(Direction::Master),
            Some("Table 5.1: C ← P only")
        );
        let acc = parse(&[3 << 1, 15]).unwrap();
        assert_eq!(acc.against(Direction::Slave), None);
    }

    /// One line: the name, two spaces, the parameters, then the length note
    /// in brackets; an empty part leaves no gap.
    #[test]
    fn the_words_are_name_params_and_note() {
        assert_eq!(parse(&[3 << 1, 15]).unwrap().words(), "accepted  0f");
        assert_eq!(parse(&[47 << 1]).unwrap().words(), "timing_accuracy_req");
        assert_eq!(
            parse(&[3 << 1]).unwrap().words(),
            "accepted  (length 1, Table 5.1: 2)"
        );
    }
}
