// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! How one gain figure is spread across a device's stages.
//!
//! **Policy, not description.** `traits.rs` says what a device's stages are;
//! this says what sdrtop does with them, which is a decision rather than an
//! answer, so it lives in its own file.
//!
//! The decision: **fill front to back.** Raise the first stage to its ceiling
//! before touching the second, and unwind in reverse. That is the
//! noise-figure-optimal direction, because gain taken early lifts the signal
//! above every later stage's own noise, and it is the opposite of what
//! `SoapySDR`'s `setGain` does. Measured on a HackRF through `SoapyHackRF`, the
//! driver's automatic distribution had the VGA at 48 dB while the LNA sat at 16.
//!
//! Two honest caveats, both written down rather than smoothed over:
//!
//! - The order is the driver's own `listGains` order, which the SoapySDR header
//!   says *should* be RF to baseband. `SoapyHackRF` lists LNA before AMP when
//!   the physical order is the reverse, so this is a convention we follow rather
//!   than a guarantee we rely on. It is still the only statement available, and
//!   still better than filling the back first.
//! - Front-to-back is optimal for noise and worst for overload. sdrtop handles
//!   overload where it already did: the ADC peak warning and the `[A]` auto-gain
//!   key. **The knob is deterministic, the automation is clever**, and merging
//!   those two jobs into one control would make the knob unpredictable.

use super::traits::StageSpec;

/// Spread `total_db` across `stages`, front to back.
///
/// Returns the per-stage values and the total actually achieved, which is not
/// always the total asked for: a stage quantised to 8 dB cannot absorb 3, so
/// some requests are simply not reachable. On a HackRF's `LNA [0,40,8]` plus
/// `VGA [0,62,2]`, asking for 39 gives 38 and asking for 55 gives 54.
///
/// **The caller displays the achieved figure, never the request.** Showing what
/// was asked for would put a number on screen that the radio is not set to.
///
/// Each stage is floored onto its own grid rather than rounded, so the running
/// total never overshoots and the knob stays monotonic: a larger request can
/// never produce a smaller result.
///
/// Every stage starts at its **minimum**, which matters for an element whose
/// range begins below zero. Such a stage is an attenuator, and the reachable
/// total starts at the sum of the minimums rather than at zero.
#[allow(dead_code)] // wired in at G8
pub fn distribute(stages: &[StageSpec], total_db: f64) -> (Vec<f64>, f64) {
    let mut out: Vec<f64> = stages.iter().map(|s| s.min_db).collect();
    if stages.is_empty() {
        return (out, 0.0);
    }
    let floor_total: f64 = out.iter().sum();
    let want = if total_db.is_finite() { total_db } else { 0.0 };
    let mut remaining = (want - floor_total).max(0.0);

    for (slot, spec) in out.iter_mut().zip(stages) {
        let headroom = (spec.max_db - spec.min_db).max(0.0);
        let take = spec.snap_down(spec.min_db + remaining.min(headroom)) - spec.min_db;
        let take = take.max(0.0);
        *slot = spec.min_db + take;
        remaining -= take;
    }
    let achieved = out.iter().sum();
    (out, achieved)
}

/// The range of totals a stage list can actually reach.
///
/// The knob's own limits. Without this the caller would have to add the
/// maximums up itself, which is the same arithmetic in a second place.
#[allow(dead_code)] // wired in at G8
pub fn total_range(stages: &[StageSpec]) -> (f64, f64) {
    stages
        .iter()
        .fold((0.0, 0.0), |(lo, hi), s| (lo + s.min_db, hi + s.max_db))
}

/// The next total the stage list can actually reach, above or below `from`.
///
/// **Without this the knob sticks.** Stepping by a fixed 1 dB and redistributing
/// lands on the same achievable total again: on a HackRF's 8 dB and 2 dB grids,
/// 21 floors back to 20, so the readout never moves however long the key is
/// held. The step has to be measured in reachable totals, not in dB.
///
/// The probe is the finest grid any stage has, because a total can only change
/// by at least that much. A list of purely continuous stages has no grid, and
/// 1 dB is the arbitrary-but-useful answer there, matching what the knob used to
/// do before it distributed anything.
#[allow(dead_code)] // wired in at G8
pub fn next_total(stages: &[StageSpec], from: f64, up: bool) -> f64 {
    let (lo, hi) = total_range(stages);
    if stages.is_empty() || hi <= lo {
        return lo;
    }
    let finest = stages
        .iter()
        .map(|s| s.step_db)
        .filter(|s| *s > 0.0)
        .fold(f64::INFINITY, f64::min);
    let probe = if finest.is_finite() { finest } else { 1.0 };

    let current = distribute(stages, from).1;
    let dir = if up { 1.0 } else { -1.0 };
    // Bounded by the range itself, so a pathological grid cannot spin here.
    let limit = (((hi - lo) / probe).ceil() as i64).clamp(1, 4096);
    for k in 1..=limit {
        let candidate = (current + dir * probe * k as f64).clamp(lo, hi);
        let got = distribute(stages, candidate).1;
        if (got - current).abs() > 1e-9 {
            return got;
        }
        if (candidate - lo).abs() < 1e-9 || (candidate - hi).abs() < 1e-9 {
            break;
        }
    }
    current
}

/// Parse a named-stage gain string: `"LNA=28,VGA=12"`.
///
/// **One parser for the config file and the command line.** They accept the same
/// text on purpose: a form the file can express but the flag cannot is a trap,
/// and two parsers would eventually disagree about which.
///
/// Tolerant by design, because this is user-typed text in a file that must never
/// fail to load. A malformed entry is skipped and reported; the rest still
/// apply. Separators are `,` or `;`, whitespace is ignored, and names are
/// matched without regard to case later, so `lna=28` works.
///
/// Returns the pairs in the order given, plus anything worth telling the user.
#[allow(dead_code)] // wired in at G10
pub fn parse_named(text: &str) -> (Vec<(String, f64)>, Vec<String>) {
    let mut pairs = Vec::new();
    let mut notes = Vec::new();
    for entry in text
        .split([',', ';'])
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        match entry.split_once('=') {
            Some((name, value)) => {
                let (name, value) = (name.trim(), value.trim());
                if name.is_empty() {
                    notes.push(format!("gain: {entry:?} has no stage name"));
                    continue;
                }
                match value.parse::<f64>() {
                    Ok(v) if v.is_finite() => pairs.push((name.to_string(), v)),
                    _ => notes.push(format!("gain: {value:?} is not a number, in {entry:?}")),
                }
            }
            None => notes.push(format!("gain: {entry:?} is not NAME=value")),
        }
    }
    (pairs, notes)
}

/// Place parsed pairs onto a device's stages, snapping each into its own range.
///
/// Starts from `current`, so a string that names only some stages leaves the
/// others where they were rather than zeroing them. A name the device does not
/// have is **reported, not guessed at**: silently applying it to the nearest
/// stage is how a config written for one radio quietly mis-sets another.
#[allow(dead_code)] // wired in at G10
pub fn apply_named(
    stages: &[StageSpec],
    pairs: &[(String, f64)],
    current: &[f64],
) -> (Vec<f64>, Vec<String>) {
    let mut out: Vec<f64> = stages
        .iter()
        .enumerate()
        .map(|(i, s)| current.get(i).copied().unwrap_or(s.min_db))
        .collect();
    let mut notes = Vec::new();
    for (name, value) in pairs {
        match stages
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
        {
            Some(i) => out[i] = stages[i].snap(*value),
            None => {
                let known: Vec<&str> = stages.iter().map(|s| s.name.as_str()).collect();
                notes.push(format!(
                    "gain: this device has no stage {name:?}; it has {}",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                ));
            }
        }
    }
    (out, notes)
}

/// Render the current values as the string the config and the flag accept.
///
/// The round trip is the contract: what this writes, [`parse_named`] and
/// [`apply_named`] must read back to the same values.
#[allow(dead_code)] // wired in at G10
pub fn format_named(stages: &[StageSpec], values: &[f64]) -> String {
    stages
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{}={:.0}",
                s.name,
                values.get(i).copied().unwrap_or(s.min_db)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two stages a HackRF presents through SoapyHackRF once `AMP`, a
    /// two-position element, has become the boost. Real probe values.
    fn hackrf() -> Vec<StageSpec> {
        vec![
            StageSpec::ranged("LNA", 0.0, 40.0, 8.0),
            StageSpec::ranged("VGA", 0.0, 62.0, 2.0),
        ]
    }

    /// Written out by hand before the function existed, which is the point: the
    /// table is the specification, not a record of what the code happened to do.
    #[test]
    fn the_hand_table_for_a_hackrf() {
        let s = hackrf();
        let cases = [
            //  asked   LNA   VGA  achieved
            (0.0, 0.0, 0.0, 0.0),
            (1.0, 0.0, 0.0, 0.0),
            (2.0, 0.0, 2.0, 2.0),
            (3.0, 0.0, 2.0, 2.0),
            (8.0, 8.0, 0.0, 8.0),
            (20.0, 16.0, 4.0, 20.0),
            (39.0, 32.0, 6.0, 38.0),
            (40.0, 40.0, 0.0, 40.0),
            (41.0, 40.0, 0.0, 40.0),
            (55.0, 40.0, 14.0, 54.0),
            (90.0, 40.0, 50.0, 90.0),
            (102.0, 40.0, 62.0, 102.0),
            (200.0, 40.0, 62.0, 102.0),
        ];
        for (asked, lna, vga, achieved) in cases {
            let (got, total) = distribute(&s, asked);
            assert_eq!(got, vec![lna, vga], "asked {asked}");
            assert_eq!(total, achieved, "asked {asked}");
        }
    }

    /// The front stage fills first. This is the whole policy, and it is the
    /// opposite of what the driver does on its own.
    #[test]
    fn the_front_stage_fills_before_the_back_one() {
        let s = hackrf();
        for asked in [8.0, 16.0, 24.0, 32.0, 40.0] {
            let (got, _) = distribute(&s, asked);
            assert_eq!(got[0], asked, "the LNA should absorb all of {asked}");
            assert_eq!(got[1], 0.0, "and the VGA stay at zero");
        }
        // Only once the front is full does the back move.
        let (got, _) = distribute(&s, 48.0);
        assert_eq!(got, vec![40.0, 8.0]);
    }

    /// A larger request must never produce a smaller result. Rounding to the
    /// nearest instead of flooring breaks this, which is why `snap_down` exists.
    #[test]
    fn the_achieved_total_never_decreases_and_never_overshoots() {
        let s = hackrf();
        let mut previous = -1.0;
        for tenth in 0..=1100 {
            let asked = tenth as f64 / 10.0;
            let (_, achieved) = distribute(&s, asked);
            assert!(achieved >= previous, "went backwards at {asked}");
            assert!(achieved <= asked + 1e-9, "overshot at {asked}: {achieved}");
            previous = achieved;
        }
    }

    /// The parts must add up to the whole, or the readout and the radio would
    /// disagree about what the device is set to.
    #[test]
    fn the_stages_always_sum_to_the_achieved_total() {
        let s = hackrf();
        for asked in [0.0, 7.5, 33.3, 61.0, 101.9, 500.0] {
            let (got, achieved) = distribute(&s, asked);
            let sum: f64 = got.iter().sum();
            assert!((sum - achieved).abs() < 1e-9, "asked {asked}");
            for (v, spec) in got.iter().zip(&s) {
                assert!(*v >= spec.min_db && *v <= spec.max_db, "asked {asked}");
            }
        }
    }

    /// A single continuous stage, which is what a driver that names no elements
    /// leaves behind. No grid, so the answer is exact.
    #[test]
    fn one_continuous_stage_gets_exactly_what_is_asked() {
        let s = vec![StageSpec::ranged("RF", 0.0, 45.0, 0.0)];
        assert_eq!(distribute(&s, 17.3), (vec![17.3], 17.3));
        assert_eq!(distribute(&s, 100.0), (vec![45.0], 45.0));
    }

    /// An RTL-SDR: one stage over an irregular table, so the achievable totals
    /// are the table itself.
    #[test]
    fn a_tabled_stage_lands_on_the_drivers_own_values() {
        let s = vec![StageSpec::tabled("Tuner", vec![0.0, 9.0, 16.0, 24.0, 49.0])];
        assert_eq!(distribute(&s, 20.0).0, vec![16.0], "largest at or below");
        assert_eq!(distribute(&s, 9.0).0, vec![9.0]);
        assert_eq!(distribute(&s, 5.0).0, vec![0.0]);
        assert_eq!(distribute(&s, 1000.0).0, vec![49.0]);
    }

    /// A stage whose range starts below zero is an attenuator, and the reachable
    /// total starts there rather than at zero. Asking for less than the floor
    /// gets the floor, not a panic.
    #[test]
    fn a_negative_minimum_sets_the_floor_of_the_range() {
        let s = vec![
            StageSpec::ranged("ATT", -30.0, 0.0, 10.0),
            StageSpec::ranged("AMP", 0.0, 20.0, 5.0),
        ];
        assert_eq!(total_range(&s), (-30.0, 20.0));
        assert_eq!(distribute(&s, -30.0).0, vec![-30.0, 0.0]);
        assert_eq!(
            distribute(&s, -100.0).0,
            vec![-30.0, 0.0],
            "clamped, not wrapped"
        );
        assert_eq!(distribute(&s, -10.0), (vec![-10.0, 0.0], -10.0));
        assert_eq!(distribute(&s, 15.0), (vec![0.0, 15.0], 15.0));
    }

    /// Nothing to distribute across. The knob has to answer something.
    #[test]
    fn an_empty_stage_list_is_answered_rather_than_indexed() {
        assert_eq!(distribute(&[], 40.0), (vec![], 0.0));
        assert_eq!(total_range(&[]), (0.0, 0.0));
    }

    /// A request that is not a number must not propagate into the radio.
    #[test]
    fn a_nonsense_request_settles_at_the_floor() {
        let s = hackrf();
        assert_eq!(distribute(&s, f64::NAN), (vec![0.0, 0.0], 0.0));
        assert_eq!(distribute(&s, f64::INFINITY), (vec![0.0, 0.0], 0.0));
    }

    /// The knob has to move. Stepping a fixed 1 dB and redistributing would
    /// floor straight back to where it started, and the readout would never
    /// change however long the key was held.
    #[test]
    fn the_knob_steps_to_the_next_reachable_total() {
        let s = hackrf();
        assert_eq!(next_total(&s, 20.0, true), 22.0);
        assert_eq!(next_total(&s, 20.0, false), 18.0);
        // From a total that is not itself reachable, the answer is still a
        // reachable one on the correct side.
        assert_eq!(next_total(&s, 21.0, true), 22.0);
        assert_eq!(next_total(&s, 21.0, false), 18.0, "21 floors to 20 first");
    }

    /// And it stops at the rails rather than running off or spinning.
    #[test]
    fn the_knob_rests_against_the_rails() {
        let s = hackrf();
        assert_eq!(next_total(&s, 0.0, false), 0.0);
        assert_eq!(next_total(&s, -50.0, false), 0.0);
        assert_eq!(next_total(&s, 102.0, true), 102.0);
        assert_eq!(next_total(&s, 500.0, true), 102.0);
    }

    /// Walking the whole range one press at a time must terminate at both ends
    /// and visit strictly increasing totals.
    #[test]
    fn a_walk_from_end_to_end_terminates() {
        let s = hackrf();
        let mut at = 0.0;
        let mut seen = 0;
        while at < 102.0 && seen < 200 {
            let next = next_total(&s, at, true);
            assert!(next > at, "stalled at {at}");
            at = next;
            seen += 1;
        }
        assert_eq!(at, 102.0, "did not reach the ceiling in {seen} presses");
        while at > 0.0 && seen < 400 {
            let next = next_total(&s, at, false);
            assert!(next < at, "stalled coming down at {at}");
            at = next;
            seen += 1;
        }
        assert_eq!(at, 0.0);
    }

    /// A continuous range has no grid to follow, so the knob keeps its old 1 dB
    /// feel rather than inventing something finer.
    #[test]
    fn a_continuous_stage_steps_by_one_db() {
        let s = vec![StageSpec::ranged("RF", 0.0, 45.0, 0.0)];
        assert_eq!(next_total(&s, 17.0, true), 18.0);
        assert_eq!(next_total(&s, 17.0, false), 16.0);
    }

    /// An RTL-SDR walks its own table.
    #[test]
    fn a_tabled_stage_steps_between_its_entries() {
        let s = vec![StageSpec::tabled("Tuner", vec![0.0, 9.0, 16.0, 24.0, 49.0])];
        assert_eq!(next_total(&s, 9.0, true), 16.0);
        assert_eq!(next_total(&s, 16.0, false), 9.0);
        assert_eq!(next_total(&s, 49.0, true), 49.0);
    }

    /// Nothing to step across.
    #[test]
    fn an_empty_list_has_nowhere_to_step() {
        assert_eq!(next_total(&[], 10.0, true), 0.0);
    }

    /// A second two-position element stays in the list after G3 gave the boost
    /// key to the first. It is still a stage the knob can set, and its span is
    /// simply large: it absorbs nothing until the remainder reaches it.
    #[test]
    fn a_switch_left_in_the_list_is_still_distributed_over() {
        let s = vec![
            StageSpec::ranged("LNA", 0.0, 40.0, 8.0),
            StageSpec::ranged("PREAMP", 0.0, 20.0, 20.0),
        ];
        assert_eq!(distribute(&s, 40.0).0, vec![40.0, 0.0]);
        assert_eq!(
            distribute(&s, 55.0).0,
            vec![40.0, 0.0],
            "20 does not fit in 15"
        );
        assert_eq!(distribute(&s, 60.0), (vec![40.0, 20.0], 60.0));
    }

    // ── The named-stage string ──────────────────────────────────────────────

    #[test]
    fn the_named_form_round_trips() {
        let s = hackrf();
        let values = vec![24.0, 30.0];
        let text = format_named(&s, &values);
        assert_eq!(text, "LNA=24,VGA=30");

        let (pairs, notes) = parse_named(&text);
        assert!(notes.is_empty(), "{notes:?}");
        let (back, notes) = apply_named(&s, &pairs, &[0.0, 0.0]);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(back, values, "what we wrote must read back the same");
    }

    /// User-typed text, in a file that must never fail to load.
    #[test]
    fn the_parser_forgives_the_shapes_people_actually_type() {
        let (pairs, notes) = parse_named(" lna = 28 ; VGA=12 , ");
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            pairs,
            vec![("lna".to_string(), 28.0), ("VGA".to_string(), 12.0)]
        );
        assert_eq!(parse_named("").0, vec![]);
    }

    /// A broken entry is skipped and named; the rest still apply. Refusing the
    /// whole line would lose settings the user got right.
    #[test]
    fn a_malformed_entry_is_reported_and_the_rest_survive() {
        let (pairs, notes) = parse_named("LNA=24,VGA,=9,MIX=abc,AMP=14");
        assert_eq!(
            pairs,
            vec![("LNA".to_string(), 24.0), ("AMP".to_string(), 14.0)]
        );
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("\"VGA\"")));
        assert!(notes.iter().any(|n| n.contains("no stage name")));
        assert!(notes.iter().any(|n| n.contains("not a number")));
    }

    /// Names are matched without regard to case, and each value lands on its own
    /// stage's grid rather than wherever the user typed.
    #[test]
    fn values_snap_onto_the_stage_they_name() {
        let s = hackrf();
        let (pairs, _) = parse_named("lna=27,vga=31");
        let (out, notes) = apply_named(&s, &pairs, &[0.0, 0.0]);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(out, vec![24.0, 32.0], "8 dB and 2 dB grids");
    }

    /// A stage the string does not mention keeps what it had, rather than being
    /// zeroed by omission.
    #[test]
    fn an_unmentioned_stage_is_left_alone() {
        let s = hackrf();
        let (pairs, _) = parse_named("VGA=20");
        let (out, _) = apply_named(&s, &pairs, &[40.0, 8.0]);
        assert_eq!(out, vec![40.0, 20.0]);
    }

    /// A config written on one radio, opened on another. The name that does not
    /// exist is **reported**, and the message says what the device does have,
    /// because that is the only thing that helps the person reading it.
    #[test]
    fn a_stage_this_device_does_not_have_is_named_not_guessed_at() {
        let s = hackrf();
        let (pairs, _) = parse_named("IFGR=20,LNA=8");
        let (out, notes) = apply_named(&s, &pairs, &[0.0, 0.0]);
        assert_eq!(out, vec![8.0, 0.0], "the one it does have still applied");
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("IFGR"), "{:?}", notes[0]);
        assert!(
            notes[0].contains("LNA, VGA"),
            "names what it has: {:?}",
            notes[0]
        );
    }

    /// Out of range in either direction, and a device with no stages at all.
    #[test]
    fn values_out_of_range_are_clamped_and_no_stages_is_survivable() {
        let s = hackrf();
        let (pairs, _) = parse_named("LNA=999,VGA=-50");
        let (out, _) = apply_named(&s, &pairs, &[0.0, 0.0]);
        assert_eq!(out, vec![40.0, 0.0]);

        let (out, notes) = apply_named(&[], &pairs, &[]);
        assert!(out.is_empty());
        assert_eq!(
            notes.len(),
            2,
            "both names are unknown to a device with none"
        );
        assert_eq!(format_named(&[], &[]), "");
    }
}
