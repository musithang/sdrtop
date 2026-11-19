// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! An AuxPtr's promise: where and when an auxiliary packet will come, and
//! whether a packet heard there is the one promised (Core 5.4 Vol 6 Part B
//! 2.3.4.5).
//!
//! Pure: a promise is a window on the stream and what to listen for, and
//! every way it can end has a name ([`AuxOutcome`]), so the worker accounts
//! for each one rather than letting a promise it could not keep vanish.

pub use super::ext::AuxPhy;
use super::ext::{Adi, ExtHeader};
pub use super::follow::OWN_CLOCK_PPM;
use super::Phy;

/// How many auxiliary packets deep a chain is followed: an `AUX_ADV_IND`
/// is depth 0, each `AUX_CHAIN_IND` after it one more.
pub const CHAIN_DEPTH: u8 = 4;

/// Listened to before a promised packet's earliest start, us: the
/// receiver's filters settle on it, as a connection event's warm-up does.
pub const WARMUP_US: f64 = 64.0;

/// The longest packet `phy` can carry an extended PDU in, us: LE 1M's 2120
/// (a 255-octet payload), LE 2M's half that, LE Coded's 17040 at S=8 (Table
/// 2.1).
pub fn longest_us(phy: AuxPhy) -> f64 {
    match phy {
        AuxPhy::OneM => 2_120.0,
        AuxPhy::TwoM => 1_064.0,
        AuxPhy::Coded => 17_040.0,
    }
}

/// One AuxPtr's promise: a channel, a PHY, and the stretch of the stream the
/// auxiliary packet will start in, with what it must carry to be the one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Promise {
    /// The `seq` of the packet whose AuxPtr this is.
    pub superior_seq: u64,
    pub channel: u8,
    pub phy: AuxPhy,
    /// The superior's ADI, which the auxiliary packet repeats (2.3.1.6).
    pub adi: Option<Adi>,
    /// The superior's AdvA, where it carried one.
    pub adv_a: Option<[u8; 6]>,
    /// 0 for an `AUX_ADV_IND`, one more for each `AUX_CHAIN_IND` after it.
    pub depth: u8,
    /// Where the superior packet started: what the Aux Offset is counted from.
    pub superior_start_pair: f64,
    pub from_pair: f64,
    pub to_pair: f64,
}

/// How a promise ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AuxOutcome {
    /// Waiting for the stream to reach its window.
    Pending,
    /// The promised packet was heard: its `seq`, and how long after the
    /// superior's start it began, us.
    Heard { seq: u64, after_us: f64 },
    /// In view and listened to, and not heard.
    Missed,
    /// Its channel is outside what the radio sees.
    NotInView,
    /// Its samples were not held when the window came: lost to the feed, a
    /// break in the stream, or a receiver that could not be built.
    FeedLost,
    /// No auxiliary packet was promised (no AuxPtr, or an Aux Offset of zero).
    NonePromised,
    /// Not followed, and why.
    Refused(&'static str),
}

/// The promise in `header`'s AuxPtr, whose packet started at stream pair
/// `start_pair` (the first preamble symbol), `depth` packets down a chain,
/// at `rate_hz`. `Err` with the outcome when no window is opened.
///
/// The window is the one 2.3.4.5 states, from the offset to one unit after
/// it, widened by the advertiser's clock bound and this radio's own
/// assumed one ([`OWN_CLOCK_PPM`]) over the offset, opened [`WARMUP_US`]
/// early and kept open for the longest packet the PHY can send.
pub fn promise(
    superior_seq: u64,
    start_pair: f64,
    header: &ExtHeader,
    depth: u8,
    rate_hz: f64,
) -> Result<Promise, AuxOutcome> {
    let Some(aux) = header.aux_ptr else {
        return Err(AuxOutcome::NonePromised);
    };
    if aux.promises_nothing() {
        return Err(AuxOutcome::NonePromised);
    }
    let Some(phy) = aux.phy else {
        return Err(AuxOutcome::Refused("a reserved Aux PHY"));
    };
    if depth >= CHAIN_DEPTH {
        return Err(AuxOutcome::Refused("deeper in its chain than is followed"));
    }
    let offset = aux.offset_us as f64;
    let widening = offset * (aux.clock_ppm() as f64 + OWN_CLOCK_PPM) * 1e-6;
    let pairs = |us: f64| us * rate_hz * 1e-6;
    Ok(Promise {
        superior_seq,
        channel: aux.channel,
        phy,
        adi: header.adi,
        adv_a: header.adv_a,
        depth,
        superior_start_pair: start_pair,
        from_pair: start_pair + pairs(offset - widening - WARMUP_US),
        to_pair: start_pair + pairs(offset + aux.unit_us as f64 + widening + longest_us(phy)),
    })
}

/// Whether `heard`, received on `heard_phy` in `p`'s window, is the promised
/// packet: on the promised PHY, with the superior's ADI, and the same AdvA
/// where both name one.
pub fn keeps(p: &Promise, heard_phy: Phy, heard: &ExtHeader) -> bool {
    let phy = matches!(
        (p.phy, heard_phy),
        (AuxPhy::OneM, Phy::OneM) | (AuxPhy::TwoM, Phy::TwoM) | (AuxPhy::Coded, Phy::Coded(_))
    );
    let adi = p.adi == heard.adi;
    let adv_a = match (p.adv_a, heard.adv_a) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    };
    phy && adi && adv_a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::coded::Coding;
    use crate::signal::ble::ext::{parse, ExtHeader};

    fn header(aux: Option<(u8, u32, bool, u8)>) -> ExtHeader {
        // AuxPtr from (channel, offset in 30 us units, CA, PHY bits).
        let mut payload = vec![0, 0b0000_1000, 0x23, 0x31];
        if let Some((ch, units, ca, phy)) = aux {
            payload[1] |= 0b0001_0000;
            let v: u32 = ch as u32 | u32::from(ca) << 6 | units << 8 | (phy as u32) << 21;
            payload.extend([v as u8, (v >> 8) as u8, (v >> 16) as u8]);
        }
        payload[0] = (payload.len() - 1) as u8;
        parse(&payload).unwrap()
    }

    /// The window runs from the offset, less the clocks' widening and a
    /// warm-up, to one unit past it, plus the widening and the longest
    /// packet the promised PHY can send.
    #[test]
    fn the_window_runs_from_the_offset_to_one_unit_after() {
        let p = promise(7, 1000.0, &header(Some((9, 100, true, 0b010))), 0, 20e6).unwrap();
        let pairs = |us: f64| us * 20.0;
        let widening = 3000.0 * (50.0 + OWN_CLOCK_PPM) * 1e-6;
        assert_eq!(p.channel, 9);
        assert_eq!(p.phy, AuxPhy::Coded);
        assert_eq!(p.superior_seq, 7);
        assert!((p.from_pair - (1000.0 + pairs(3000.0 - widening - WARMUP_US))).abs() < 1e-6);
        assert!(
            (p.to_pair - (1000.0 + pairs(3030.0 + widening + longest_us(AuxPhy::Coded)))).abs()
                < 1e-6
        );
    }

    #[test]
    fn a_zero_offset_promises_nothing() {
        assert_eq!(
            promise(1, 0.0, &header(Some((9, 0, true, 0b010))), 0, 20e6),
            Err(AuxOutcome::NonePromised)
        );
        assert_eq!(
            promise(1, 0.0, &header(None), 0, 20e6),
            Err(AuxOutcome::NonePromised)
        );
    }

    #[test]
    fn a_reserved_aux_phy_is_refused() {
        assert!(matches!(
            promise(1, 0.0, &header(Some((9, 100, true, 0b101))), 0, 20e6),
            Err(AuxOutcome::Refused(_))
        ));
    }

    #[test]
    fn chains_stop_at_their_depth() {
        assert!(promise(
            1,
            0.0,
            &header(Some((9, 100, true, 0b010))),
            CHAIN_DEPTH - 1,
            20e6
        )
        .is_ok());
        assert!(matches!(
            promise(
                1,
                0.0,
                &header(Some((9, 100, true, 0b010))),
                CHAIN_DEPTH,
                20e6
            ),
            Err(AuxOutcome::Refused(_))
        ));
    }

    /// The promised packet carries the superior's ADI, on the promised PHY,
    /// and the same AdvA when both name one; anything else in the window is
    /// a stranger.
    #[test]
    fn a_stranger_in_the_window_is_not_the_promised_packet() {
        let p = promise(1, 0.0, &header(Some((9, 100, true, 0b010))), 0, 20e6).unwrap();
        let coded = Phy::Coded(Coding::S8);
        let same = header(None);
        assert!(keeps(&p, coded, &same));
        let mut other_adi = same.clone();
        other_adi.adi = Some(crate::signal::ble::ext::Adi { did: 0x124, sid: 3 });
        assert!(!keeps(&p, coded, &other_adi));
        assert!(
            !keeps(&p, Phy::OneM, &same),
            "an LE 1M packet keeps no Coded promise"
        );
        let mut with_a = p;
        with_a.adv_a = Some([1, 2, 3, 4, 5, 6]);
        let mut other_a = same.clone();
        other_a.adv_a = Some([9, 9, 9, 9, 9, 9]);
        assert!(!keeps(&with_a, coded, &other_a));
        other_a.adv_a = Some([1, 2, 3, 4, 5, 6]);
        assert!(keeps(&with_a, coded, &other_a));
    }
}
