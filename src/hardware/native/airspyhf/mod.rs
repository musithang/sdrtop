// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Airspy HF+ Discovery, driven through libairspyhf.
//!
//! Found and identified here, for now only when `--device airspyhf` names
//! this backend: opening it is refused until the backend can stream.
//!
//! **The Discovery, and only the Discovery.** The HF+ Dual Port and the
//! Ranger answer on the same USB id and through the same library; the part id
//! their firmware reports tells them apart (1, 2, 3). Only the Discovery has
//! been held and measured here, so the others are named and not taken: their
//! front ends, attenuators and rates would be guesses.

pub mod ffi;

use std::ffi::CStr;

use libc::{c_char, c_int, c_void};

use crate::hardware::{DeviceKind, DeviceListing};

/// Which HF+ a firmware is, by the part id it reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Board {
    DualPort,
    Discovery,
    Ranger,
    /// A part id none of the above has.
    Unknown,
}

impl Board {
    pub fn from_part_id(id: u32) -> Self {
        match id {
            1 => Board::DualPort,
            2 => Board::Discovery,
            3 => Board::Ranger,
            _ => Board::Unknown,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Board::DualPort => "Airspy HF+ Dual Port",
            Board::Discovery => "Airspy HF+ Discovery",
            Board::Ranger => "Airspy Ranger",
            Board::Unknown => "Airspy HF+",
        }
    }
}

/// What libairspyhf said about one radio.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// As SoapyAirspyHF spells it: lowercase hex, no prefix.
    pub serial: String,
    pub board: Board,
    pub firmware: Option<String>,
    pub rates: Vec<u32>,
    /// Listed on the bus but would not open: another program has claimed it,
    /// or this user may not open it. Nothing else is known about it.
    pub held: bool,
}

/// Every HF+ on the bus, each opened just long enough to identify it. A radio
/// that is listed but will not open is kept, held and described by its serial
/// alone.
pub fn find(api: &ffi::AirspyHfApi) -> Vec<Found> {
    // SAFETY: a null buffer asks only for the count.
    let count = unsafe { (api.airspyhf_list_devices)(std::ptr::null_mut(), 0) };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    let mut serials = vec![0u64; count];
    // SAFETY: the buffer holds `count` serials, and the call writes at most
    // that many.
    let got = unsafe {
        (api.airspyhf_list_devices)(serials.as_mut_ptr(), c_int::try_from(count).unwrap_or(0))
    };
    serials.truncate(usize::try_from(got).unwrap_or(0));
    serials
        .into_iter()
        .map(|serial| identify(api, serial))
        .collect()
}

/// One radio, opened by its serial, read and closed; or, when it would not
/// open, held.
fn identify(api: &ffi::AirspyHfApi, serial: u64) -> Found {
    let mut device: *mut c_void = std::ptr::null_mut();
    // SAFETY: libairspyhf writes the handle it opened, or fails.
    if unsafe { (api.airspyhf_open_sn)(&mut device, serial) } != 0 || device.is_null() {
        return Found {
            serial: format!("{serial:x}"),
            board: Board::Unknown,
            firmware: None,
            rates: Vec::new(),
            held: true,
        };
    }
    // SAFETY: `device` was opened just above and is closed once read.
    let found = unsafe { read_found(api, device, serial) };
    // SAFETY: as above.
    unsafe { (api.airspyhf_close)(device) };
    found
}

/// What an open radio says about itself. A board whose part id cannot be read
/// is `Unknown`, and so not taken.
///
/// # Safety
/// `device` must be a handle libairspyhf opened and has not closed.
unsafe fn read_found(api: &ffi::AirspyHfApi, device: *mut c_void, serial: u64) -> Found {
    // SAFETY: each call writes only into the buffer it is given, of the size
    // it is given; the version buffer is the size libairspyhf copies from.
    unsafe {
        let mut part = ffi::PartSerial::default();
        let board = if (api.airspyhf_board_partid_serialno_read)(device, &mut part) == 0 {
            Board::from_part_id(part.part_id)
        } else {
            Board::Unknown
        };
        let mut text = [0 as c_char; ffi::VERSION_STRING_SIZE as usize];
        let firmware = ((api.airspyhf_version_string_read)(
            device,
            text.as_mut_ptr(),
            ffi::VERSION_STRING_SIZE,
        ) == 0)
            .then(|| text_of(&text))
            .flatten();
        let mut n = 0u32;
        let rates = if (api.airspyhf_get_samplerates)(device, &mut n, 0) == 0 && n > 0 {
            let mut rates = vec![0u32; n as usize];
            if (api.airspyhf_get_samplerates)(device, rates.as_mut_ptr(), n) == 0 {
                rates
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        Found {
            serial: format!("{serial:x}"),
            board,
            firmware,
            rates,
            held: false,
        }
    }
}

/// The text up to the first NUL of a buffer libairspyhf filled, trimmed;
/// `None` when that is empty.
fn text_of(buf: &[c_char]) -> Option<String> {
    // SAFETY: c_char and u8 have the same size and alignment.
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len()) };
    let text = CStr::from_bytes_until_nul(bytes).ok()?.to_string_lossy();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The device selector's entries for what [`find`] returned.
pub fn list() -> Vec<DeviceListing> {
    let Ok(api) = ffi::api() else {
        return Vec::new();
    };
    find(api)
        .into_iter()
        .enumerate()
        .map(|(index, found)| DeviceListing {
            kind: DeviceKind::AirspyHf,
            index,
            label: label(&found),
            serial: Some(found.serial),
            args: None,
            path: None,
            tiny_sa_input: None,
        })
        .collect()
}

fn label(found: &Found) -> String {
    if found.held {
        return format!(
            "{} · {} · in use by another program, or not ours to open",
            Board::Unknown.name(),
            found.serial
        );
    }
    let base = format!("{} · {}", found.board.name(), found.serial);
    match found.board {
        Board::Discovery => base,
        _ => format!("{base} · not driven natively: nobody here has held one"),
    }
}

/// Why a listed radio does not open yet.
pub fn not_yet(listing: &DeviceListing) -> anyhow::Error {
    anyhow::anyhow!(
        "{}: sdrtop's own Airspy HF+ backend finds this radio but cannot open it yet.",
        listing.label
    )
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TestLibrary;
    use super::*;

    #[test]
    fn loader_rejects_missing_required_symbol() {
        let fixture = TestLibrary::new(true);
        let err = match super::super::loader::load("libairspyhf", &[fixture.path()], ffi::resolve) {
            Ok(_) => panic!("accepted an incomplete libairspyhf"),
            Err(err) => err,
        };
        assert!(err.contains(fixture.path()));
        assert!(
            err.contains("missing required symbol airspyhf_get_samplerates"),
            "{err}"
        );
    }

    /// The part ids the firmware reports, each its own board; anything else
    /// is no board of ours.
    #[test]
    fn a_board_is_told_by_its_part_id() {
        assert_eq!(Board::from_part_id(1), Board::DualPort);
        assert_eq!(Board::from_part_id(2), Board::Discovery);
        assert_eq!(Board::from_part_id(3), Board::Ranger);
        assert_eq!(Board::from_part_id(0), Board::Unknown);
        assert_eq!(Board::from_part_id(0xff), Board::Unknown);
    }

    /// A Discovery is found with its serial spelled as SoapyAirspyHF spells
    /// it, so the same radio reached both ways is one radio; its board,
    /// firmware and rates as the library reads them; and it is closed again.
    #[test]
    fn a_discovery_is_found_and_identified() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        assert_eq!(
            find(&api),
            vec![Found {
                serial: "dd528080ff444630".into(),
                board: Board::Discovery,
                firmware: Some("R5.0.1-CD".into()),
                rates: vec![912_000, 768_000, 650_000, 456_000, 384_000, 228_000, 192_000],
                held: false,
            }]
        );
        assert_eq!(fixture.calls(3), 1, "opened to identify it, then closed");
        // libairspyhf's own buffer is 64 bytes (`MAX_VERSION_STRING_SIZE`),
        // a fact of the library, so it is written out here rather than read
        // from the constant under test.
        assert!(
            (1..=64).contains(&fixture.airspyhf_state(0)),
            "asked for no more of the version than libairspyhf holds: {}",
            fixture.airspyhf_state(0)
        );
    }

    /// Another board is named as itself; a radio another program holds is
    /// kept by its serial alone; no radio is no radio.
    #[test]
    fn another_board_is_named_and_a_held_radio_kept() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        fixture.mode(30);
        assert_eq!(find(&api)[0].board, Board::Ranger);
        fixture.mode(31);
        assert_eq!(
            find(&api),
            vec![Found {
                serial: "dd528080ff444630".into(),
                board: Board::Unknown,
                firmware: None,
                rates: vec![],
                held: true,
            }]
        );
        fixture.mode(32);
        assert_eq!(find(&api), vec![]);
    }

    /// The selector says which HF+ it is and by which serial; another board
    /// says it is not driven natively, and a held one that it is held.
    #[test]
    fn the_label_names_the_board_and_serial() {
        let discovery = Found {
            serial: "dd528080ff444630".into(),
            board: Board::Discovery,
            firmware: None,
            rates: vec![768_000],
            held: false,
        };
        assert_eq!(label(&discovery), "Airspy HF+ Discovery · dd528080ff444630");
        let ranger = Found {
            board: Board::Ranger,
            ..discovery.clone()
        };
        assert!(
            label(&ranger).starts_with("Airspy Ranger · dd528080ff444630 · not driven natively"),
            "{}",
            label(&ranger)
        );
        let held = Found {
            board: Board::Unknown,
            held: true,
            ..discovery
        };
        assert!(label(&held).contains("in use by another program"));
    }

    /// The C structs have the layout libairspyhf writes into.
    #[test]
    fn the_structs_match_the_c_layout() {
        let fixture = TestLibrary::new(false);
        assert_eq!(
            fixture.airspyhf_layout(0),
            std::mem::size_of::<ffi::PartSerial>()
        );
        assert_eq!(
            fixture.airspyhf_layout(1),
            std::mem::offset_of!(ffi::PartSerial, serial_no)
        );
    }

    /// Opening is refused by name, with the radio's own label.
    #[test]
    fn opening_is_refused_with_the_radios_label() {
        let listing = DeviceListing {
            kind: DeviceKind::AirspyHf,
            index: 0,
            label: "Airspy HF+ Discovery · dd528080ff444630".into(),
            serial: Some("dd528080ff444630".into()),
            args: None,
            path: None,
            tiny_sa_input: None,
        };
        let why = not_yet(&listing).to_string();
        assert!(
            why.starts_with("Airspy HF+ Discovery · dd528080ff444630: "),
            "{why}"
        );
        assert!(why.contains("cannot open it yet"), "{why}");
    }
}
