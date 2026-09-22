// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What a device advertises: the AD structures in an advertising or scan
//! response payload (net-ux-polish-plan 5.2).
//!
//! **Where each fact was read.** The data types - Flags and its bits, Local
//! Name, TX Power Level, the Service UUID lists, Service Data, Manufacturer
//! Specific Data with its two-octet company identifier - from the Core
//! Specification Supplement v11, Part A, read on the SIG's own site this
//! session. The type codes from the SIG's Assigned Numbers registry
//! (`assigned_numbers/core/ad_types.yaml`), likewise. The framing itself - a
//! length octet covering the type and the data, and a zero length ending the
//! significant part - from public documentation of Core Vol 3 Part C 11
//! (Silicon Labs, Nordic), not from a copy of that section read this session:
//! the standing `pdu` states for its own layout.
//!
//! **Malformed is shown as malformed, at its offset, and parsing stops
//! there.** A length that runs past the end, a list whose length is not a
//! whole number of UUIDs, a TX power that is not one octet, a name that is not
//! UTF-8: each is a real fact about what the device sent, and skipping it, or
//! reading on past it at a guessed boundary, would put invented structures on
//! screen (rule 2). An AD type the registry names but this does not decode is
//! kept with its name and bytes; one the registry does not name is kept with
//! its code alone.
//!
//! **A company is a number here.** Manufacturer data carries a SIG-assigned
//! company identifier, and a name for it belongs to a cited snapshot of that
//! registry, not to this parser (rule 2, and the IEEE listing's precedent in
//! `signal::net::vendor`).
//!
//! **Beside the physics, never instead of it** (rule 3): a payload says what
//! the device claims; SNR, CFO and the modulation say how it sent it.
//!
//! **Text from the air is untrusted.** A local name is whatever the device
//! chose to send, control characters included, and a terminal takes some of
//! those as commands. [`printable`] is how a name reaches a screen or a file.

/// Which payload octets are advertising data, for the PDU types that carry
/// it: everything after AdvA on ADV_IND, ADV_NONCONN_IND, ADV_SCAN_IND and
/// SCAN_RSP. `None` for every other type, whose payload is something else.
pub fn adv_data(pdu_type: super::pdu::PduType, payload: &[u8]) -> Option<&[u8]> {
    use super::pdu::PduType;
    match pdu_type {
        PduType::AdvInd | PduType::AdvNonconnInd | PduType::AdvScanInd | PduType::ScanRsp => {
            payload.get(6..)
        }
        _ => None,
    }
}

/// The registry's name for an AD type code, for the ones this parser does not
/// decode as well as the ones it does. `None` for a code the registry does not
/// list.
// Drawn by the packet detail view (net-ux-polish-plan 5.4); goes with it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn type_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0x01 => "Flags",
        0x02 => "Incomplete List of 16-bit Service UUIDs",
        0x03 => "Complete List of 16-bit Service UUIDs",
        0x04 => "Incomplete List of 32-bit Service UUIDs",
        0x05 => "Complete List of 32-bit Service UUIDs",
        0x06 => "Incomplete List of 128-bit Service UUIDs",
        0x07 => "Complete List of 128-bit Service UUIDs",
        0x08 => "Shortened Local Name",
        0x09 => "Complete Local Name",
        0x0A => "Tx Power Level",
        0x0D => "Class of Device",
        0x0E => "Simple Pairing Hash C-192",
        0x0F => "Simple Pairing Randomizer R-192",
        0x10 => "Device ID / Security Manager TK Value",
        0x11 => "Security Manager Out of Band Flags",
        0x12 => "Peripheral Connection Interval Range",
        0x14 => "List of 16-bit Service Solicitation UUIDs",
        0x15 => "List of 128-bit Service Solicitation UUIDs",
        0x16 => "Service Data - 16-bit UUID",
        0x17 => "Public Target Address",
        0x18 => "Random Target Address",
        0x19 => "Appearance",
        0x1A => "Advertising Interval",
        0x1B => "LE Bluetooth Device Address",
        0x1C => "LE Role",
        0x1D => "Simple Pairing Hash C-256",
        0x1E => "Simple Pairing Randomizer R-256",
        0x1F => "List of 32-bit Service Solicitation UUIDs",
        0x20 => "Service Data - 32-bit UUID",
        0x21 => "Service Data - 128-bit UUID",
        0x22 => "LE Secure Connections Confirmation Value",
        0x23 => "LE Secure Connections Random Value",
        0x24 => "URI",
        0x25 => "Indoor Positioning",
        0x26 => "Transport Discovery Data",
        0x27 => "LE Supported Features",
        0x28 => "Channel Map Update Indication",
        0x29 => "PB-ADV",
        0x2A => "Mesh Message",
        0x2B => "Mesh Beacon",
        0x2C => "BIGInfo",
        0x2D => "Broadcast_Code",
        0x2E => "Resolvable Set Identifier",
        0x2F => "Advertising Interval - long",
        0x30 => "Broadcast_Name",
        0x31 => "Encrypted Advertising Data",
        0x32 => "Periodic Advertising Response Timing Information",
        0x34 => "Electronic Shelf Label",
        0x3D => "3D Information Data",
        0xFF => "Manufacturer Specific Data",
        _ => return None,
    })
}

/// The Flags bits, CSS v11 Part A 1.3: bit 4 is "Previously Used", and
/// bits 5 to 7 are not defined there, so they are shown as set bits rather
/// than named.
// Drawn by the packet detail view (net-ux-polish-plan 5.4); goes with it.
#[cfg_attr(not(test), allow(dead_code))]
pub const FLAG_BITS: [(u8, &str); 5] = [
    (0, "LE Limited Discoverable"),
    (1, "LE General Discoverable"),
    (2, "BR/EDR Not Supported"),
    (3, "Simultaneous LE and BR/EDR (Controller)"),
    (4, "previously used bit"),
];

/// The set bits of a flags octet, named where CSS v11 names them and by
/// number where it does not: `["LE General Discoverable", "BR/EDR Not
/// Supported", "bit 6"]`.
// Drawn by the packet detail view (net-ux-polish-plan 5.4); goes with it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn flag_names(flags: u8) -> Vec<String> {
    (0..8u8)
        .filter(|b| flags & (1 << b) != 0)
        .map(|b| {
            FLAG_BITS
                .iter()
                .find(|(bit, _)| *bit == b)
                .map(|(_, name)| name.to_string())
                .unwrap_or_else(|| format!("bit {b}"))
        })
        .collect()
}

/// One AD structure's content.
#[derive(Clone, Debug, PartialEq)]
pub enum Ad {
    /// The flags octet (the first; the data type may be longer).
    Flags(u8),
    /// A service UUID list: `bits` is 16, 32 or 128, `complete` says
    /// whether the device claims it is the whole list. 128-bit UUIDs are
    /// kept most significant octet first, the order they are written in.
    Uuids {
        bits: u16,
        complete: bool,
        uuids: Vec<Vec<u8>>,
    },
    /// A local name, `complete` or shortened, as sent (see [`printable`]).
    Name { complete: bool, text: String },
    /// TX power level, dBm.
    TxPower(i8),
    /// Service data: the UUID (written order) and what follows it.
    ServiceData { uuid: Vec<u8>, data: Vec<u8> },
    /// Manufacturer data: the company identifier and what follows it.
    Manufacturer { company: u16, data: Vec<u8> },
    /// A type this does not decode: named where the registry names it.
    Other { code: u8, data: Vec<u8> },
}

/// One structure, or where the payload stopped making sense.
#[derive(Clone, Debug, PartialEq)]
pub enum Structure {
    Ad {
        offset: usize,
        ad: Ad,
    },
    /// What went wrong, at which octet of the advertising data; nothing after
    /// it is read.
    Malformed {
        offset: usize,
        why: &'static str,
    },
}

/// Little-endian octets in the order a UUID is written: reversed.
fn written(le: &[u8]) -> Vec<u8> {
    le.iter().rev().copied().collect()
}

/// The structures in `data`, in order, ending at the first malformed one.
pub fn parse(data: &[u8]) -> Vec<Structure> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let length = data[at] as usize;
        if length == 0 {
            // The significant part ends here; what follows is padding, and
            // padding is zeros.
            if data[at..].iter().any(|b| *b != 0) {
                out.push(Structure::Malformed {
                    offset: at,
                    why: "octets after a zero length are not zero",
                });
            }
            break;
        }
        let Some(body) = data.get(at + 1..at + 1 + length) else {
            out.push(Structure::Malformed {
                offset: at,
                why: "length runs past the end of the payload",
            });
            break;
        };
        let (code, value) = (body[0], &body[1..]);
        match decode(code, value) {
            Ok(ad) => out.push(Structure::Ad { offset: at, ad }),
            Err(why) => {
                out.push(Structure::Malformed { offset: at, why });
                break;
            }
        }
        at += 1 + length;
    }
    out
}

fn decode(code: u8, value: &[u8]) -> Result<Ad, &'static str> {
    let uuids = |bits: u16, complete: bool| {
        let size = bits as usize / 8;
        if !value.len().is_multiple_of(size) {
            return Err("UUID list is not a whole number of UUIDs");
        }
        Ok(Ad::Uuids {
            bits,
            complete,
            uuids: value.chunks(size).map(written).collect(),
        })
    };
    let service = |size: usize| {
        if value.len() < size {
            return Err("service data shorter than its UUID");
        }
        Ok(Ad::ServiceData {
            uuid: written(&value[..size]),
            data: value[size..].to_vec(),
        })
    };
    match code {
        0x01 => Ok(Ad::Flags(value.first().copied().unwrap_or(0))),
        0x02 | 0x03 => uuids(16, code == 0x03),
        0x04 | 0x05 => uuids(32, code == 0x05),
        0x06 | 0x07 => uuids(128, code == 0x07),
        0x08 | 0x09 => match std::str::from_utf8(value) {
            Ok(text) => Ok(Ad::Name {
                complete: code == 0x09,
                text: text.to_string(),
            }),
            Err(_) => Err("local name is not UTF-8"),
        },
        0x0A => match value {
            [p] => Ok(Ad::TxPower(*p as i8)),
            _ => Err("TX power level is not one octet"),
        },
        0x16 => service(2),
        0x20 => service(4),
        0x21 => service(16),
        0xFF => match value {
            [lo, hi, rest @ ..] => Ok(Ad::Manufacturer {
                company: u16::from_le_bytes([*lo, *hi]),
                data: rest.to_vec(),
            }),
            _ => Err("manufacturer data shorter than its company identifier"),
        },
        _ => Ok(Ad::Other {
            code,
            data: value.to_vec(),
        }),
    }
}

/// `text` as it may reach a terminal or a file: every control character
/// replaced with U+FFFD, so an advertised name cannot move the cursor, clear
/// the screen or break a CSV row, and the replacement shows that something
/// was there.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect()
}

/// The device's name as advertised, from a list of structures: the complete
/// one where both are present, the shortened one otherwise.
pub fn name(structures: &[Structure]) -> Option<(&str, bool)> {
    let names = structures.iter().filter_map(|s| match s {
        Structure::Ad {
            ad: Ad::Name { complete, text },
            ..
        } => Some((text.as_str(), *complete)),
        _ => None,
    });
    names.max_by_key(|(_, complete)| *complete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::pdu::PduType;

    /// **The real ADV_NONCONN_IND from the air** (`pdu`'s recorded one):
    /// its eight octets of advertising data are one structure, manufacturer
    /// data from company 0x004C with four octets behind the identifier.
    #[test]
    fn the_real_packet_is_manufacturer_data_from_0x004c() {
        let payload = [
            0x9e, 0x27, 0x91, 0x7e, 0x9a, 0xd1, 0x07, 0xff, 0x4c, 0x00, 0x12, 0x02, 0x00, 0x02,
        ];
        let data = adv_data(PduType::AdvNonconnInd, &payload).unwrap();
        assert_eq!(
            parse(data),
            vec![Structure::Ad {
                offset: 0,
                ad: Ad::Manufacturer {
                    company: 0x004C,
                    data: vec![0x12, 0x02, 0x00, 0x02],
                },
            }]
        );
    }

    /// A payload built from each decoded type in turn, each read back as
    /// what it is, at its offset, UUIDs in the order they are written.
    #[test]
    fn every_decoded_type_reads_back() {
        let data = [
            0x02, 0x01, 0x06, // flags: LE General, BR/EDR not supported
            0x05, 0x03, 0x0f, 0x18, 0x0a, 0x18, // complete 16-bit: 0x180F, 0x180A
            0x06, 0x08, b'S', b'e', b'n', b's', b'o', // shortened name
            0x02, 0x0a, 0xf4, // TX power -12 dBm
            0x05, 0x16, 0x0f, 0x18, 0x55, 0x66, // service data 0x180F: 55 66
        ];
        let got = parse(&data);
        assert_eq!(got.len(), 5, "{got:?}");
        assert_eq!(
            got[0],
            Structure::Ad {
                offset: 0,
                ad: Ad::Flags(0x06)
            }
        );
        assert_eq!(
            got[1],
            Structure::Ad {
                offset: 3,
                ad: Ad::Uuids {
                    bits: 16,
                    complete: true,
                    uuids: vec![vec![0x18, 0x0f], vec![0x18, 0x0a]],
                },
            }
        );
        assert_eq!(
            got[2],
            Structure::Ad {
                offset: 9,
                ad: Ad::Name {
                    complete: false,
                    text: "Senso".to_string()
                }
            }
        );
        assert_eq!(
            got[3],
            Structure::Ad {
                offset: 16,
                ad: Ad::TxPower(-12)
            }
        );
        assert_eq!(
            got[4],
            Structure::Ad {
                offset: 19,
                ad: Ad::ServiceData {
                    uuid: vec![0x18, 0x0f],
                    data: vec![0x55, 0x66]
                }
            }
        );
    }

    /// **Malformed at its offset, and nothing read past it.** A length that
    /// overruns, a ragged UUID list, a two-octet TX power, a name that is
    /// not UTF-8, and non-zero octets after a zero length.
    #[test]
    fn malformed_is_said_at_its_offset_and_ends_the_parse() {
        let cases: [(&[u8], usize, &str); 5] = [
            (&[0x02, 0x01, 0x06, 0x09, 0xff, 0x4c], 3, "past the end"),
            (
                &[0x04, 0x03, 0x0f, 0x18, 0x0a, 0x02, 0x01, 0x06],
                0,
                "whole number",
            ),
            (&[0x03, 0x0a, 0x01, 0x02], 0, "one octet"),
            (&[0x03, 0x09, 0xff, 0xfe], 0, "UTF-8"),
            (&[0x02, 0x01, 0x06, 0x00, 0x00, 0x07], 3, "not zero"),
        ];
        for (data, offset, why) in cases {
            let got = parse(data);
            match got.last() {
                Some(Structure::Malformed { offset: o, why: w }) => {
                    assert_eq!(*o, offset, "{data:02x?}: {got:?}");
                    assert!(w.contains(why), "{data:02x?}: {w}");
                }
                other => panic!("{data:02x?}: {other:?}"),
            }
        }
        // Zero padding after a zero length is the payload's end, not a fault.
        assert_eq!(parse(&[0x02, 0x01, 0x06, 0x00, 0x00]).len(), 1);
    }

    /// A type the registry names but this does not decode is kept, named; a
    /// code it does not list is kept by number.
    #[test]
    fn types_not_decoded_are_kept_and_named_where_the_registry_names_them() {
        let got = parse(&[0x03, 0x19, 0x41, 0x03, 0x02, 0x33, 0xaa]);
        assert_eq!(
            got[0],
            Structure::Ad {
                offset: 0,
                ad: Ad::Other {
                    code: 0x19,
                    data: vec![0x41, 0x03]
                }
            }
        );
        assert_eq!(type_name(0x19), Some("Appearance"));
        assert_eq!(type_name(0x33), None);
        assert!(matches!(
            got[1],
            Structure::Ad {
                ad: Ad::Other { code: 0x33, .. },
                ..
            }
        ));
    }

    /// Only the four types that carry advertising data are read as such.
    #[test]
    fn only_the_advertising_types_carry_advertising_data() {
        let payload = [0u8; 9];
        for t in [
            PduType::AdvInd,
            PduType::AdvNonconnInd,
            PduType::AdvScanInd,
            PduType::ScanRsp,
        ] {
            assert_eq!(adv_data(t, &payload).map(<[u8]>::len), Some(3), "{t:?}");
        }
        for t in [PduType::AdvDirectInd, PduType::ScanReq, PduType::ConnectInd] {
            assert_eq!(adv_data(t, &payload), None, "{t:?}");
        }
    }

    /// **An advertised name cannot command the terminal.** Escape and the
    /// rest of the control characters come out as U+FFFD; everything else,
    /// accents included, as sent.
    #[test]
    fn a_name_from_the_air_is_made_printable() {
        assert_eq!(printable("Café\u{1b}[2J\n"), "Café\u{fffd}[2J\u{fffd}");
        assert_eq!(printable("Mi Band 7"), "Mi Band 7");
    }

    /// The complete name wins over the shortened one, whichever came first.
    #[test]
    fn the_complete_name_is_preferred() {
        let got = parse(&[0x03, 0x08, b'A', b'b', 0x04, 0x09, b'A', b'b', b'c']);
        assert_eq!(name(&got), Some(("Abc", true)));
        assert_eq!(name(&parse(&[0x02, 0x01, 0x06])), None);
    }

    /// Defined bits by name, the rest by number, never a guessed name.
    #[test]
    fn flags_are_named_only_where_the_supplement_names_them() {
        assert_eq!(
            flag_names(0x06),
            ["LE General Discoverable", "BR/EDR Not Supported"]
        );
        assert_eq!(flag_names(0x41), ["LE Limited Discoverable", "bit 6"]);
        assert!(flag_names(0).is_empty());
    }
}
