// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Bluetooth Low Energy: one of the two protocol modules under the Bluetooth
//! arc (`bluetooth-bench-design.md`; the checkpoint plan is
//! `bluetooth-bench-plan.md`).
//!
//! Split the way `net-foundation-design.md` section 10 requires of every
//! protocol module: detect, sync, decode, measure, in that order, each in its
//! own file, with none of it knowing classic Bluetooth (`signal::bt`, not yet
//! built) or Wi-Fi exist. [`channel`] comes before any of those stages: it is
//! arithmetic the whole arc needs (which frequency a channel index names), not
//! a step in the receive chain. [`gfsk`] sits beside it for the same reason:
//! turning bits into the waveform LE 1M transmits is not itself a receive
//! stage, but [`detect`] needs it to build the reference it correlates
//! against, since the advertising access address is known in advance.

pub mod ad;
pub mod address;
pub mod assigned;
pub mod channel;
pub mod coded;
pub mod connect;
pub mod detect;
pub mod fer;
pub mod gfsk;
pub mod interval;
pub mod measure;
pub mod pdu;
pub mod receive;
pub mod sync;

/// Which of BLE's two uncoded PHYs a chain is built for. LE 1M is every
/// step from B6 through B16's own PHY; B17 adds LE 2M, design section 1.2's
/// own "the same chain at twice the symbol rate... nothing new except the
/// numbers" - a receiver parameterised by this rather than a second,
/// separately-maintained copy of [`receive::Receiver`].
///
/// **Not read from the Bluetooth Core Specification itself this session** -
/// the same standing every fact in this arc has (design section 6's own
/// facts-to-verify table, which gains rows for LE 2M's own preamble length
/// and deviation figure alongside the ones already there). [`Phy::OneM`]'s
/// own numbers already had that citation from B1 onward; [`Phy::TwoM`]'s
/// are reasoned by direct analogy - the same modulation index formula
/// (`h = 2 * deviation / symbol_rate`) held at the same `h = 0.5`, and the
/// same alternating-preamble rule run for twice as long - not independently
/// looked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Phy {
    #[default]
    OneM,
    /// Received end to end by `signal::ble::receive::Receiver`, and
    /// selectable since net-ux-polish-plan 5.5 (`NetState::ble_phy`). Never
    /// used on the primary advertising channels, which carry only LE 1M and
    /// LE Coded; the worker refuses it there rather than listening to
    /// nothing.
    TwoM,
}

impl Phy {
    /// `LE 1M` / `LE 2M`: the specification's names, for the chrome tag and
    /// the detail view.
    pub fn label(self) -> &'static str {
        match self {
            Phy::OneM => "LE 1M",
            Phy::TwoM => "LE 2M",
        }
    }

    /// The symbol rate this PHY transmits at - fixed by the PHY itself, not
    /// a free parameter a caller picks.
    pub fn symbol_rate_hz(self) -> f64 {
        match self {
            Phy::OneM => 1_000_000.0,
            Phy::TwoM => 2_000_000.0,
        }
    }

    /// The nominal peak frequency deviation a modulation index of 0.5
    /// implies at this PHY's own symbol rate - 250 kHz at 1 Mb/s, 500 kHz
    /// at 2 Mb/s, both numbers moving together so the index itself does
    /// not.
    pub fn deviation_hz(self) -> f64 {
        match self {
            Phy::OneM => 250_000.0,
            Phy::TwoM => 500_000.0,
        }
    }

    /// How many bits the preamble is on this PHY - 8 for LE 1M, 16 for LE
    /// 2M, the same alternating rule ([`detect::preamble_bits`]'s own doc)
    /// run for twice as long.
    pub fn preamble_bits_len(self) -> usize {
        match self {
            Phy::OneM => 8,
            Phy::TwoM => 16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both PHYs hold the same modulation-index relationship design
    /// section 2.1 states for LE 1M - `h = 2 * deviation / symbol_rate`
    /// equal to 0.5 - checked directly rather than trusted from the two
    /// numbers having been chosen by eye to look proportional.
    #[test]
    fn both_phys_hold_the_same_modulation_index() {
        for phy in [Phy::OneM, Phy::TwoM] {
            let h = 2.0 * phy.deviation_hz() / phy.symbol_rate_hz();
            assert!((h - 0.5).abs() < 1e-9, "{phy:?}: h = {h}");
        }
    }

    /// LE 2M runs at exactly twice LE 1M's own symbol rate and preamble
    /// length - design section 1.2's own claim, held to account rather
    /// than trusted from the doc comment alone.
    #[test]
    fn two_m_is_exactly_double_one_m() {
        assert_eq!(Phy::TwoM.symbol_rate_hz(), Phy::OneM.symbol_rate_hz() * 2.0);
        assert_eq!(
            Phy::TwoM.preamble_bits_len(),
            Phy::OneM.preamble_bits_len() * 2
        );
    }
}
