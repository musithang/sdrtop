// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The PlutoSky R2, driven through its own `iiod` server.
//!
//! Found and identified here, opened and described in [`device`] with its
//! transmitter dealt with in [`tx`], streamed in [`stream`]. Asked for by name
//! only (`--device plutosky`) until its rehearsal on the hardware.
//!
//! **No library between sdrtop and the radio.** The radio runs `iiod`, and
//! [`iiod`] speaks its text protocol over TCP. Over USB the radio is a small
//! network (RNDIS, or NCM on macOS) at the address its `config.txt` gives,
//! 192.168.2.1 unless changed.
//!
//! **The address is trusted only when the serials agree.** A PlutoSDR on the
//! USB bus says nothing about what answers at an address: another radio may
//! own it, on another cable or across a network. So the radio on the bus is
//! asked for its USB serial, the server at the address for its `hw_serial`,
//! and only a match makes the two one radio. Networks are never swept.
//!
//! **The R2, and only the R2.** The USB id is ADI's stock ADALM-PLUTO's, shared
//! by every board in the family, so it decides nothing. The radio's own
//! description does: `hw_model` exactly `Analog Devices PlutoSDR Rev.C
//! (Z7020-AD9361)` with an `ad9361` transceiver. That is the board held and
//! measured here; a stock Pluto or another clone is named by what it said and
//! left to SoapyPlutoSDR, whose road works for all of them.
//!
//! **Only checked servers.** `iiod` versions differ in what they accept; a
//! version nobody here has spoken to is refused by name ([`CHECKED_SERVERS`]).

pub mod context;
pub mod device;
pub mod iiod;
pub mod stream;
pub mod tx;

use std::sync::Mutex;
use std::time::Duration;

use crate::hardware::{usb, DeviceKind, DeviceListing};

/// Where the radio is on its USB network unless its `config.txt` says
/// otherwise.
pub const DEFAULT_ADDRESS: &str = "192.168.2.1";

/// `iiod` versions spoken to on the bench, as `VERSION`'s numbers.
pub const CHECKED_SERVERS: [&str; 1] = ["0.25"];

/// The model string the R2 reports, exactly.
const R2_MODEL: &str = "Analog Devices PlutoSDR Rev.C (Z7020-AD9361)";

/// Short, because the radio is a wire away: on its USB network a reply takes
/// well under a millisecond, and a `PRINT` of its 32 KB a few. A second for
/// each wait still leaves room for a busy radio, and keeps a search that finds
/// nothing at the address from holding up the device selector.
const SEARCH: iiod::Timeouts = iiod::Timeouts {
    connect: Duration::from_millis(1000),
    io: Duration::from_millis(2000),
};

/// Which board answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Model {
    R2,
    /// Another member of the family, by the model string it reported.
    Other(String),
}

/// What a radio that answered said about itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub model: Model,
    /// `hw_serial`, which over USB is also the radio's USB serial.
    pub serial: String,
    pub firmware: Option<String>,
    /// `iiod`'s version, `0.25`.
    pub server: String,
    /// Where it answered.
    pub address: String,
}

/// Why no radio was identified.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// The system could not list its USB devices.
    Usb(String),
    /// A PlutoSDR is on the bus but nothing answered at the address.
    NoAnswer { address: String, why: String },
    /// The server's version was never checked here.
    Server { address: String, version: String },
    /// The server answered for a radio other than the one on the bus.
    OtherRadio {
        address: String,
        usb: String,
        answered: Option<String>,
    },
    /// The description lacks what identifies the radio.
    Undescribed {
        address: String,
        missing: &'static str,
    },
}

impl Problem {
    pub fn text(&self) -> String {
        match self {
            Problem::Usb(why) => format!("PlutoSky: the USB devices could not be listed ({why})."),
            Problem::NoAnswer { address, why } => format!(
                "PlutoSky: a PlutoSDR is on USB, but nothing answered at {address} ({why}). \
                 It may still be starting (about half a minute after power-up), its USB \
                 network may be down, or its config.txt may give another address."
            ),
            Problem::Server { address, version } => format!(
                "PlutoSky: the radio at {address} runs iiod {version}; sdrtop has checked only {}. \
                 Open it through SoapySDR: sdrtop --device soapy=driver=plutosdr",
                CHECKED_SERVERS.join(", ")
            ),
            Problem::OtherRadio { address, usb, answered } => format!(
                "PlutoSky: the radio answering at {address} ({}) is not the PlutoSDR on USB ({usb}).",
                answered.as_deref().unwrap_or("no serial given")
            ),
            Problem::Undescribed { address, missing } => format!(
                "PlutoSky: the radio at {address} does not say its {missing}, so it cannot be \
                 told from another board."
            ),
        }
    }
}

/// Decides what answered, from its description and the serial the USB bus
/// gave.
pub fn identify(
    ctx: &context::Context,
    server: &iiod::Version,
    usb_serial: Option<&str>,
    address: &str,
) -> Result<Found, Problem> {
    let address = address.to_string();
    if !CHECKED_SERVERS.contains(&server.short().as_str()) {
        return Err(Problem::Server {
            address,
            version: server.short(),
        });
    }
    let answered = ctx.attribute("hw_serial");
    if let Some(usb) = usb_serial {
        if answered != Some(usb) {
            return Err(Problem::OtherRadio {
                address,
                usb: usb.to_string(),
                answered: answered.map(str::to_string),
            });
        }
    }
    let Some(serial) = answered else {
        return Err(Problem::Undescribed {
            address,
            missing: "serial",
        });
    };
    let Some(model) = ctx.attribute("hw_model") else {
        return Err(Problem::Undescribed {
            address,
            missing: "model",
        });
    };
    let r2 = model == R2_MODEL
        && ctx.attribute("ad9361-phy,model") == Some("ad9361")
        && ctx.has_device("cf-ad9361-lpc");
    Ok(Found {
        model: if r2 {
            Model::R2
        } else {
            Model::Other(model.to_string())
        },
        serial: serial.to_string(),
        firmware: ctx.attribute("fw_version").map(str::to_string),
        server: server.short(),
        address,
    })
}

/// Asks the server at `address` what it is.
pub fn find_at(
    address: &str,
    usb_serial: Option<&str>,
    t: iiod::Timeouts,
) -> Result<Found, Problem> {
    let no_answer = |e: iiod::Error| Problem::NoAnswer {
        address: address.to_string(),
        why: e.to_string(),
    };
    let mut client = iiod::Client::connect(address, t).map_err(no_answer)?;
    let version = client.version().map_err(no_answer)?;
    let xml = client.print().map_err(no_answer)?;
    client.exit();
    identify(&context::parse(&xml), &version, usb_serial, address)
}

static LAST_PROBLEM: Mutex<Option<String>> = Mutex::new(None);

/// Why the last search identified nothing, for the no-device hint; `None`
/// when it found a radio or found no PlutoSDR on the bus at all.
pub fn last_problem() -> Option<String> {
    LAST_PROBLEM
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// The device selector's entry for a PlutoSDR on the bus that answered at the
/// default address. Only when asked for by name: until the backend opens the
/// radio, offering it unasked would offer a refusal.
pub fn list(asked_by_name: bool) -> Vec<DeviceListing> {
    if !asked_by_name {
        return Vec::new();
    }
    let outcome = match usb::find(&usb::PLUTOSDR) {
        Err(why) => Err(Some(Problem::Usb(why))),
        Ok(None) => Err(None),
        Ok(Some(info)) => find_at(DEFAULT_ADDRESS, info.serial.as_deref(), SEARCH).map_err(Some),
    };
    let mut last = LAST_PROBLEM.lock().unwrap_or_else(|e| e.into_inner());
    match outcome {
        Ok(found) => {
            *last = None;
            vec![DeviceListing {
                kind: DeviceKind::PlutoSky,
                index: 0,
                label: label(&found),
                serial: Some(found.serial),
                args: None,
                path: None,
                tiny_sa_input: None,
            }]
        }
        Err(problem) => {
            *last = problem.map(|p| p.text());
            Vec::new()
        }
    }
}

fn label(found: &Found) -> String {
    let firmware = found.firmware.as_deref().unwrap_or("firmware not said");
    match &found.model {
        Model::R2 => format!(
            "PlutoSky R2 \u{00b7} {} \u{00b7} {firmware} \u{00b7} iiod {}",
            found.serial, found.server
        ),
        Model::Other(model) => format!(
            "{model} \u{00b7} {} \u{00b7} not a PlutoSky R2: sdrtop --device soapy=driver=plutosdr",
            found.serial
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::iiod::fake;
    use super::*;

    const R2_XML: &str = include_str!("../fixtures/pluto-r2-context.xml");
    const R2_SERIAL: &str = "1044730231960000000000000000000001";

    fn v025() -> iiod::Version {
        iiod::Version::parse("0.25.v0.25  ").unwrap()
    }

    fn quick() -> iiod::Timeouts {
        iiod::Timeouts {
            connect: Duration::from_millis(500),
            io: Duration::from_millis(300),
        }
    }

    fn r2_script() -> fake::Radio {
        fake::Radio {
            version: "0.25.v0.25  \n".into(),
            xml: R2_XML.into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_r2_is_recognised_when_the_serials_agree() {
        let f = identify(&context::parse(R2_XML), &v025(), Some(R2_SERIAL), "a").unwrap();
        assert_eq!(f.model, Model::R2);
        assert_eq!(f.serial, R2_SERIAL);
        assert_eq!(f.firmware.as_deref(), Some("v0.38"));
        assert_eq!(f.server, "0.25");
    }

    /// The address belongs to whoever answers there; a radio with another
    /// serial is another radio, however alike.
    #[test]
    fn another_serial_at_the_address_is_another_radio() {
        let p = identify(&context::parse(R2_XML), &v025(), Some("deadbeef"), "a").unwrap_err();
        assert!(matches!(p, Problem::OtherRadio { .. }), "{p:?}");
        assert!(p.text().contains("deadbeef"), "{}", p.text());
    }

    /// A stock Pluto answers on the same USB id; it is named, not taken.
    #[test]
    fn another_board_is_named_by_what_it_said() {
        let stock = R2_XML.replace(R2_MODEL, "Analog Devices PlutoSDR Rev.B (Z7010-AD9363A)");
        let f = identify(&context::parse(&stock), &v025(), Some(R2_SERIAL), "a").unwrap();
        assert_eq!(
            f.model,
            Model::Other("Analog Devices PlutoSDR Rev.B (Z7010-AD9363A)".into())
        );
        let l = label(&f);
        assert!(l.contains("not a PlutoSky R2"), "{l}");
        assert!(l.contains("--device soapy=driver=plutosdr"), "{l}");
    }

    /// The R2's model string with another transceiver is not the board that
    /// was measured.
    #[test]
    fn the_model_string_alone_is_not_enough() {
        let other_chip = R2_XML.replace(
            "name=\"ad9361-phy,model\" value=\"ad9361\"",
            "name=\"ad9361-phy,model\" value=\"ad9363a\"",
        );
        let f = identify(&context::parse(&other_chip), &v025(), Some(R2_SERIAL), "a").unwrap();
        assert!(matches!(f.model, Model::Other(_)));
    }

    #[test]
    fn an_unchecked_server_is_refused_by_name() {
        let v = iiod::Version::parse("0.26.v0.26").unwrap();
        let p = identify(&context::parse(R2_XML), &v, Some(R2_SERIAL), "a").unwrap_err();
        assert_eq!(
            p,
            Problem::Server {
                address: "a".into(),
                version: "0.26".into()
            }
        );
        assert!(p.text().contains("checked only 0.25"), "{}", p.text());
    }

    #[test]
    fn a_description_without_a_serial_is_refused() {
        let p = identify(&context::parse("<context/>"), &v025(), None, "a").unwrap_err();
        assert!(matches!(
            p,
            Problem::Undescribed {
                missing: "serial",
                ..
            }
        ));
    }

    /// End to end against the stand-in server: the R2's recorded answers in,
    /// the R2 out.
    #[test]
    fn the_r2_is_found_at_its_address() {
        let (addr, _) = fake::serve(r2_script());
        let f = find_at(&addr, Some(R2_SERIAL), quick()).unwrap();
        assert_eq!(f.model, Model::R2);
        assert_eq!(f.address, addr);
        let l = label(&f);
        assert_eq!(
            l,
            format!("PlutoSky R2 \u{00b7} {R2_SERIAL} \u{00b7} v0.38 \u{00b7} iiod 0.25")
        );
    }

    #[test]
    fn a_silent_radio_is_a_problem_said_not_a_hang() {
        let (addr, _) = fake::serve(fake::Radio {
            silent: true,
            ..r2_script()
        });
        let p = find_at(&addr, Some(R2_SERIAL), quick()).unwrap_err();
        assert!(matches!(p, Problem::NoAnswer { .. }), "{p:?}");
        assert!(p.text().contains("nothing answered"), "{}", p.text());
    }
}
