// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The classic Bluetooth hits, one to a row (net-ux-polish-plan 6.6).
//!
//! The fifth body behind the section's one export key, sharing the
//! provenance header and the destination handling with the others.
//!
//! **The hits the section keeps, oldest first.** `NetState::bt_hops` holds
//! the latest [`crate::state::BT_HOP_LIMIT`]; a session that heard more says
//! so in the note ([`note`]), with how many.
//!
//! **Blank wherever the panels would not state it.** A slot residual only
//! for a hit its piconet's grid was fitted over, on the same stream: beyond
//! the fitted span it would be an extrapolation, and on another stream a
//! time on another clock (`signal::bt::slots::SlotFit::residual_at`).
//! Header fields only where the header was read under a resolved UAP; the
//! `header` column says which of the other cases it is. The UAP is written
//! once it is one value, and the candidates left always.
//!
//! **Marked as what it is.** The header decode is a `libbtbb` port never
//! checked against a classic transmitter on the air (`signal::bt::header`),
//! and the note says so beside the columns it fills (rule 1).

use crate::signal::bt::header::PacketType;
use crate::signal::bt::piconet::{HeaderRead, Inquiry, Kind};
use crate::state::SdrMetrics;

/// The columns, in order. `lap_kind` is `piconet`; `GIAC`, `LIAC` or `DIAC`
/// for an inquiry code, which is somebody searching and has neither a UAP to
/// narrow nor a slot grid to be timed against
/// (`signal::bt::piconet::Inquiry`); or `paged` for a device being called
/// (`signal::bt::piconet::Piconet::kind`), as the roster had it when the
/// file was written. `stream_us` is the hit's time on the stream's
/// sample clock, µs, comparable only within one run of the stream; the
/// header's `flow`, `arqn` and `seqn` are its flag bits in the order Core
/// 5.4 Vol 2 Part B 6.4 lists them.
pub const HEADER: &str = "age_s,stream_us,channel,lap,lap_kind,uap,uap_candidates,\
slot_residual_us,header,lt_addr,packet_type,flow,arqn,seqn";

/// What the file says about itself when it has rows: where the header
/// fields come from, and whether the session heard more than it keeps.
pub fn note(state: &SdrMetrics) -> String {
    let kept = state.net.bt_hops.len() as u64;
    let heard = state.net.health.bt_hits;
    let mut parts = vec![
        "header fields from a libbtbb port, unchecked on the air".to_string(),
        "residuals from each piconet's own fitted 625 us grid".to_string(),
    ];
    if heard > kept {
        parts.push(format!(
            "the latest {kept} of {heard} hits heard this session"
        ));
    }
    parts.join("; ")
}

/// The hits as CSV rows, oldest first.
pub fn rows(state: &SdrMetrics) -> Vec<String> {
    let now = std::time::Instant::now();
    state
        .net
        .bt_hops
        .iter()
        .rev()
        .map(|h| {
            let uaps = state.net.bt_uap.get(&h.lap);
            let piconet = state.net.bt_piconets.iter().find(|p| p.lap == h.lap);
            let residual = piconet
                .filter(|p| p.slots_stream == h.stream)
                .and_then(|p| p.slots.as_ref())
                .and_then(|s| s.as_ref().ok())
                .and_then(|fit| fit.residual_at(h.at_us))
                .map(|r| format!("{r:.3}"))
                .unwrap_or_default();
            let (status, fields) = match h.header {
                None => ("none", None),
                Some(HeaderRead::Unresolved) => ("not read: UAP unresolved", None),
                Some(HeaderRead::Undecoded) => ("did not decode", None),
                Some(HeaderRead::Decoded(hd)) => ("decoded", Some(hd)),
            };
            let mut f = vec![
                now.saturating_duration_since(h.seen).as_secs().to_string(),
                format!("{:.2}", h.at_us),
                h.channel.to_string(),
                // As the address mode allows, as on screen: an inquiry code
                // in hex still (it is no one's address), a masked LAP by its
                // roster number, and the UAP value hidden with it.
                match crate::signal::bt::piconet::Inquiry::of(h.lap) {
                    Some(_) => format!("{:#08x}", h.lap),
                    None => state.net.show_lap(h.lap),
                },
                match piconet.map(|p| p.kind()) {
                    Some(Kind::Inquiry(i)) => i.short(),
                    Some(k) => k.word(),
                    None => Inquiry::of(h.lap).map_or("piconet", |i| i.short()),
                }
                .to_string(),
                match uaps.map(|u| u.as_slice()) {
                    Some([one]) => state.net.show_uap(*one),
                    _ => String::new(),
                },
                uaps.map(|u| u.len().to_string()).unwrap_or_default(),
                residual,
                status.to_string(),
            ];
            match fields {
                Some(hd) => f.extend([
                    hd.lt_addr.to_string(),
                    PacketType::from_code(hd.packet_type.code())
                        .label()
                        .to_string(),
                    (hd.flags & 1).to_string(),
                    (hd.flags >> 1 & 1).to_string(),
                    (hd.flags >> 2 & 1).to_string(),
                ]),
                None => f.extend(std::iter::repeat_n(String::new(), 5)),
            }
            f.join(",")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::bt::header::Header;
    use crate::signal::bt::piconet::observe;
    use crate::state::BtHop;
    use std::time::{Duration, Instant};

    fn hop(lap: u32, channel: u8, at_us: f64, header: Option<HeaderRead>) -> BtHop {
        BtHop {
            channel,
            lap,
            seen: Instant::now() - Duration::from_secs(3),
            at_us,
            stream: 1,
            header,
        }
    }

    fn get(row: &str, name: &str) -> String {
        let i = HEADER.split(',').position(|h| h == name).unwrap();
        row.split(',').nth(i).unwrap().to_string()
    }

    /// **Each hit with what is known of it**: a residual from its piconet's
    /// own grid, the header fields where one was read, the UAP once one
    /// value, and blanks with the reason everywhere else.
    #[test]
    fn a_hit_exports_its_residual_and_its_header_where_known() {
        let mut m = SdrMetrics::fixture().streaming();
        let lap = 0x5a3c71;
        // Twelve hits on a clean grid, the last with a decoded DH1 header.
        let times: Vec<f64> = [0u32, 5, 11, 16, 22, 28, 33, 40, 46, 51, 57, 63]
            .iter()
            .map(|&k| 1_000.0 + k as f64 * 625.0)
            .collect();
        for (i, &t) in times.iter().enumerate() {
            observe(&mut m.net.bt_piconets, lap, 40, Instant::now());
            let header = (i == 11).then_some(HeaderRead::Decoded(Header {
                lt_addr: 2,
                packet_type: PacketType::Dh1,
                flags: 0b101,
                hec: 0,
                clk6: 0,
            }));
            m.net.bt_hops.push_front(hop(lap, 40, t, header));
        }
        let p = &mut m.net.bt_piconets[0];
        p.slots = Some(crate::signal::bt::slots::fit(&times));
        p.slots_stream = 1;
        m.net.bt_uap.insert(lap, vec![0x4c]);
        // Another piconet, one hit, nothing known.
        m.net.bt_hops.push_front(hop(0x123456, 12, 50_000.0, None));
        m.net.health.bt_hits = 20;

        let out = rows(&m);
        assert_eq!(out.len(), 13);
        for row in &out {
            assert_eq!(row.split(',').count(), HEADER.split(',').count(), "{row}");
        }
        let first = &out[0];
        assert_eq!(get(first, "lap"), "0x5a3c71");
        assert_eq!(get(first, "lap_kind"), "piconet");
        assert_eq!(get(first, "stream_us"), "1000.00");
        assert_eq!(get(first, "uap"), "0x4c");
        let r: f64 = get(first, "slot_residual_us").parse().unwrap();
        assert!(r.abs() < 0.01, "{first}");
        assert_eq!(get(first, "header"), "none");
        assert_eq!(get(first, "packet_type"), "");

        let dh1 = &out[11];
        assert_eq!(get(dh1, "header"), "decoded");
        assert_eq!(get(dh1, "lt_addr"), "2");
        assert_eq!(get(dh1, "packet_type"), "DH1");
        assert_eq!(
            (get(dh1, "flow"), get(dh1, "arqn"), get(dh1, "seqn")),
            ("1".to_string(), "0".to_string(), "1".to_string())
        );

        let other = &out[12];
        assert_eq!(get(other, "slot_residual_us"), "", "no grid: {other}");
        assert_eq!(get(other, "uap_candidates"), "", "{other}");
        assert!(
            note(&m).contains("the latest 13 of 20 hits"),
            "{}",
            note(&m)
        );
        assert!(note(&m).contains("unchecked on the air"));
    }

    /// A grid fitted on another stream gives no residual: those times are
    /// on another clock.
    #[test]
    fn a_residual_from_another_stream_is_blank() {
        let mut m = SdrMetrics::fixture().streaming();
        let times: Vec<f64> = (0..10).map(|k| k as f64 * 625.0 * 3.0).collect();
        observe(&mut m.net.bt_piconets, 0x1, 40, Instant::now());
        m.net.bt_piconets[0].slots = Some(crate::signal::bt::slots::fit(&times));
        m.net.bt_piconets[0].slots_stream = 2;
        m.net.bt_hops.push_front(hop(0x1, 40, times[3], None));
        assert_eq!(get(&rows(&m)[0], "slot_residual_us"), "");
    }
}
