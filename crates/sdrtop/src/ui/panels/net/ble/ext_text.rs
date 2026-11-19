// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! An extended advertising packet's own facts in words: its event and set,
//! the packet that pointed at it, and what became of its AuxPtr.
//!
//! The LE and LE Coded details both say these, each in its own rows, and
//! say them the same way: one wording for one fact wherever it appears
//! (rule 5). `list` is the list the packet is in, where its superior and
//! the auxiliary packet it was followed to are found.

use std::collections::VecDeque;

use crate::signal::ble::aux_ptr::{AuxOutcome, AuxPhy};
use crate::signal::ble::ext::{AdvMode, ExtHeader};
use crate::state::{BlePacket, ExtInfo, ExtRole};

/// The advertising mode, and the set and event the ADI names.
pub(crate) fn event(header: &ExtHeader) -> String {
    let mode = match header.mode {
        Some(AdvMode::NonConnectableNonScannable) => "non-connectable, non-scannable",
        Some(AdvMode::Connectable) => "connectable",
        Some(AdvMode::Scannable) => "scannable",
        None => "a reserved AdvMode",
    };
    let set = header.adi.map_or(String::new(), |a| {
        format!(" · SID {} · DID 0x{:03x}", a.sid, a.did)
    });
    format!("{mode}{set}")
}

/// What became of the AuxPtr. A heard packet still in `list` names the
/// scheme it came in, which the AuxPtr itself does not.
pub(crate) fn aux(ext: &ExtInfo, list: &VecDeque<BlePacket>) -> String {
    let (ch, phy) = ext.header.aux_ptr.map_or((0, ""), |a| {
        let phy = match a.phy {
            Some(AuxPhy::OneM) => "LE 1M",
            Some(AuxPhy::TwoM) => "LE 2M",
            Some(AuxPhy::Coded) => "LE Coded",
            None => "a reserved PHY",
        };
        (a.channel, phy)
    });
    match ext.aux {
        AuxOutcome::Heard { seq, after_us } => {
            let phy = list
                .iter()
                .find(|q| q.seq == seq)
                .map_or(phy, |q| q.phy.label());
            format!(
                "aux on ch {ch}, {phy}, heard {:.2} ms later",
                after_us / 1000.0
            )
        }
        AuxOutcome::NotInView => format!("aux on ch {ch}: not in the radio's view"),
        AuxOutcome::Missed => format!("aux on ch {ch}: listened, not heard"),
        AuxOutcome::FeedLost => format!("aux on ch {ch}: its samples were not held"),
        AuxOutcome::Pending => format!("aux on ch {ch}: waiting for its window"),
        AuxOutcome::NonePromised => "no auxiliary packet promised".to_string(),
        AuxOutcome::Refused(why) => format!("aux not followed: {why}"),
    }
}

/// For an auxiliary packet, the packet that pointed at it; `None` for a
/// primary one.
pub(crate) fn pointed(ext: &ExtInfo, list: &VecDeque<BlePacket>) -> Option<String> {
    let seq = match ext.role {
        ExtRole::AdvExt => return None,
        ExtRole::AuxAdv { superior_seq } => superior_seq,
        ExtRole::AuxChain { superior_seq } => Some(superior_seq),
    };
    Some(match seq.and_then(|s| list.iter().find(|q| q.seq == s)) {
        Some(q) => {
            let name = q.ext.as_ref().map_or("packet", |e| e.role.label());
            format!("from the {name} on ch {}", q.channel)
        }
        None if seq.is_some() => "from a packet no longer in the list".to_string(),
        None => "heard in another advertising set's window".to_string(),
    })
}
