// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The SDRplay RSP1B, driven through SDRplay's API.
//!
//! Found and identified here; not yet opened. Asked for by name only
//! (`--device sdrplay`), which finds the radio, says what it is and points at
//! the SoapySDR road that streams it today ([`not_yet`]).
//!
//! **A service sits between sdrtop and the radio.** SDRplay's API is a
//! client of `sdrplay_apiService`, which owns the USB device. Without the
//! service, `sdrplay_api_Open` fails and nothing about the radio can be
//! asked; sdrtop never starts or stops the service, only says that it is
//! needed. A radio another program has selected is simply absent from the
//! API's list.
//!
//! **Only checked API versions.** The API's structs are written in place, so
//! a version whose layout was not checked is refused by name before anything
//! is listed ([`ffi::CHECKED_VERSIONS`]).
//!
//! **The RSP1B, and only the RSP1B.** The API serves every RSP, each telling
//! itself apart by a hardware id. Only the RSP1B has been held and measured
//! here; the RSP1A is close kin, but its front end, LNA tables and rates would
//! still be guesses, so every other RSP is named and left to SoapySDRPlay3.

pub mod ffi;
pub mod layout;

use std::ffi::CStr;
use std::sync::Mutex;

use crate::hardware::{DeviceKind, DeviceListing};

/// Which RSP an entry is, by the hardware id the API reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Rsp1,
    Rsp1a,
    Rsp1b,
    Rsp2,
    RspDuo,
    RspDx,
    RspDxR2,
    /// An id none of the above has.
    Unknown(u8),
}

impl Model {
    /// The ids of `sdrplay_api.h` 3.15 (`SDRPLAY_RSP1_ID` and the rest).
    pub fn from_hw_ver(id: u8) -> Self {
        match id {
            1 => Model::Rsp1,
            255 => Model::Rsp1a,
            6 => Model::Rsp1b,
            2 => Model::Rsp2,
            3 => Model::RspDuo,
            4 => Model::RspDx,
            7 => Model::RspDxR2,
            other => Model::Unknown(other),
        }
    }

    fn name(self) -> String {
        match self {
            Model::Rsp1 => "SDRplay RSP1".into(),
            Model::Rsp1a => "SDRplay RSP1A".into(),
            Model::Rsp1b => "SDRplay RSP1B".into(),
            Model::Rsp2 => "SDRplay RSP2".into(),
            Model::RspDuo => "SDRplay RSPduo".into(),
            Model::RspDx => "SDRplay RSPdx".into(),
            Model::RspDxR2 => "SDRplay RSPdx-R2".into(),
            Model::Unknown(id) => format!("SDRplay RSP (hardware id {id})"),
        }
    }
}

/// One radio as the API lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// As the API spells it, which is also how SoapySDRPlay3 spells it
    /// (`2402040E60`).
    pub serial: String,
    pub model: Model,
}

/// Why the API could not be asked for its radios.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// `sdrplay_api_Open` failed, with the API's name for the error.
    Open(String),
    /// The API is a version whose layout sdrtop has not checked.
    Version(String),
    /// Listing the radios failed, with the API's name for the error.
    List(String),
}

impl Problem {
    pub fn text(&self) -> String {
        match self {
            // When the service is not running, Open answers sdrplay_api_Fail,
            // not ServiceNotResponding (measured with the service stopped):
            // so the text says the service is needed, not that it is down.
            Problem::Open(err) => format!(
                "SDRplay's API would not open ({err}). It needs SDRplay's service running: \
                 on Linux, sudo systemctl start sdrplay."
            ),
            Problem::Version(v) => format!(
                "SDRplay's API is version {v}; sdrtop has checked the layout of {} only, so it \
                 does not use {v}. Install SDRplay's API {}, or use the SoapySDR road: \
                 sdrtop --device soapy=driver=sdrplay",
                ffi::CHECKED_VERSIONS.join(", "),
                ffi::CHECKED_VERSIONS.join(" or ")
            ),
            Problem::List(err) => format!("SDRplay's API would not list its radios ({err})."),
        }
    }
}

/// Every RSP the API lists. Opens the API, checks its version, lists under
/// the device lock and closes the API again on every path that opened it.
pub fn find(api: &ffi::SdrplayApi) -> Result<Vec<Found>, Problem> {
    // SAFETY: a call with no arguments; the session it opens is closed below.
    let opened = unsafe { (api.open)() };
    if opened != ffi::SUCCESS {
        return Err(Problem::Open(api.error_name(opened)));
    }
    let result = list_opened(api);
    // SAFETY: closes the session opened above.
    unsafe { (api.close)() };
    result
}

/// The version check and the listing, inside a session [`find`] opened.
fn list_opened(api: &ffi::SdrplayApi) -> Result<Vec<Found>, Problem> {
    let mut version = 0f32;
    // SAFETY: writes one float.
    let asked = unsafe { (api.api_version)(&mut version) };
    if asked != ffi::SUCCESS {
        return Err(Problem::Open(api.error_name(asked)));
    }
    if !ffi::is_checked(version) {
        return Err(Problem::Version(ffi::version_text(version)));
    }
    let mut devices = [layout::DeviceT::default(); layout::MAX_DEVICES];
    let mut count = 0u32;
    // SAFETY: the lock is released below on every path; GetDevices writes at
    // most `MAX_DEVICES` entries into an array of that many and their number
    // into `count`.
    let listed = unsafe {
        (api.lock_device_api)();
        let listed =
            (api.get_devices)(devices.as_mut_ptr(), &mut count, layout::MAX_DEVICES as u32);
        (api.unlock_device_api)();
        listed
    };
    if listed != ffi::SUCCESS {
        return Err(Problem::List(api.error_name(listed)));
    }
    let count = (count as usize).min(layout::MAX_DEVICES);
    Ok(devices[..count]
        .iter()
        .map(|d| Found {
            serial: serial_of(&d.ser_no),
            model: Model::from_hw_ver(d.hw_ver),
        })
        .collect())
}

/// The serial up to its first NUL; the whole buffer when there is none.
fn serial_of(buf: &[libc::c_char; layout::SERIAL_LEN]) -> String {
    // SAFETY: c_char and u8 have the same size and alignment.
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len()) };
    match CStr::from_bytes_until_nul(bytes) {
        Ok(text) => text.to_string_lossy().trim().to_string(),
        Err(_) => String::from_utf8_lossy(bytes).trim().to_string(),
    }
}

/// What the last search could not do, kept so the no-device hint can say it
/// without opening the API a second time: with the service down, opening it
/// makes the library print to the terminal, where the hint is being written.
static LAST_PROBLEM: Mutex<Option<Problem>> = Mutex::new(None);

/// Why the last [`list`] found nothing to offer, when the API itself was the
/// reason.
pub fn last_problem() -> Option<String> {
    LAST_PROBLEM
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Problem::text)
}

/// The device selector's entries: every RSP the API lists, but only when
/// `--device sdrplay` named this backend, until it can open the radio.
pub fn list(asked_by_name: bool) -> Vec<DeviceListing> {
    if !asked_by_name {
        return Vec::new();
    }
    let Ok(api) = ffi::api() else {
        return Vec::new();
    };
    let found = find(api);
    let (found, problem) = match found {
        Ok(found) => (found, None),
        Err(problem) => (Vec::new(), Some(problem)),
    };
    *LAST_PROBLEM.lock().unwrap_or_else(|e| e.into_inner()) = problem;
    found
        .into_iter()
        .enumerate()
        .map(|(index, found)| DeviceListing {
            kind: DeviceKind::Sdrplay,
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
        Model::Rsp1b => base,
        _ => format!("{base} · not driven natively: nobody here has held one"),
    }
}

/// Why `--device sdrplay` does not open a radio yet, and the road that
/// streams it meanwhile.
pub fn not_yet(listing: &DeviceListing) -> anyhow::Error {
    anyhow::anyhow!(
        "{}: sdrtop's own SDRplay backend finds this radio but cannot open it yet. \
         Open it through SoapySDR meanwhile: sdrtop --device soapy=driver=sdrplay",
        listing.label
    )
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TestLibrary;
    use super::*;

    /// What the fixture's SDRplay API records, by field (see
    /// `fixture_sdrplay_state` in the fixture).
    const OPENS: i32 = 0;
    const CLOSES: i32 = 1;
    const LOCKS: i32 = 2;
    const UNLOCKS: i32 = 3;
    const MAX_ASKED: i32 = 4;

    #[test]
    fn loader_rejects_missing_required_symbol() {
        let fixture = TestLibrary::new(true);
        let err =
            match super::super::loader::load("libsdrplay_api", &[fixture.path()], ffi::resolve) {
                Ok(_) => panic!("accepted an incomplete libsdrplay_api"),
                Err(err) => err,
            };
        assert!(err.contains(fixture.path()));
        assert!(
            err.contains("missing required symbol sdrplay_api_GetErrorString"),
            "{err}"
        );
    }

    /// The ids of the 3.15 header, each its own RSP; anything else is named
    /// by its number.
    #[test]
    fn an_rsp_is_told_by_its_hardware_id() {
        assert_eq!(Model::from_hw_ver(6), Model::Rsp1b);
        assert_eq!(Model::from_hw_ver(255), Model::Rsp1a);
        assert_eq!(Model::from_hw_ver(1), Model::Rsp1);
        assert_eq!(Model::from_hw_ver(7), Model::RspDxR2);
        assert_eq!(Model::from_hw_ver(9), Model::Unknown(9));
    }

    /// The RSP1B is found with its serial as the API spells it; the API is
    /// opened once and closed once, the device lock taken and given back, and
    /// GetDevices told how many entries it may fill.
    #[test]
    fn an_rsp1b_is_found_and_the_session_closed() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        assert_eq!(
            find(&api),
            Ok(vec![Found {
                serial: "2402040E60".into(),
                model: Model::Rsp1b,
            }])
        );
        assert_eq!(fixture.sdrplay_state(OPENS), 1);
        assert_eq!(fixture.sdrplay_state(CLOSES), 1, "every Open is closed");
        assert_eq!(fixture.sdrplay_state(LOCKS), 1);
        assert_eq!(fixture.sdrplay_state(UNLOCKS), 1, "the lock is given back");
        assert_eq!(fixture.sdrplay_state(MAX_ASKED), 16);
    }

    /// With the service down Open fails, the search says the service is
    /// needed and nothing is closed that was not opened.
    #[test]
    fn a_failed_open_names_the_service_and_closes_nothing() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        fixture.mode(41);
        let problem = find(&api).unwrap_err();
        assert_eq!(problem, Problem::Open("sdrplay_api_Fail".into()));
        assert!(problem.text().contains("systemctl start sdrplay"));
        assert_eq!(fixture.sdrplay_state(CLOSES), 0);
    }

    /// An API whose layout was not checked is refused by its version, before
    /// anything is listed, and the session it opened is closed.
    #[test]
    fn an_unchecked_api_version_is_refused_by_name() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        fixture.mode(42);
        let problem = find(&api).unwrap_err();
        assert_eq!(problem, Problem::Version("3.14".into()));
        let text = problem.text();
        assert!(text.contains("version 3.14"), "{text}");
        assert!(text.contains("3.15"), "{text}");
        assert!(text.contains("--device soapy=driver=sdrplay"), "{text}");
        assert_eq!(fixture.sdrplay_state(LOCKS), 0, "nothing listed");
        assert_eq!(fixture.sdrplay_state(CLOSES), 1);
    }

    /// Another RSP is named as itself; no radio is no radio.
    #[test]
    fn another_rsp_is_named_and_none_is_none() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        fixture.mode(43);
        assert_eq!(find(&api).unwrap()[0].model, Model::Rsp1a);
        fixture.mode(44);
        assert_eq!(find(&api), Ok(vec![]));
        assert_eq!(
            fixture.sdrplay_state(UNLOCKS),
            fixture.sdrplay_state(LOCKS),
            "every lock given back"
        );
    }

    /// The selector says which RSP it is and by which serial; another RSP
    /// says it is not driven natively.
    #[test]
    fn the_label_names_the_model_and_serial() {
        let rsp1b = Found {
            serial: "2402040E60".into(),
            model: Model::Rsp1b,
        };
        assert_eq!(label(&rsp1b), "SDRplay RSP1B · 2402040E60");
        let rsp1a = Found {
            model: Model::Rsp1a,
            ..rsp1b
        };
        assert!(
            label(&rsp1a).starts_with("SDRplay RSP1A · 2402040E60 · not driven natively"),
            "{}",
            label(&rsp1a)
        );
    }

    /// Until the backend opens the radio, asking for it by name says so and
    /// names the road that works.
    #[test]
    fn opening_is_refused_with_the_soapy_road() {
        let listing = DeviceListing {
            kind: DeviceKind::Sdrplay,
            index: 0,
            label: "SDRplay RSP1B · 2402040E60".into(),
            serial: Some("2402040E60".into()),
            args: None,
            path: None,
            tiny_sa_input: None,
        };
        let why = not_yet(&listing).to_string();
        assert!(why.starts_with("SDRplay RSP1B · 2402040E60: "), "{why}");
        assert!(why.contains("--device soapy=driver=sdrplay"), "{why}");
    }

    /// Unasked, nothing is listed and the API is not even opened.
    #[test]
    fn unasked_the_backend_lists_nothing() {
        assert!(list(false).is_empty());
    }
}
