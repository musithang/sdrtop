// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Print what a SoapySDR driver actually answers about its gain elements.
//!
//! A diagnostic, not part of the app. It exists because the gain chain is the
//! one place where sdrtop's behaviour depends on a driver's own answer rather
//! than on a table we wrote, and "the AMP is still a knob" is a claim about that
//! answer. Guessing which of `listGains`, `getGainElementRange` or the
//! whole-chain `getGainRange` is lying is not something to do from a screenshot.
//!
//! Run it against the same argument string sdrtop opens with:
//!
//! ```text
//! cargo run --example soapy_probe -- "driver=hackrf"
//! ```
//!
//! With no argument it enumerates and probes the first device it finds.

// The whole API surface is included so the probe asks exactly what the app
// asks, but it only calls a handful of it. The rest is not dead: it is the same
// file the app compiles.
#[path = "../src/hardware/soapy/api.rs"]
#[allow(dead_code)]
mod api;

fn main() {
    let args = std::env::args().nth(1).unwrap_or_default();
    let args = if args.is_empty() {
        match first_device() {
            Some(a) => {
                println!("no argument given, using the first device: {a}");
                a
            }
            None => {
                eprintln!("no argument given and SoapySDR found no device");
                std::process::exit(1);
            }
        }
    } else {
        args
    };

    let Some(api) = api::api() else {
        eprintln!("libSoapySDR is not available");
        std::process::exit(1);
    };

    println!("opening: {args}");
    let dev = match unsafe { api.make(&args) } {
        Ok(d) => d,
        Err(e) => {
            eprintln!("could not open: {e}");
            std::process::exit(1);
        }
    };

    let (whole_min, whole_max) = unsafe { api.gain_range(dev) };
    println!("\nwhole-chain getGainRange: [{whole_min}, {whole_max}]");

    let names = unsafe { api.gain_elements(dev) };
    println!("listGains: {names:?}");

    println!("\nper element:");
    for name in &names {
        let r = unsafe { api.gain_element_range(dev, name) };
        match r {
            Some(r) => {
                let positions = if r.step > 0.0 {
                    ((r.maximum - r.minimum) / r.step).floor() as i64 + 1
                } else {
                    0
                };
                let verdict = match positions {
                    2 => "SWITCH (boost)",
                    1 => "fixed",
                    0 => "continuous",
                    _ => "stage",
                };
                println!(
                    "  {name:<8} [{}, {}, step {}]  -> {positions} positions  {verdict}",
                    r.minimum, r.maximum, r.step
                );
            }
            None => println!("  {name:<8} <no range reported>"),
        }
    }

    println!("\nhasGainMode (AGC): {}", unsafe { api.has_gain_mode(dev) });

    // The question sdrtop actually asks when the step is missing: can this
    // element hold a value strictly between its bounds? A switch snaps to an
    // end, a continuous control keeps the middle. This is what decides whether
    // the AMP becomes the boost key or stays a 0-14 knob.
    println!("\nread-back probe (only meaningful when step is 0):");
    for name in &names {
        let Some(r) = (unsafe { api.gain_element_range(dev, name) }) else {
            continue;
        };
        if r.step > 0.0
            || !(r.maximum.is_finite() && r.minimum.is_finite())
            || r.maximum <= r.minimum
        {
            println!("  {name:<8} skipped (step present or range not a range)");
            continue;
        }
        let before = unsafe { api.gain_element(dev, name) };
        let middle = r.minimum + (r.maximum - r.minimum) / 2.0;
        let set = unsafe { api.set_gain_element(dev, name, middle) };
        let after = unsafe { api.gain_element(dev, name) };
        if let Some(b) = before {
            let _ = unsafe { api.set_gain_element(dev, name, b) };
        }
        match (set, after) {
            (Ok(()), Some(after)) => {
                let held = (after - middle).abs() < 1e-6;
                println!(
                    "  {name:<8} set {middle} -> read {after}  {}",
                    if held { "CONTINUOUS" } else { "SWITCH (boost)" }
                );
            }
            (Err(e), _) => println!("  {name:<8} set failed: {e}"),
            (_, None) => println!("  {name:<8} read-back failed"),
        }
    }

    unsafe { api.unmake(dev) };
}

/// The first device SoapySDR enumerates, as an open argument string.
fn first_device() -> Option<String> {
    let api = api::api()?;
    let found = api.enumerate(&[]);
    let first = found.first()?;
    let driver = first
        .iter()
        .find(|(k, _)| k == "driver")
        .map(|(_, v)| v.as_str())?;
    let mut out = format!("driver={driver}");
    for key in ["serial", "device_id"] {
        if let Some((_, v)) = first.iter().find(|(k, _)| k == key) {
            out.push_str(&format!(", {key}={v}"));
            break;
        }
    }
    Some(out)
}
