// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The link layer's control PDUs (LLID 0b11): what a connection's two ends
//! say to each other about the connection itself.
//!
//! **Core 5.4 Vol 6 Part B 2.4.2**, read from the SIG's HTML copy on
//! 2026-10-01. Table 2.20 gives the opcodes, 0x00 LL_CONNECTION_UPDATE_IND
//! to 0x2A LL_PERIODIC_SYNC_WR_IND; each PDU's CtrData is laid out in a
//! figure of its own subsection (Figures 2.27 to 2.58), and the lengths in
//! [`OPCODES`] were read off those figures, fields summed in octets. They
//! agree with the Core's own encryption sample data (Vol 6 Part C 1):
//! LL_ENC_REQ is 1 + 22 octets there, LL_ENC_RSP 1 + 12, LL_START_ENC_REQ
//! the opcode alone.
//!
//! **A PDU is read only at its own length.** On an encrypted link the
//! opcode itself is encrypted and four octets of MIC follow, so an
//! encrypted PDU reads as some other opcode at the wrong length, or as none:
//! the length check is what keeps it from being read as one. A length that
//! disagrees says so and nothing more (rule 2); the Core lets a receiver
//! accept a long PDU (2.4.2: "if the PDU is too long it can ignore the extra
//! data"), and this does not.
//!
//! **Key material is named, never printed**: LL_ENC_REQ's and LL_ENC_RSP's
//! random number, diversifiers and initialisation vectors.

use crate::signal::errors::error_name;

/// Table 2.20, with each PDU's CtrData length in octets from its figure.
pub const OPCODES: &[(u8, &str, usize)] = &[
    (0x00, "LL_CONNECTION_UPDATE_IND", 11),
    (0x01, "LL_CHANNEL_MAP_IND", 7),
    (0x02, "LL_TERMINATE_IND", 1),
    (0x03, "LL_ENC_REQ", 22),
    (0x04, "LL_ENC_RSP", 12),
    (0x05, "LL_START_ENC_REQ", 0),
    (0x06, "LL_START_ENC_RSP", 0),
    (0x07, "LL_UNKNOWN_RSP", 1),
    (0x08, "LL_FEATURE_REQ", 8),
    (0x09, "LL_FEATURE_RSP", 8),
    (0x0a, "LL_PAUSE_ENC_REQ", 0),
    (0x0b, "LL_PAUSE_ENC_RSP", 0),
    (0x0c, "LL_VERSION_IND", 5),
    (0x0d, "LL_REJECT_IND", 1),
    (0x0e, "LL_PERIPHERAL_FEATURE_REQ", 8),
    (0x0f, "LL_CONNECTION_PARAM_REQ", 23),
    (0x10, "LL_CONNECTION_PARAM_RSP", 23),
    (0x11, "LL_REJECT_EXT_IND", 2),
    (0x12, "LL_PING_REQ", 0),
    (0x13, "LL_PING_RSP", 0),
    (0x14, "LL_LENGTH_REQ", 8),
    (0x15, "LL_LENGTH_RSP", 8),
    (0x16, "LL_PHY_REQ", 2),
    (0x17, "LL_PHY_RSP", 2),
    (0x18, "LL_PHY_UPDATE_IND", 4),
    (0x19, "LL_MIN_USED_CHANNELS_IND", 2),
    (0x1a, "LL_CTE_REQ", 1),
    (0x1b, "LL_CTE_RSP", 0),
    (0x1c, "LL_PERIODIC_SYNC_IND", 34),
    (0x1d, "LL_CLOCK_ACCURACY_REQ", 1),
    (0x1e, "LL_CLOCK_ACCURACY_RSP", 1),
    (0x1f, "LL_CIS_REQ", 35),
    (0x20, "LL_CIS_RSP", 8),
    (0x21, "LL_CIS_IND", 15),
    (0x22, "LL_CIS_TERMINATE_IND", 3),
    (0x23, "LL_POWER_CONTROL_REQ", 3),
    (0x24, "LL_POWER_CONTROL_RSP", 4),
    (0x25, "LL_POWER_CHANGE_IND", 4),
    (0x26, "LL_SUBRATE_REQ", 10),
    (0x27, "LL_SUBRATE_IND", 10),
    (0x28, "LL_CHANNEL_REPORTING_IND", 3),
    (0x29, "LL_CHANNEL_STATUS_IND", 10),
    (0x2a, "LL_PERIODIC_SYNC_WR_IND", 42),
];

/// One LL Control PDU, as far as it can be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub opcode: u8,
    /// Table 2.20's name, `None` for an opcode the table does not have.
    pub name: Option<&'static str>,
    /// Its parameters in words, or why they are not read.
    pub words: String,
}

/// A change to the connection that takes effect at an instant (5.1), or
/// its end: what a follower must apply to keep following.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Update {
    /// LL_CONNECTION_UPDATE_IND, in the PDU's own units (1.25 ms, events,
    /// 10 ms).
    Connection {
        win_size: u8,
        win_offset: u16,
        interval: u16,
        latency: u16,
        timeout: u16,
        instant: u16,
    },
    /// LL_CHANNEL_MAP_IND: bit `n` set is data channel `n` used.
    ChannelMap { map: u64, instant: u16 },
    /// LL_PHY_UPDATE_IND: each direction's new PHY as Table 2.21's bit, 0
    /// unchanged.
    Phy {
        c_to_p: u8,
        p_to_c: u8,
        instant: u16,
    },
    /// LL_TERMINATE_IND.
    Terminate { reason: u8 },
    /// LL_SUBRATE_IND: the events are spaced anew (4.5.1's connSubrateFactor),
    /// which a follower does not follow; it is told, so it can say so.
    Subrate,
}

fn u16le(p: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([p[at], p[at + 1]])
}

fn hex(p: &[u8]) -> String {
    p.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Table 2.21's PHY bits in words, `1M, 2M`.
fn phys(bits: u8) -> String {
    let names: Vec<&str> = [(0, "1M"), (1, "2M"), (2, "Coded")]
        .iter()
        .filter(|(b, _)| bits >> b & 1 != 0)
        .map(|(_, n)| *n)
        .collect();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

/// The opcode's entry in Table 2.20.
fn entry(opcode: u8) -> Option<(&'static str, usize)> {
    OPCODES
        .iter()
        .find(|o| o.0 == opcode)
        .map(|&(_, name, len)| (name, len))
}

/// Read an unencrypted LL Control PDU from its payload (opcode first).
pub fn read(payload: &[u8]) -> Control {
    let Some(&opcode) = payload.first() else {
        return Control {
            opcode: 0,
            name: None,
            words: "empty".to_string(),
        };
    };
    let Some((name, len)) = entry(opcode) else {
        return Control {
            opcode,
            name: None,
            words: format!("opcode 0x{opcode:02x}: not in Table 2.20"),
        };
    };
    let words = if payload.len() != len + 1 {
        format!(
            "not readable at this length ({} octets, Table 2.20's PDU has {})",
            payload.len(),
            len + 1
        )
    } else {
        words(opcode, &payload[1..])
    };
    Control {
        opcode,
        name: Some(name),
        words,
    }
}

/// The parameters of a PDU already checked to be its own length.
fn words(opcode: u8, p: &[u8]) -> String {
    match opcode {
        0x00 => format!(
            "interval {:.2} ms · latency {} · timeout {} ms · instant {}",
            u16le(p, 3) as f64 * 1.25,
            u16le(p, 5),
            u16le(p, 7) as u32 * 10,
            u16le(p, 9)
        ),
        0x01 => {
            let used = p[..5].iter().map(|b| b.count_ones()).sum::<u32>();
            format!("{used} of 37 used · instant {}", u16le(p, 5))
        }
        0x02 | 0x0d => error_name(p[0]),
        0x03 => "8-byte random number, EDIV, SKD_C and IV_C".to_string(),
        0x04 => "SKD_P and IV_P".to_string(),
        0x07 => entry(p[0]).map_or(format!("opcode 0x{:02x}", p[0]), |e| e.0.to_string()),
        0x08 | 0x09 | 0x0e => hex(p),
        0x0c => format!(
            "version 0x{:02x} · company 0x{:04x} · sub 0x{:04x}",
            p[0],
            u16le(p, 1),
            u16le(p, 3)
        ),
        0x11 => {
            let rejected =
                entry(p[0]).map_or(format!("opcode 0x{:02x}", p[0]), |e| e.0.to_string());
            format!("{rejected} · {}", error_name(p[1]))
        }
        0x14 | 0x15 => format!(
            "max RX {} octets, {} us · max TX {} octets, {} us",
            u16le(p, 0),
            u16le(p, 2),
            u16le(p, 4),
            u16le(p, 6)
        ),
        0x16 | 0x17 => format!("TX {} · RX {}", phys(p[0]), phys(p[1])),
        0x18 => {
            if p[0] == 0 && p[1] == 0 {
                // "If both ... are zero then there is no Instant" (2.4.2.23).
                "no change".to_string()
            } else {
                let way = |b: u8| {
                    if b == 0 {
                        "unchanged".to_string()
                    } else {
                        phys(b)
                    }
                };
                format!(
                    "C→P {} · P→C {} · instant {}",
                    way(p[0]),
                    way(p[1]),
                    u16le(p, 2)
                )
            }
        }
        _ => hex(p),
    }
}

/// The change an LL Control PDU makes to the connection, if it is one a
/// follower must apply, read at its own length only.
pub fn update(payload: &[u8]) -> Option<Update> {
    let (&opcode, p) = payload.split_first()?;
    let (_, len) = entry(opcode)?;
    if p.len() != len {
        return None;
    }
    match opcode {
        0x00 => Some(Update::Connection {
            win_size: p[0],
            win_offset: u16le(p, 1),
            interval: u16le(p, 3),
            latency: u16le(p, 5),
            timeout: u16le(p, 7),
            instant: u16le(p, 9),
        }),
        0x01 => Some(Update::ChannelMap {
            map: p[..5]
                .iter()
                .enumerate()
                .fold(0u64, |m, (i, &b)| m | (b as u64) << (8 * i))
                & ((1u64 << 37) - 1),
            instant: u16le(p, 5),
        }),
        0x02 => Some(Update::Terminate { reason: p[0] }),
        0x18 => Some(Update::Phy {
            c_to_p: p[0],
            p_to_c: p[1],
            instant: u16le(p, 2),
        }),
        0x27 => Some(Update::Subrate),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_whole_and_in_order() {
        let ops: Vec<u8> = OPCODES.iter().map(|o| o.0).collect();
        assert_eq!(ops, (0x00..=0x2a).collect::<Vec<u8>>());
    }

    /// The Core's encryption sample PDUs (Vol 6 Part C 1) are the lengths
    /// the table gives, and the encrypted one is not read as a control PDU.
    #[test]
    fn the_cores_sample_pdus_have_the_tables_lengths() {
        let enc_req = [
            0x03, 0x90, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x74, 0x24, 0x13, 0x02, 0xf1,
            0xe0, 0xdf, 0xce, 0xbd, 0xac, 0x24, 0xab, 0xdc, 0xba,
        ];
        let c = read(&enc_req);
        assert_eq!(c.name, Some("LL_ENC_REQ"));
        assert!(!c.words.contains("not readable"), "{}", c.words);
        let rsp = read(&[
            0x04, 0x79, 0x68, 0x57, 0x46, 0x35, 0x24, 0x13, 0x02, 0xbe, 0xba, 0xaf, 0xde,
        ]);
        assert_eq!(rsp.name, Some("LL_ENC_RSP"));
        assert!(!rsp.words.contains("not readable"), "{}", rsp.words);
        assert_eq!(read(&[0x05]).name, Some("LL_START_ENC_REQ"));
        // LL_START_ENC_RSP1 as sent, encrypted: "Control Type Encrypted:0x9F".
        let encrypted = read(&[0x9f, 0xcd, 0xa7, 0xf4, 0x48]);
        assert_eq!(encrypted.name, None);
    }

    /// Key material is named, never printed.
    #[test]
    fn key_material_is_named_never_printed() {
        let mut enc_req = vec![0x03];
        enc_req.extend([0xa5; 22]);
        let w = read(&enc_req).words;
        assert!(!w.contains("a5"), "{w}");
        assert!(w.contains("8-byte random number"), "{w}");
    }

    #[test]
    fn a_version_reads_as_numbers() {
        // LL_VERSION_IND: VersNr 0x0c, CompId 0x004c, SubVersNr 0x1234.
        let c = read(&[0x0c, 0x0c, 0x4c, 0x00, 0x34, 0x12]);
        assert_eq!(c.name, Some("LL_VERSION_IND"));
        assert_eq!(c.words, "version 0x0c · company 0x004c · sub 0x1234");
    }

    #[test]
    fn a_wrong_length_is_not_read() {
        let c = read(&[0x0c, 0x0c, 0x4c]);
        assert_eq!(c.name, Some("LL_VERSION_IND"));
        assert_eq!(
            c.words,
            "not readable at this length (3 octets, Table 2.20's PDU has 6)"
        );
    }

    #[test]
    fn an_unknown_opcode_says_so() {
        let c = read(&[0x7f]);
        assert_eq!(c.name, None);
        assert_eq!(c.words, "opcode 0x7f: not in Table 2.20");
        assert_eq!(read(&[]).words, "empty");
    }

    #[test]
    fn settings_read_as_words() {
        assert_eq!(read(&[0x16, 0b011, 0b010]).words, "TX 1M, 2M · RX 2M");
        assert_eq!(
            read(&[0x18, 0x02, 0x00, 0x04, 0x00]).words,
            "C→P 2M · P→C unchanged · instant 4"
        );
        assert_eq!(read(&[0x18, 0, 0, 0, 0]).words, "no change");
        assert_eq!(
            read(&[0x00, 1, 2, 0, 6, 0, 0, 0, 0x64, 0, 0x10, 0]).words,
            "interval 7.50 ms · latency 0 · timeout 1000 ms · instant 16"
        );
        assert_eq!(
            read(&[0x01, 0xff, 0xff, 0x1f, 0x00, 0x00, 0x34, 0x12]).words,
            "21 of 37 used · instant 4660"
        );
        assert_eq!(
            read(&[0x02, 0x13]).words,
            "Remote User Terminated Connection (0x13)"
        );
        assert_eq!(read(&[0x07, 0x08]).words, "LL_FEATURE_REQ");
        assert_eq!(read(&[0x12]).words, "");
    }

    #[test]
    fn updates_carry_their_instant() {
        let u = update(&[0x01, 0xff, 0xff, 0x00, 0x00, 0x00, 0x34, 0x12]).unwrap();
        assert_eq!(
            u,
            Update::ChannelMap {
                map: 0xffff,
                instant: 0x1234
            }
        );
        let u = update(&[0x18, 0x02, 0x02, 0x00, 0x01]).unwrap();
        assert_eq!(
            u,
            Update::Phy {
                c_to_p: 2,
                p_to_c: 2,
                instant: 0x0100
            }
        );
        let u = update(&[0x00, 1, 2, 0, 6, 0, 0, 0, 0x64, 0, 0x10, 0]).unwrap();
        assert_eq!(
            u,
            Update::Connection {
                win_size: 1,
                win_offset: 2,
                interval: 6,
                latency: 0,
                timeout: 100,
                instant: 16
            }
        );
        assert_eq!(
            update(&[0x02, 0x13]),
            Some(Update::Terminate { reason: 0x13 })
        );
        let mut subrate = vec![0x27];
        subrate.extend([0; 10]);
        assert_eq!(update(&subrate), Some(Update::Subrate));
        // Read only at the table's length.
        assert_eq!(update(&[0x02]), None);
        assert_eq!(update(&[0x0c, 0x0c, 0x4c, 0x00, 0x34, 0x12]), None);
    }
}
