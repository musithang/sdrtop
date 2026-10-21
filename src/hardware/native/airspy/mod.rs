// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Airspy R2, driven through libairspy.
//!
//! Found and identified here so far; it is reached only when `--device
//! airspy` asks for it by name, and until it can stream it says so and points
//! at the SoapySDR path, which keeps offering the radio as before.
//!
//! **The R2, and only the R2.** The Airspy Mini answers on the same USB id
//! and board id; the rates its firmware lists tell them apart (R2: 10 and 2.5
//! Msps; Mini: 6 and 3). A Mini is named and not taken: nobody here has held
//! one, and its own rates and gains would be guesses.

pub mod ffi;

use std::ffi::CStr;

use libc::{c_char, c_int, c_void};

use crate::hardware::{DeviceKind, DeviceListing};

/// Which Airspy a firmware is, by the rates it offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    R2,
    Mini,
    /// Rates neither of the above lists.
    Unknown,
}

/// The model the firmware's rate list (complex rates, Hz) belongs to, in any
/// order.
pub fn model_from_rates(rates: &[u32]) -> Model {
    let mut sorted = rates.to_vec();
    sorted.sort_unstable();
    match sorted.as_slice() {
        [2_500_000, 10_000_000] => Model::R2,
        [3_000_000, 6_000_000] => Model::Mini,
        _ => Model::Unknown,
    }
}

/// What libairspy said about one radio.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// As SoapyAirspy spells it: lowercase hex, no prefix.
    pub serial: String,
    /// The board's name as libairspy gives it, when the board could be read.
    pub board: Option<String>,
    pub firmware: Option<String>,
    pub rates: Vec<u32>,
    pub model: Model,
}

impl Model {
    fn name(self) -> &'static str {
        match self {
            Model::R2 => "Airspy R2",
            Model::Mini => "Airspy Mini",
            Model::Unknown => "Airspy",
        }
    }
}

/// Every Airspy on the bus, each opened just long enough to identify it. A
/// radio that will not open (another app holding it) is left out rather than
/// described from nothing.
pub fn find(api: &ffi::AirspyApi) -> Vec<Found> {
    // SAFETY: a null buffer asks only for the count.
    let count = unsafe { (api.airspy_list_devices)(std::ptr::null_mut(), 0) };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    let mut serials = vec![0u64; count];
    // SAFETY: the buffer holds `count` serials, and the call writes at most
    // that many.
    let got = unsafe {
        (api.airspy_list_devices)(serials.as_mut_ptr(), c_int::try_from(count).unwrap_or(0))
    };
    serials.truncate(usize::try_from(got).unwrap_or(0));
    serials
        .into_iter()
        .filter_map(|serial| identify(api, serial))
        .collect()
}

/// One radio, opened by its serial, read and closed.
fn identify(api: &ffi::AirspyApi, serial: u64) -> Option<Found> {
    let mut device: *mut c_void = std::ptr::null_mut();
    // SAFETY: libairspy writes the handle it opened, or fails.
    if unsafe { (api.airspy_open_sn)(&mut device, serial) } != 0 || device.is_null() {
        return None;
    }
    // SAFETY: `device` is open until the close below; each call writes only
    // into the buffer it is given, of the size it is given.
    let found = unsafe {
        let mut id = 0u8;
        let board = ((api.airspy_board_id_read)(device, &mut id) == 0)
            .then(|| c_string((api.airspy_board_id_name)(c_int::from(id))))
            .flatten();
        let mut text = [0 as c_char; 128];
        let firmware = ((api.airspy_version_string_read)(device, text.as_mut_ptr(), 128) == 0)
            .then(|| c_string(text.as_ptr()))
            .flatten()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let mut n = 0u32;
        let rates = if (api.airspy_get_samplerates)(device, &mut n, 0) == 0 && n > 0 {
            let mut rates = vec![0u32; n as usize];
            if (api.airspy_get_samplerates)(device, rates.as_mut_ptr(), n) == 0 {
                rates
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        (api.airspy_close)(device);
        Found {
            serial: format!("{serial:x}"),
            board,
            firmware,
            model: model_from_rates(&rates),
            rates,
        }
    };
    Some(found)
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
            kind: DeviceKind::Airspy,
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
    let base = format!("{} · {}", found.model.name(), found.serial);
    match found.model {
        Model::R2 => base,
        Model::Mini => format!("{base} · not driven natively, use soapy=driver=airspy"),
        Model::Unknown => format!("{base} · rates not recognised, use soapy=driver=airspy"),
    }
}

/// Why a listing cannot be opened natively yet, and the road that works.
pub fn not_yet(listing: &DeviceListing) -> String {
    format!(
        "{}: sdrtop's own Airspy backend finds this radio but cannot stream from it yet.\n\
         Open it through SoapySDR meanwhile: sdrtop --device soapy=driver=airspy",
        listing.label
    )
}

fn c_string(ptr: *const c_char) -> Option<String> {
    // SAFETY: a NUL-terminated string libairspy returned or wrote.
    (!ptr.is_null()).then(|| {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TestLibrary;
    use super::*;

    /// Each model by the rates its firmware lists, whatever their order; a
    /// list that is neither is no model of ours.
    #[test]
    fn a_model_is_told_by_its_rates() {
        assert_eq!(model_from_rates(&[10_000_000, 2_500_000]), Model::R2);
        assert_eq!(model_from_rates(&[2_500_000, 10_000_000]), Model::R2);
        assert_eq!(model_from_rates(&[6_000_000, 3_000_000]), Model::Mini);
        assert_eq!(model_from_rates(&[10_000_000]), Model::Unknown);
        assert_eq!(model_from_rates(&[]), Model::Unknown);
    }

    #[test]
    fn loader_rejects_missing_required_symbol() {
        let fixture = TestLibrary::new(true);
        let err = match super::super::loader::load("libairspy", &[fixture.path()], ffi::resolve) {
            Ok(_) => panic!("accepted an incomplete libairspy"),
            Err(err) => err,
        };
        assert!(err.contains(fixture.path()));
        assert!(
            err.contains("missing required symbol airspy_get_samplerates"),
            "{err}"
        );
    }

    /// An R2 is found with its serial spelled as SoapyAirspy spells it, so
    /// the same radio reached both ways is one radio; its board, firmware
    /// and rates as the library reads them; and it is closed again.
    #[test]
    fn an_r2_is_found_and_identified() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        let found = find(&api);
        assert_eq!(
            found,
            vec![Found {
                serial: "637862dc2e1182d7".into(),
                board: Some("AIRSPY".into()),
                firmware: Some("AirSpy NOS v1.0.0-rc10-0-g946184a 2016-09-19".into()),
                rates: vec![10_000_000, 2_500_000],
                model: Model::R2,
            }]
        );
        assert_eq!(fixture.calls(3), 1, "opened to identify it, then closed");
    }

    /// A Mini is named as one; a radio another app holds is left out; no
    /// radio is no radio.
    #[test]
    fn a_mini_is_named_and_a_held_radio_left_out() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        fixture.mode(20);
        assert_eq!(find(&api)[0].model, Model::Mini);
        fixture.mode(21);
        assert_eq!(find(&api), vec![]);
        fixture.mode(22);
        assert_eq!(find(&api), vec![]);
    }

    /// The selector says which Airspy it is and by which serial; a Mini says
    /// it is not driven natively.
    #[test]
    fn the_label_names_the_model_and_serial() {
        let r2 = Found {
            serial: "637862dc2e1182d7".into(),
            board: Some("AIRSPY".into()),
            firmware: None,
            rates: vec![10_000_000, 2_500_000],
            model: Model::R2,
        };
        assert_eq!(label(&r2), "Airspy R2 · 637862dc2e1182d7");
        let mini = Found {
            model: Model::Mini,
            rates: vec![6_000_000, 3_000_000],
            ..r2
        };
        assert!(label(&mini).starts_with("Airspy Mini · 637862dc2e1182d7"));
        assert!(
            label(&mini).contains("not driven natively"),
            "{}",
            label(&mini)
        );
    }

    /// Until it streams, opening says so and names the road that works.
    #[test]
    fn opening_natively_says_it_is_not_finished_and_points_at_soapy() {
        let listing = DeviceListing {
            kind: DeviceKind::Airspy,
            index: 0,
            label: "Airspy R2 · 637862dc2e1182d7".into(),
            serial: Some("637862dc2e1182d7".into()),
            args: None,
            path: None,
            tiny_sa_input: None,
        };
        let why = not_yet(&listing);
        assert!(why.contains("Airspy R2 · 637862dc2e1182d7"), "{why}");
        assert!(why.contains("--device soapy=driver=airspy"), "{why}");
    }
}
