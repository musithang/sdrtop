// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The `.sigmf-meta` file: SigMF 1.2.6, with sdrtop's own fields in the
//! `sdrtop` extension namespace.
//!
//! Every field name used here is either one the specification defines in
//! `core` (read in the specification, sections 1.10 to 1.12) or one
//! `user_docs/sdrtop.sigmf-ext.md` defines, and a test holds the two lists to
//! each other: SigMF 1.9.2 requires an extension to be written down, and a
//! description typed beside the code is only trusted while something checks
//! it.

use serde_json::{json, Map, Value};

use super::recording::{Recording, Stop};
use crate::hardware::{AcquisitionKind, DeviceCapabilities, SampleFormat, StageReading};
use crate::state::SdrMetrics;

/// The SigMF version the files follow.
pub const SIGMF_VERSION: &str = "1.2.6";
/// The version of `user_docs/sdrtop.sigmf-ext.md`.
pub const EXTENSION_VERSION: &str = "1.0.0";

/// What is true of the whole recording, taken once when it starts.
#[derive(Clone, Debug)]
pub struct Header {
    pub datatype: &'static str,
    pub sample_rate: f64,
    pub bytes_per_pair: u64,
    pub hw: String,
    pub description: String,
    pub full_scale: f32,
    pub bits: u8,
    pub stack: Option<String>,
    pub gain: String,
    pub reference: String,
    pub iq_correction: String,
}

/// SigMF's name for one of sdrtop's sample formats (1.8).
pub fn datatype(format: SampleFormat) -> &'static str {
    match format {
        SampleFormat::Int8 => "ci8",
        SampleFormat::Uint8 => "cu8",
        SampleFormat::Int16 => "ci16_le",
    }
}

/// The gain chain by stage name, as the config saves it, and the boost when
/// the radio has one. A stage whose dB was not measured at `hz` is written as
/// its step (`LNA=step 14/14`), as the screen shows it.
pub fn gain_text(caps: &DeviceCapabilities, gains: &[f64], boost: bool, hz: u64) -> String {
    let named = caps
        .gain
        .stages()
        .iter()
        .enumerate()
        .map(|(i, spec)| {
            let db = gains.get(i).copied().unwrap_or(spec.min_db);
            match spec.reading(db, hz) {
                StageReading::Position { at, of } => format!("{}=step {at}/{of}", spec.name),
                StageReading::Db(_) => crate::hardware::gain::format_named(
                    std::slice::from_ref(spec),
                    std::slice::from_ref(&db),
                ),
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    match caps.gain.boost() {
        Some(_) => format!(
            "{named}, {} {}",
            caps.gain.boost_label(),
            if boost { "on" } else { "off" }
        ),
        None => named,
    }
}

impl Header {
    /// The header for a recording starting now, or why there cannot be one.
    pub fn from_state(state: &SdrMetrics) -> Result<Header, String> {
        if state.caps.acquisition != AcquisitionKind::IqSamples {
            return Err("this radio delivers power traces, not IQ samples".to_string());
        }
        let geometry = state.caps.sample_geometry;
        let cal = &state.iq.cal;
        let iq_correction = match (cal.dc_block_on, cal.cal_applied) {
            (false, false) => "none".to_string(),
            (dc, iq) => format!(
                "{}{}{}: on screen, not in this file, which holds the samples as the radio delivered them",
                if dc { "DC block" } else { "" },
                if dc && iq { " and " } else { "" },
                if iq { "I/Q amplitude and phase correction" } else { "" },
            ),
        };
        Ok(Header {
            datatype: datatype(geometry.format),
            sample_rate: state.radio.config_sample_rate,
            bytes_per_pair: geometry.bytes_per_pair() as u64,
            hw: format!(
                "{}, serial {}",
                state.system.board_name, state.system.serial
            ),
            description: format!(
                "recorded by sdrtop from the {} layout",
                state.ui.active_preset
            ),
            full_scale: geometry.full_scale,
            bits: geometry.bits(),
            stack: state
                .system
                .stack
                .as_ref()
                .map(|s| format!("{} {}", s.label.trim(), s.value)),
            gain: gain_text(
                &state.caps,
                &state.radio.gains,
                state.radio.amp_enabled,
                state.radio.frequency,
            ),
            reference: crate::export::provenance::reference_line(state, std::time::Instant::now()),
            iq_correction,
        })
    }
}

/// `2026-09-27T10:15:00.123456Z`: RFC 3339 with the `Z` offset, the only one
/// SigMF allows (1.11.2), to the microsecond.
pub fn datetime(unix: f64) -> String {
    let secs = unix.floor();
    let micros = ((unix - secs) * 1e6).round().min(999_999.0) as u64;
    let whole = crate::export::provenance::iso8601(secs as i64);
    format!("{}.{micros:06}Z", whole.trim_end_matches('Z'))
}

/// The whole `.sigmf-meta` document. `stop` is `None` while recording, so a
/// file read mid-way says it is still growing.
pub fn meta(header: &Header, rec: &Recording, stop: Option<&Stop>) -> String {
    let mut global = Map::new();
    let mut put = |k: &str, v: Value| {
        global.insert(k.to_string(), v);
    };
    put("core:datatype", json!(header.datatype));
    put("core:version", json!(SIGMF_VERSION));
    put("core:sample_rate", json!(header.sample_rate));
    put("core:hw", json!(header.hw));
    put(
        "core:recorder",
        json!(format!("sdrtop {}", env!("CARGO_PKG_VERSION"))),
    );
    put("core:description", json!(header.description));
    put(
        "core:extensions",
        json!([{ "name": "sdrtop", "version": EXTENSION_VERSION, "optional": true }]),
    );
    put("sdrtop:full_scale", json!(header.full_scale));
    put("sdrtop:bits", json!(header.bits));
    if let Some(stack) = &header.stack {
        put("sdrtop:stack", json!(stack));
    }
    put("sdrtop:gain", json!(header.gain));
    put("sdrtop:reference", json!(header.reference));
    put("sdrtop:iq_correction", json!(header.iq_correction));
    put(
        "sdrtop:state",
        json!(if stop.is_some() {
            "finished"
        } else {
            "recording"
        }),
    );
    if let Some(stop) = stop {
        put("sdrtop:stopped", json!(stop.sentence()));
    }
    put("sdrtop:pairs_written", json!(rec.written));
    put("sdrtop:lost_driver", json!(rec.lost_driver));
    put("sdrtop:lost_queue", json!(rec.lost_queue));
    put("sdrtop:lost_unexplained", json!(rec.lost_unexplained));
    put("sdrtop:wall_seconds", json!(rec.wall_seconds()));
    if let Some(n) = rec.radio_drops {
        put("sdrtop:radio_drops", json!(n));
    }
    if let Some(why) = &rec.radio_unreadable {
        put("sdrtop:radio_drops_unreadable", json!(why));
    }
    if let Some(hz) = rec.delivered_hz() {
        put("sdrtop:delivered_rate", json!(hz));
    }
    if let Some(sentence) = rec.shortfall_sentence() {
        put("sdrtop:shortfall", json!(sentence));
    }

    let captures: Vec<Value> = rec
        .segments
        .iter()
        .map(|s| {
            json!({
                "core:sample_start": s.sample_start,
                "core:global_index": s.global_index,
                "core:frequency": s.frequency_hz,
                "core:datetime": datetime(s.arrived_unix),
            })
        })
        .collect();
    let annotations: Vec<Value> = rec
        .all_annotations()
        .iter()
        .map(|a| {
            json!({
                "core:sample_start": a.sample_start,
                "core:sample_count": a.sample_count,
                "core:label": a.label,
                "core:comment": a.comment,
                "core:generator": "sdrtop",
            })
        })
        .collect();
    let doc = json!({
        "global": Value::Object(global),
        "captures": captures,
        "annotations": annotations,
    });
    // Pretty, because a person opens this file too.
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::iq::recording::Limits;
    use crate::hardware::record_tap::BlockAt;

    fn header() -> Header {
        Header {
            datatype: "ci8",
            sample_rate: 1e6,
            bytes_per_pair: 2,
            hw: "HackRF One, serial 0000".to_string(),
            description: "recorded by sdrtop from the lab_iq layout".to_string(),
            full_scale: 128.0,
            bits: 8,
            stack: Some("libhackrf 2024.02.1".to_string()),
            gain: "LNA=16,VGA=20, amp off".to_string(),
            reference: "unreferenced, no reference established".to_string(),
            iq_correction: "none".to_string(),
        }
    }

    fn at(first_pair: u64, pairs: u64) -> BlockAt {
        BlockAt {
            first_pair,
            pairs,
            driver_dropped: 0,
            centre_hz: 2_441_000_000,
            rate_hz: 1e6,
            arrived_unix: T0 + (first_pair + pairs) as f64 / 1e6,
        }
    }

    const T0: f64 = 1_790_500_000.25;

    /// **The recording keeps the rule the screen keeps**: a stage whose dB
    /// was measured where the radio was tuned is written as its dB, one whose
    /// dB was not as its step, so a file read years later does not claim a
    /// gain nobody measured.
    #[test]
    fn a_stage_measured_elsewhere_is_recorded_as_its_step() {
        let m = crate::state::SdrMetrics::fixture().band_measured_chain();
        assert_eq!(
            gain_text(&m.caps, &m.radio.gains, false, 943_400_000),
            "LNA=step 14/14,MIX=12.03,VGA=26.79"
        );
        assert_eq!(
            gain_text(&m.caps, &m.radio.gains, false, 92_800_000),
            "LNA=36.02,MIX=12.03,VGA=26.79"
        );
    }

    /// Fixed points, including the fraction's carry.
    #[test]
    fn datetimes_are_rfc3339_in_utc_to_the_microsecond() {
        assert_eq!(datetime(0.0), "1970-01-01T00:00:00.000000Z");
        assert_eq!(datetime(1_788_632_561.5), "2026-09-05T18:22:41.500000Z");
        assert_eq!(datetime(1.000_000_4), "1970-01-01T00:00:01.000000Z");
    }

    /// Each sample format is a datatype the core ABNF defines.
    #[test]
    fn the_three_formats_are_core_datatypes() {
        assert_eq!(datatype(SampleFormat::Int8), "ci8");
        assert_eq!(datatype(SampleFormat::Uint8), "cu8");
        assert_eq!(datatype(SampleFormat::Int16), "ci16_le");
    }

    /// The document parses, has the three objects SigMF requires, and each
    /// segment's time is when its first block arrived.
    #[test]
    fn the_meta_is_valid_json_with_every_segment_timed_by_its_arrival() {
        let mut rec = Recording::new(1e6, 2, Limits::DEFAULT);
        rec.block(&at(0, 1_000), "LNA=16,VGA=20, amp off");
        rec.refused(&at(1_000, 500_000));
        rec.block(&at(501_000, 1_000), "LNA=16,VGA=20, amp off");
        let doc: Value = serde_json::from_str(&meta(&header(), &rec, None)).unwrap();
        assert_eq!(doc["global"]["core:datatype"], "ci8");
        assert_eq!(doc["global"]["sdrtop:state"], "recording");
        assert!(doc["global"].get("sdrtop:stopped").is_none());
        let caps = doc["captures"].as_array().unwrap();
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0]["core:datetime"], "2026-09-27T09:06:40.251000Z");
        assert_eq!(caps[1]["core:sample_start"], 1_000);
        assert_eq!(caps[1]["core:global_index"], 501_000);
        assert_eq!(caps[1]["core:datetime"], "2026-09-27T09:06:40.752000Z");
        assert_eq!(doc["annotations"][0]["core:label"], "lost 500000");

        rec.finish(&Stop::Asked);
        let done: Value = serde_json::from_str(&meta(&header(), &rec, Some(&Stop::Asked))).unwrap();
        assert_eq!(done["global"]["sdrtop:state"], "finished");
        assert_eq!(done["global"]["sdrtop:lost_queue"], 500_000);
        let last = done["annotations"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(last["core:label"], "stopped");
        assert_eq!(last["core:sample_start"], 2_000);
    }

    /// Every `sdrtop:` field written is described in the extension document,
    /// and everything the document describes is written: SigMF 1.9.2 wants
    /// the namespace written down, and this keeps it the same namespace.
    #[test]
    fn the_extension_document_and_the_fields_written_agree() {
        let doc_text = include_str!("../../../user_docs/sdrtop.sigmf-ext.md");
        // Every optional field present: a recording long enough to state its
        // delivered rate, from a radio running short.
        let mut rec = Recording::new(1e6, 2, Limits::DEFAULT);
        for k in 0..30u64 {
            let mut b = at(k * 100_000, 100_000);
            b.arrived_unix = T0 + (k + 1) as f64 * 0.2;
            rec.block(&b, "");
        }
        rec.radio(crate::hardware::record_tap::RadioNote::Counted {
            events: 1,
            from_pair: 0,
            to_pair: 50_000,
            longest_bytes: 1_000,
        });
        rec.radio(crate::hardware::record_tap::RadioNote::Unreadable(
            "a later reading failed".into(),
        ));
        let written: Value =
            serde_json::from_str(&meta(&header(), &rec, Some(&Stop::Asked))).unwrap();
        let mut ours: Vec<String> = written["global"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with("sdrtop:"))
            .cloned()
            .collect();
        ours.sort();
        let mut described: Vec<String> = doc_text
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == ':' || c == '_'))
            .filter(|w| w.starts_with("sdrtop:") && w.len() > "sdrtop:".len())
            .map(str::to_string)
            .collect();
        described.sort();
        described.dedup();
        assert_eq!(ours, described);
        assert!(doc_text.contains(&format!("version {EXTENSION_VERSION}")));
    }
}
