// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The live line under a view in the menu (net-ux-polish-plan 7.2): what that
//! screen would tell you now, in one line.
//!
//! **Never more than the screen delivers.** Each line is built from the
//! same state and by the same rules the view's own panels use: the
//! capability word from the verdict's own composition, a duty cycle only
//! where the band's floor is trusted and the figure resolved, a CRC share
//! only past the frame error curve's ten-packet rule. A view that is not
//! running now says so and gives the session's figures as the session's;
//! a refused one gives its refusal.
//!
//! Pure, and tested without a terminal, as `entries::scroll_offset` is: one
//! function per layout, `None` for the layouts that have nothing live to
//! say, so their entries keep the two rows they always had.

use std::time::{Duration, Instant};

use crate::state::SdrMetrics;

/// One live line: its words, and whether the view is running now (drawn
/// `●` in the ok ink) or not (`○`, quiet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub text: String,
    pub running: bool,
}

/// How recently a census device must have been heard to count as present.
const PRESENT: Duration = Duration::from_secs(60);

/// The live line for the layout named `preset`, or `None` when it has none.
pub fn line(preset: &str, m: &SdrMetrics, now: Instant) -> Option<Live> {
    let net = &m.net;
    // Running: the radio streaming and the receiver that feeds this view
    // running now, which is not the same as the view being on screen: the
    // BLE decoder feeds the census and the packet list on either layout.
    let streaming = m.radio.hw_streaming;
    let on = streaming
        && match preset {
            "net_ble" | "net_census" => net.ble_channel.is_some(),
            "net_bt" => !net.bt_channels_watched.is_empty(),
            _ => m.ui.active_preset == preset,
        };
    let quiet = |text: String| Live {
        text,
        running: false,
    };
    // A session figure, said as the session's when the view is not running.
    let said = |text: String, refused: Option<&String>| match (on, refused) {
        (true, Some(why)) => quiet(format!("refused: {why}")),
        (true, None) => Live {
            text,
            running: true,
        },
        (false, _) => quiet(format!("{text}; not listening now")),
    };
    match preset {
        "net" => Some(Live {
            text: crate::ui::panels::net::shared::capability::headline(
                &m.caps,
                net.retune.as_ref(),
            ),
            // A statement about the radio, true whether or not it streams.
            running: true,
        }),
        "net_survey" => Some(survey(
            m,
            streaming
                && m.ui.section == crate::signal::net::SECTION
                && net.mode == crate::state::NetMode::Survey,
        )),
        "net_census" => {
            let devices = &net.census.devices;
            if devices.is_empty() {
                return Some(said("no device counted this session".to_string(), None));
            }
            let present = devices
                .iter()
                .filter(|d| now.saturating_duration_since(d.last_seen) <= PRESENT)
                .count();
            Some(said(
                format!(
                    "{} devices counted, {present} heard in the last minute",
                    devices.len()
                ),
                None,
            ))
        }
        "net_ble" => {
            let packets: u64 = net.ble_channel_packets.iter().sum();
            let ok: u64 = net.ble_channel_crc_ok.iter().sum();
            if packets == 0 {
                return Some(said(
                    "no packet decoded this session".to_string(),
                    net.ble_refused.as_ref(),
                ));
            }
            let share = crate::signal::ble::fer::fraction(ok, packets)
                .map(|r| format!(", {:.0} % CRC ok", r.value() * 100.0))
                .unwrap_or_default();
            Some(said(
                format!("{packets} packets this session{share}"),
                net.ble_refused.as_ref(),
            ))
        }
        "net_bt" => {
            let piconets = net.bt_piconets.len();
            if piconets == 0 {
                return Some(said(
                    "no piconet heard this session".to_string(),
                    net.bt_refused.as_ref(),
                ));
            }
            let resolved = net
                .bt_piconets
                .iter()
                .filter(|p| net.bt_uap.get(&p.lap).is_some_and(|u| u.len() == 1))
                .count();
            Some(said(
                format!("{piconets} piconets heard, {resolved} UAP resolved"),
                net.bt_refused.as_ref(),
            ))
        }
        _ => None,
    }
}

/// The survey's line: its busiest measured megahertz and that cell's duty,
/// asked as the occupancy panel and its export ask it, or why there is none.
fn survey(m: &SdrMetrics, on: bool) -> Live {
    use crate::signal::net::occupancy;
    let band = &m.net.band;
    let quiet = |text: String| Live {
        text,
        running: false,
    };
    if on {
        if let Some(why) = &m.net.survey_refused {
            return quiet(format!("refused: {why}"));
        }
    }
    let text = if band.cells.is_empty() {
        "no band measurement yet".to_string()
    } else if !band.trusted {
        "the noise floor failed its checks, so no duty is stated".to_string()
    } else {
        let busiest = band
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.observed())
            .map(|(i, c)| (i, occupancy::duty_uncertain(c.duty, c.windows).scale(100.0)))
            .filter(|(_, d)| d.is_resolved(occupancy::DUTY_RESOLUTION * 100.0))
            .max_by(|a, b| a.1.value().total_cmp(&b.1.value()));
        match busiest {
            Some((cell, duty)) => format!(
                "busiest {:.0} MHz, {:.0} % duty",
                occupancy::cell_centre_hz(cell) as f64 / 1e6,
                duty.value()
            ),
            None => "no cell measured well enough to state a duty".to_string(),
        }
    };
    if on {
        Live {
            text,
            running: true,
        }
    } else {
        quiet(format!("{text}; not surveying now"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::net::census::Device;

    /// On `preset`, with its receiver running when `streaming`, as the worker
    /// leaves the state.
    fn at(preset: &str, streaming: bool) -> SdrMetrics {
        let mut m = if streaming {
            SdrMetrics::fixture().streaming()
        } else {
            SdrMetrics::fixture()
        };
        m.ui.active_preset = preset.to_string();
        m.ui.section = crate::signal::net::SECTION.to_string();
        if streaming {
            match preset {
                "net_ble" | "net_census" => m.net.ble_channel = Some(37),
                "net_bt" => m.net.bt_channels_watched = vec![10, 11],
                _ => {}
            }
        }
        m
    }

    #[test]
    fn a_layout_with_nothing_live_has_no_line() {
        assert_eq!(line("lab_iq", &at("lab_iq", true), Instant::now()), None);
    }

    /// **Running only when streaming and on screen**; otherwise the
    /// session's figures, said as the session's.
    #[test]
    fn a_view_not_running_says_so_and_keeps_its_session_figures() {
        let mut m = at("net_ble", true);
        m.net.ble_channel_packets = [300, 100, 12];
        m.net.ble_channel_crc_ok = [290, 95, 11];
        let l = line("net_ble", &m, Instant::now()).unwrap();
        assert_eq!(l.text, "412 packets this session, 96 % CRC ok");
        assert!(l.running);

        // On the census layout the same decoder runs, so it still is.
        m.ui.active_preset = "net_census".to_string();
        assert!(line("net_ble", &m, Instant::now()).unwrap().running);

        // On the classic layout it does not.
        m.ui.active_preset = "net_bt".to_string();
        m.net.ble_channel = None;
        let l = line("net_ble", &m, Instant::now()).unwrap();
        assert!(!l.running);
        assert!(l.text.ends_with("; not listening now"), "{}", l.text);
    }

    /// Nine packets have no share, as the error curve's rule says.
    #[test]
    fn a_thin_count_has_no_share() {
        let mut m = at("net_ble", true);
        m.net.ble_channel_packets = [9, 0, 0];
        m.net.ble_channel_crc_ok = [9, 0, 0];
        let l = line("net_ble", &m, Instant::now()).unwrap();
        assert_eq!(l.text, "9 packets this session");
    }

    /// A refused decoder, on screen, gives its refusal, not a count.
    #[test]
    fn a_refusal_is_the_line() {
        let mut m = at("net_bt", true);
        m.net.bt_refused = Some("no classic channel fits the view".to_string());
        let l = line("net_bt", &m, Instant::now()).unwrap();
        assert_eq!(l.text, "refused: no classic channel fits the view");
        assert!(!l.running);
    }

    #[test]
    fn the_census_counts_the_present_apart() {
        let now = Instant::now();
        let mut m = at("net_census", true);
        m.net.census.devices = vec![
            Device::heard([1; 6], false, now - Duration::from_secs(10)),
            Device::heard([2; 6], false, now - Duration::from_secs(600)),
        ];
        m.net.census.devices[1].last_seen = now - Duration::from_secs(300);
        let l = line("net_census", &m, now).unwrap();
        assert_eq!(l.text, "2 devices counted, 1 heard in the last minute");
    }

    /// The survey states a duty only against a trusted floor.
    #[test]
    fn the_survey_states_no_duty_without_a_trusted_floor() {
        let mut m = at("net_survey", true);
        assert_eq!(
            line("net_survey", &m, Instant::now()).unwrap().text,
            "no band measurement yet"
        );
        m.net.band.cells.push(crate::state::CellReading {
            measured: Some(Instant::now()),
            windows: 10_000,
            duty: 0.34,
            ..Default::default()
        });
        m.net.band.trusted = false;
        assert!(line("net_survey", &m, Instant::now())
            .unwrap()
            .text
            .contains("no duty is stated"));
        m.net.band.trusted = true;
        let l = line("net_survey", &m, Instant::now()).unwrap();
        assert!(l.text.starts_with("busiest "), "{}", l.text);
        assert!(l.text.ends_with("34 % duty"), "{}", l.text);
    }

    #[test]
    fn the_classic_line_counts_resolved_uaps() {
        let mut m = at("net_bt", true);
        let now = Instant::now();
        crate::signal::bt::piconet::observe(&mut m.net.bt_piconets, 1, 10, now);
        crate::signal::bt::piconet::observe(&mut m.net.bt_piconets, 2, 11, now);
        m.net.bt_uap.insert(1, vec![0x4c]);
        m.net.bt_uap.insert(2, vec![0x4c, 0x9a]);
        let l = line("net_bt", &m, now).unwrap();
        assert_eq!(l.text, "2 piconets heard, 1 UAP resolved");
    }

    #[test]
    fn the_capability_line_is_the_verdicts_own() {
        let m = at("net", false);
        let l = line("net", &m, Instant::now()).unwrap();
        assert!(
            l.text.contains("OF") || l.text.contains("OUT OF BAND"),
            "{}",
            l.text
        );
        assert!(l.running);
    }
}
