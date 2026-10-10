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
use crate::signal::assigned;
use crate::signal::errors::error_name;

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
    let p = &body[start.min(end)..end];
    let params = pdu.map_or_else(|| hex(p), |pdu| params_words(&pdu, p));
    Some(LmpMessage {
        initiator,
        opcode,
        pdu,
        length: body.len(),
        params,
        escape_cut: false,
    })
}

/// The words a parameter list is built from, in the order shown. A
/// parameter the body does not reach ends the list with `cut short`: what
/// came before it is said, nothing after it is guessed.
struct Words {
    parts: Vec<String>,
    cut: bool,
}

impl Words {
    fn new() -> Self {
        Words {
            parts: Vec::new(),
            cut: false,
        }
    }

    fn push(&mut self, part: Option<String>) {
        if self.cut {
            return;
        }
        match part {
            Some(p) => self.parts.push(p),
            None => self.cut = true,
        }
    }

    fn done(mut self) -> String {
        if self.cut {
            self.parts.push("cut short".into());
        }
        self.parts.join(" · ")
    }
}

/// Parameters are little-endian, "Values shall be stored in little-endian
/// order" (Core 5.4 Vol 1 Part E 2.9).
fn u16le(p: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*p.get(at)?, *p.get(at + 1)?]))
}

fn u32le(p: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(p.get(at..at + 4)?.try_into().ok()?))
}

/// A PDU named by an opcode parameter, as an answer names what it answers.
fn opcode_name(opcode: Opcode) -> String {
    match (lookup(opcode), opcode) {
        (Some(p), _) => p.name.to_string(),
        (None, Opcode::Short(n)) => format!("opcode {n}"),
        (None, Opcode::Escaped(e, x)) => format!("opcode {e}/{x}"),
    }
}

/// What a body that carries key material holds, by name: the bench
/// measures a link, it does not collect what a key search needs.
fn key_material(name: &str) -> Option<&'static str> {
    Some(match name {
        "au_rand"
        | "in_rand"
        | "comb_key"
        | "temp_rand"
        | "start_encryption_req"
        | "pause_encryption_aes_req" => "16-byte random number",
        "unit_key" | "temp_key" => "16-byte key",
        "sres" => "4-byte authentication response",
        "simple_pairing_confirm" => "16-byte commitment",
        "simple_pairing_number" => "16-byte nonce",
        "dhkey_check" => "16-byte confirmation",
        "encapsulated_payload" => "16 bytes of encapsulated data",
        _ => return None,
    })
}

/// A PDU's parameters in words, from Core 5.4 Vol 2 Part C 5.2; those it
/// has no words for, in hex. `p` is the body after the opcode, cut to
/// Table 5.1's length.
///
/// Multi-byte values are little-endian, and an array of 1- or 2-bit
/// elements packs its first element into the lowest bits of its first
/// byte (Vol 1 Part E 2.9, 2.9.2): the AFH channel map and the channel
/// classification are read that way.
fn params_words(pdu: &Pdu, p: &[u8]) -> String {
    if let Some(what) = key_material(pdu.name) {
        return what.to_string();
    }
    let mut w = Words::new();
    match pdu.name {
        "accepted" => w.push(p.first().map(|&op| opcode_name(Opcode::Short(op)))),
        "not_accepted" => {
            w.push(p.first().map(|&op| opcode_name(Opcode::Short(op))));
            w.push(p.get(1).map(|&e| error_name(e)));
        }
        "accepted_ext" => w.push(escaped(p)),
        "not_accepted_ext" => {
            w.push(escaped(p));
            w.push(p.get(2).map(|&e| error_name(e)));
        }
        "detach" => w.push(p.first().map(|&e| error_name(e))),
        "version_req" | "version_res" => {
            w.push(p.first().map(|&v| match assigned::core_version(v) {
                Some(name) => format!("Core {name}"),
                None => format!("version 0x{v:02x}"),
            }));
            w.push(u16le(p, 1).map(|id| match assigned::company(id) {
                Some(name) => format!("{name} (0x{id:04x})"),
                None => format!("company 0x{id:04x} (not in the SIG list)"),
            }));
            w.push(u16le(p, 3).map(|sub| format!("sub 0x{sub:04x}")));
        }
        "encryption_mode_req" => w.push(p.first().map(|&m| match m {
            0 => "off".into(),
            1 => "on".into(),
            2 => "2: previously used".into(),
            n => format!("{n}: reserved"),
        })),
        "encryption_key_size_req" => w.push(p.first().map(|n| format!("{n} bytes"))),
        "timing_accuracy_res" => {
            w.push(p.first().map(|d| format!("drift {d} ppm")));
            w.push(p.get(1).map(|j| format!("jitter {j} us")));
        }
        "max_slot" | "max_slot_req" => w.push(p.first().map(|n| format!("{n} slots"))),
        "packet_type_table_req" => w.push(p.first().map(|&t| match t {
            0 => "1 Mb/s only".into(),
            1 => "2/3 Mb/s".into(),
            n => format!("{n}: reserved"),
        })),
        "supervision_timeout" => w.push(u16le(p, 0).map(|n| match n {
            0 => "infinite".into(),
            n => format!("{n} slots ({:.1} s)", n as f64 * 0.000625),
        })),
        "clkoffset_res" => w.push(u16le(p, 0).map(|n| format!("offset {} × 1.25 ms", n & 0x7fff))),
        "preferred_rate" => w.push(p.first().map(|&r| data_rate(r))),
        "name_req" => w.push(p.first().map(|o| format!("offset {o}"))),
        "name_res" => {
            w.push(p.first().map(|o| format!("offset {o}")));
            let fragment = match (p.first(), p.get(1)) {
                (Some(&offset), Some(&length)) => {
                    let n = (length.saturating_sub(offset) as usize).min(14);
                    p.get(2..2 + n).map(|f| {
                        let shown = f
                            .iter()
                            .rposition(|&b| b != 0)
                            .map_or(&f[..0], |e| &f[..=e]);
                        format!(
                            "{} of {length} bytes · \"{}\"",
                            shown.len(),
                            name_text(shown)
                        )
                    })
                }
                _ => None,
            };
            w.push(fragment);
        }
        "features_req_ext" | "features_res_ext" => {
            w.push(p.first().map(|n| format!("page {n}")));
            w.push(p.get(1).map(|n| format!("max page {n}")));
            w.push(p.get(2..10).map(hex));
        }
        "set_afh" => {
            let instant = u32le(p, 0);
            let mode = p.get(4).copied();
            let used = p.get(5..15).map(|m| bits_set(m, 79));
            w.push(mode.map(|m| match m {
                0 => "off".into(),
                1 => "on".into(),
                n => format!("mode {n}: reserved"),
            }));
            // The map means something only while AFH is on (Part C 5.2).
            if mode == Some(1) {
                w.push(used.map(|n| format!("{n} of 79 used")));
            }
            w.push(instant.map(|i| format!("instant 0x{i:08x}")));
        }
        "channel_classification" => w.push(p.get(0..10).map(classification)),
        "sniff_req" => {
            let d = u16le(p, 1);
            let t = u16le(p, 3);
            w.push(t.map(|t| format!("T {t}")));
            w.push(d.map(|d| format!("D {d} slots")));
            w.push(u16le(p, 7).map(|n| format!("timeout {n} slots")));
        }
        "io_capability_req" | "io_capability_res" => {
            w.push(p.first().map(|&c| {
                match c {
                    0 => "Display only",
                    1 => "Display YesNo",
                    2 => "KeyboardOnly",
                    3 => "NoInputNoOutput",
                    _ => return format!("IO {c}: reserved"),
                }
                .to_string()
            }));
            w.push(p.get(1).map(|&o| {
                match o {
                    0 => "no OOB data",
                    1 => "OOB data received",
                    _ => return format!("OOB {o}: reserved"),
                }
                .to_string()
            }));
            w.push(p.get(2).map(|&a| {
                match a {
                    0x00 => "MITM Protection Not Required – No Bonding",
                    0x01 => "MITM Protection Required – No Bonding",
                    0x02 => "MITM Protection Not Required – Dedicated Bonding",
                    0x03 => "MITM Protection Required – Dedicated Bonding",
                    0x04 => "MITM Protection Not Required – General Bonding",
                    0x05 => "MITM Protection Required – General Bonding",
                    _ => return format!("authentication 0x{a:02x}: reserved"),
                }
                .to_string()
            }));
        }
        "power_control_req" => w.push(p.first().map(|&a| {
            match a {
                0 => "down one step",
                1 => "up one step",
                2 => "to maximum",
                _ => return format!("{a}: reserved"),
            }
            .to_string()
        })),
        "power_control_res" => w.push(p.first().map(|&r| {
            ["GFSK", "π/4-DQPSK", "8DPSK"]
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let answer = match (r >> (2 * i)) & 0b11 {
                        0 => "not supported",
                        1 => "one step",
                        2 => "max",
                        _ => "min",
                    };
                    format!("{m} {answer}")
                })
                .collect::<Vec<_>>()
                .join(" · ")
        })),
        _ => return hex(p),
    }
    w.done()
}

/// An escape and extended opcode pair, read as the PDU it names.
fn escaped(p: &[u8]) -> Option<String> {
    Some(opcode_name(Opcode::Escaped(*p.first()?, *p.get(1)?)))
}

/// Data_Rate's fields (Part C 5.2): bit 0 FEC, bits 1-2 the Basic Rate
/// packet size, bits 3-4 the EDR rate, bits 5-6 the EDR packet size.
fn data_rate(r: u8) -> String {
    let size = |s: u8| match s {
        0 => "no size preference",
        1 => "1-slot",
        2 => "3-slot",
        _ => "5-slot",
    };
    let fec = if r & 1 == 1 { "no FEC" } else { "FEC" };
    let rate = match (r >> 3) & 0b11 {
        0 => "DM1",
        1 => "2 Mb/s",
        2 => "3 Mb/s",
        _ => "reserved",
    };
    format!(
        "BR: {fec}, {} · EDR: {rate}, {}",
        size((r >> 1) & 0b11),
        size((r >> 5) & 0b11)
    )
}

/// How many of the first `n` one-bit elements are set, element 0 in the
/// lowest bit of the first byte.
fn bits_set(bytes: &[u8], n: usize) -> usize {
    (0..n).filter(|&i| bytes[i / 8] >> (i % 8) & 1 == 1).count()
}

/// AFH_Channel_Classification, 40 two-bit elements: element n classifies
/// channels 2n and 2n+1, the last only channel 78. 0 unknown, 1 good, 3
/// bad, 2 reserved; counted in channels.
fn classification(bytes: &[u8]) -> String {
    let mut count = [0usize; 4];
    for n in 0..40 {
        let class = (bytes[n / 4] >> (2 * (n % 4))) & 0b11;
        count[class as usize] += if n == 39 { 1 } else { 2 };
    }
    let mut out = format!(
        "good {} · bad {} · unknown {}",
        count[1], count[3], count[0]
    );
    if count[2] > 0 {
        out.push_str(&format!(" · reserved {}", count[2]));
    }
    out
}

/// A name fragment as text a terminal can show: valid UTF-8 as written,
/// a control character or a byte that is not UTF-8 as `\xNN`, and a
/// backslash doubled so the two cannot be told apart.
fn name_text(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            if c.is_control() {
                let mut buf = [0; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("\\x{b:02x}"));
                }
            } else if c == '\\' {
                out.push_str("\\\\");
            } else {
                out.push(c);
            }
        }
        for b in chunk.invalid() {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
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
        assert_eq!(parse(&[47 << 1]).unwrap().words(), "timing_accuracy_req");
        assert_eq!(
            parse(&[67 << 1, 1]).unwrap().words(),
            "opcode 67: not in Table 5.1  01"
        );
        assert_eq!(
            parse(&[3 << 1]).unwrap().words(),
            "accepted  cut short  (length 1, Table 5.1: 2)"
        );
    }

    fn words(body: &[u8]) -> String {
        parse(body).unwrap().words()
    }

    #[test]
    fn answers_name_what_they_answer() {
        assert_eq!(words(&[3 << 1, 15]), "accepted  encryption_mode_req");
        assert_eq!(
            words(&[127 << 1 | 1, 1, 127, 11]),
            "accepted_ext  packet_type_table_req"
        );
        assert_eq!(
            words(&[4 << 1, 23, 0x1a]),
            "not_accepted  sniff_req · Unsupported Remote Feature (0x1a)"
        );
        assert_eq!(
            words(&[7 << 1, 0x13]),
            "detach  Remote User Terminated Connection (0x13)"
        );
        assert_eq!(
            error_name(0x55),
            "error 0x55: not in Vol 1 Part F Table 1.1"
        );
    }

    #[test]
    fn settings_read_as_words() {
        assert_eq!(words(&[15 << 1, 1]), "encryption_mode_req  on");
        assert_eq!(words(&[15 << 1, 0]), "encryption_mode_req  off");
        assert_eq!(words(&[16 << 1, 16]), "encryption_key_size_req  16 bytes");
        assert_eq!(
            words(&[48 << 1 | 1, 20, 1]),
            "timing_accuracy_res  drift 20 ppm · jitter 1 us"
        );
        assert_eq!(words(&[45 << 1, 5]), "max_slot  5 slots");
        assert_eq!(words(&[127 << 1, 11, 1]), "packet_type_table_req  2/3 Mb/s");
        assert_eq!(
            words(&[55 << 1, 0x00, 0x7d]),
            "supervision_timeout  32000 slots (20.0 s)"
        );
        assert_eq!(words(&[55 << 1, 0, 0]), "supervision_timeout  infinite");
        // Data_Rate: bit 0 no FEC, bits 1-2 BR 5-slot, bits 3-4 EDR
        // 2 Mb/s, bits 5-6 EDR 3-slot (Part C 5.2).
        assert_eq!(
            words(&[36 << 1, 0b0100_1111]),
            "preferred_rate  BR: no FEC, 5-slot · EDR: 2 Mb/s, 3-slot"
        );
    }

    #[test]
    fn features_stay_hex_in_the_order_sent() {
        assert_eq!(
            words(&[39 << 1, 0xbf, 0xfe, 0xcf, 0xfe, 0xdb, 0xff, 0x7b, 0x87]),
            "features_req  bf fe cf fe db ff 7b 87"
        );
        assert_eq!(
            words(&[127 << 1 | 1, 4, 1, 2, 0x0f, 0, 0, 0, 0, 0, 0, 0]),
            "features_res_ext  page 1 · max page 2 · 0f 00 00 00 00 00 00 00"
        );
    }

    #[test]
    fn a_name_fragment_is_shown_as_text_and_never_as_control_bytes() {
        let mut b = vec![2 << 1 | 1, 0, 22];
        b.extend(b"WH-1000XM4\0\0\0\0");
        assert_eq!(
            words(&b),
            "name_res  offset 0 · 10 of 22 bytes · \"WH-1000XM4\""
        );
        let mut b = vec![2 << 1, 0, 16];
        b.extend([b'a', 0x1b, b'[', 0xc3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let w = words(&b);
        assert!(!w.contains('\u{1b}'), "{w}");
        assert!(w.contains("\\x1b") && w.contains("\\xc3"), "{w}");
    }

    #[test]
    fn channel_maps_are_counted() {
        // set_AFH: instant, mode on, channels 0..20 used.
        let mut b = vec![60 << 1, 0xf0, 0xa4, 0x12, 0x00, 1];
        b.extend([0xff, 0xff, 0x0f, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            words(&b),
            "set_afh  on · 20 of 79 used · instant 0x0012a4f0"
        );
        // channel_classification: element 0 good (channels 0, 1), 1 bad, rest unknown.
        let mut b = vec![127 << 1 | 1, 17];
        b.extend([0b1101, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            words(&b),
            "channel_classification  good 2 · bad 2 · unknown 75"
        );
    }

    #[test]
    fn key_material_is_named_never_printed() {
        for (op, what) in [
            (11u8, "16-byte random number"),        // au_rand
            (12, "4-byte authentication response"), // sres
            (17, "16-byte random number"),          // start_encryption_req
            (10, "16-byte key"),                    // unit_key
        ] {
            let mut b = vec![op << 1];
            b.extend([0xa5; 16]);
            let w = words(&b);
            assert!(w.contains(what), "{w}");
            assert!(!w.contains("a5"), "{w}");
        }
    }

    /// Too few bytes for a parameter: what is there is read, nothing past
    /// the end, and the length is said.
    #[test]
    fn a_short_body_reads_what_it_has() {
        assert_eq!(
            words(&[48 << 1, 20]),
            "timing_accuracy_res  drift 20 ppm · cut short  (length 2, Table 5.1: 3)"
        );
        assert_eq!(
            words(&[60 << 1, 1]),
            "set_afh  cut short  (length 2, Table 5.1: 16)"
        );
    }

    /// The rows the tests above do not reach, each against Part C 5.2's
    /// own values and Table 5.1's byte positions.
    #[test]
    fn the_other_parameters_read_as_the_core_words_them() {
        assert_eq!(
            words(&[127 << 1, 2, 127, 12, 0x24]),
            "not_accepted_ext  esco_link_req · LMP PDU Not Allowed (0x24)"
        );
        assert_eq!(
            words(&[127 << 1, 25, 1, 0, 0x03]),
            "io_capability_req  Display YesNo · no OOB data · MITM Protection Required – Dedicated Bonding"
        );
        assert_eq!(words(&[127 << 1, 31, 2]), "power_control_req  to maximum");
        // GFSK 2 (max), π/4-DQPSK 1 (one step), 8DPSK 0 (not supported).
        assert_eq!(
            words(&[127 << 1, 32, 0b00_01_10]),
            "power_control_res  GFSK max · π/4-DQPSK one step · 8DPSK not supported"
        );
        // uint15: the top bit is not part of the offset.
        assert_eq!(
            words(&[6 << 1 | 1, 0x39, 0xb0]),
            "clkoffset_res  offset 12345 × 1.25 ms"
        );
        // Flags, D 0x0002, T 0x0320, attempt, timeout 0x0004.
        assert_eq!(
            words(&[23 << 1, 0, 2, 0, 0x20, 0x03, 1, 0, 4, 0]),
            "sniff_req  T 800 · D 2 slots · timeout 4 slots"
        );
        assert_eq!(words(&[1 << 1, 14]), "name_req  offset 14");
        // AFH off: the map is reserved, so it is not counted.
        let mut b = vec![60 << 1, 1, 0, 0, 0, 0];
        b.extend([0xff; 10]);
        assert_eq!(words(&b), "set_afh  off · instant 0x00000001");
        // A reserved class is counted apart, not folded into another.
        let mut b = vec![127 << 1 | 1, 17, 0b10];
        b.extend([0; 9]);
        assert_eq!(
            words(&b),
            "channel_classification  good 0 · bad 0 · unknown 77 · reserved 2"
        );
    }

    #[test]
    fn a_version_names_its_company() {
        assert_eq!(
            words(&[38 << 1 | 1, 0x0b, 0x1d, 0x00, 0x00, 0x21]),
            "version_res  Core 5.2 · Qualcomm (0x001d) · sub 0x2100"
        );
        assert_eq!(
            words(&[37 << 1, 0x30, 0xfe, 0xff, 0, 0]),
            "version_req  version 0x30 · company 0xfffe (not in the SIG list) · sub 0x0000"
        );
    }
}
