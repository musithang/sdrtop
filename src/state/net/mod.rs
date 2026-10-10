// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetState` - what the 2.4 GHz section knows right now: the mode and the
//! tuning it leaves the radio at, each receiver's channel or refusal, the
//! packet lists, the census, the band history and the connections followed.
//!
//! One struct, because the panels, the header, the menu's live lines and the
//! export all read across it from one snapshot; split into files by what each
//! part is about, so a part reads as one thing:
//!
//! - [`address`]: how an address, a name, raw bytes, a LAP, an access address
//!   or a UAP is shown, under the masking mode the user chose.
//! - [`packets`]: the BLE and LE Coded lists and how each is held and read.
//! - [`classic`]: classic Bluetooth's hops and the Piconet view's list.
//! - [`follow`]: the connections a CONNECT_IND set up.
//! - [`census`]: the census table's view, and when an empty one means nobody
//!   counting.
//! - [`band`]: the band occupancy history.
//! - [`health`]: the decode-health counts.

mod address;
mod band;
mod census;
mod classic;
mod follow;
mod health;
mod packets;

pub use address::{who_with, AddressBook, AddressDisplay, FULL_ADDRESS_WIDTH};
pub use band::{BandOccupancy, CellReading, COLUMN_INTERVAL, HISTORY_COLUMNS};
pub use census::CensusState;
#[cfg(test)]
pub use classic::ROSTER_SORT_KEYS;
pub use classic::{BtHop, HopView, PacketsView, BT_HOP_LIMIT};
pub use follow::{ConnectionView, FollowedConnection};
pub use health::NetDecodeHealth;
pub use packets::{
    BlePacket, BlePacketView, CodedFacts, ExtInfo, ExtRole, PduKind, BLE_PACKET_LIMIT,
};

/// Whether the receiver is walking the band or sitting on one channel.
///
/// **This changes what every number below it means**, which is why it is the
/// first field in the header and why it is never absent. A duty cycle measured
/// while sweeping is a sample of a channel; measured while locked it is that
/// channel's whole story, so every panel says which one it is looking at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetMode {
    /// Stepping across the band. The default, because nothing has been chosen
    /// yet and a receiver that has not been told where to sit is surveying.
    #[default]
    Survey,
    /// Parked on one channel.
    Lock,
}

impl NetMode {
    /// The word the header shows. Upper case because it is a mode, not a
    /// reading, and the eye has to find it without looking.
    pub fn label(self) -> &'static str {
        match self {
            NetMode::Survey => "SURVEY",
            NetMode::Lock => "LOCK",
        }
    }

    /// The chrome tag every panel in the section carries.
    ///
    /// A panel says *which claim its numbers are*, and the engine spells and
    /// colours it, which is the rule for every tag. The mode is part of the
    /// reading, so this is not optional for a panel here and
    /// `every_net_panel_says_how_its_numbers_were_gathered` is what makes that
    /// true rather than customary.
    pub fn tag(self) -> crate::ui::panel::Tag {
        match self {
            NetMode::Survey => crate::ui::panel::Tag::Survey,
            NetMode::Lock => crate::ui::panel::Tag::Lock,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            NetMode::Survey => NetMode::Lock,
            NetMode::Lock => NetMode::Survey,
        }
    }
}

/// Where the radio belongs once the survey gives the tuner back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetExit {
    pub tune_hz: u64,
    /// Whether the radio stays where the pass left it, rather than going back
    /// where the survey found it.
    pub locked: bool,
    /// Why there, when it was the occupancy cursor's `L` that chose it.
    pub why: Option<String>,
}

/// A lock the occupancy cursor asked for (`signal::net::survey::lock_target`):
/// the frequency and the sentence the log gives for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockTarget {
    pub tune_hz: u64,
    pub why: String,
}

#[derive(Clone, Debug, Default)]
pub struct NetState {
    pub mode: NetMode,
    /// Why no survey is running, when the mode says one should be.
    ///
    /// **A refusal nobody can see is a silence.** The survey declines on a
    /// receiver too narrow to see past its own oscillator, which is right, and
    /// it said so only in the log - which the survey and coexistence screens do
    /// not carry. What the user saw was a panel claiming to be waiting for RX
    /// while the feed panel beside it counted blocks arriving. The sentence
    /// belongs on the screen the reader is on, so it lives here rather than only
    /// in the log.
    pub survey_refused: Option<String>,
    pub census: CensusState,
    pub health: NetDecodeHealth,
    pub band: BandOccupancy,
    /// Advertising channel PDUs decoded so far this session, newest first,
    /// capped at [`BLE_PACKET_LIMIT`].
    pub ble_packets: std::collections::VecDeque<BlePacket>,
    /// The connections whose CONNECT_IND was heard, followed through the
    /// events the radio's window holds (`signal::ble::follow`), newest
    /// first, at most [`follow::CONNECTIONS_KEPT`].
    pub ble_connections: Vec<FollowedConnection>,
    pub connection_view: ConnectionView,
    /// Packets that have entered [`Self::ble_packets`] this session: each
    /// one's [`BlePacket::seq`] is the count at its arrival.
    pub ble_heard: u64,
    /// How the packet list is being read: which packet the cursor is on.
    pub ble_view: BlePacketView,
    /// LE Coded packets heard on the LE Coded view, newest first, kept as
    /// `ble_packets` is, in a ring of their own so neither list pushes the
    /// other's rows out.
    pub coded_packets: std::collections::VecDeque<BlePacket>,
    /// LE Coded packets heard this session, as `ble_heard` counts LE 1M's.
    pub coded_heard: u64,
    /// How the LE Coded list is being read.
    pub coded_view: BlePacketView,
    /// The advertising channel the LE Coded receiver has, when it has one.
    pub coded_channel: Option<u8>,
    /// Why the LE Coded receiver is not running, as `ble_refused` says LE
    /// 1M's.
    pub coded_refused: Option<String>,
    /// The PHY the BLE decoder listens for: LE 1M, and nothing on screen
    /// changes it.
    ///
    /// **Why there is no switch.** The decoder looks for the advertising
    /// access address, so off the advertising channels it hears secondary
    /// advertising and never a connection, whose packets carry an address
    /// of their own. LE 2M lives in connections (after a PHY update) and in
    /// a rare secondary advertisement, and the advertising channels never
    /// carry it, so a key that switched to it either was refused or found
    /// almost nothing. The worker still decodes LE 2M end to end: following
    /// a connection, which knows its address and when it changes PHY, is
    /// what will set this.
    pub ble_phy: crate::signal::ble::Phy,
    /// The session's frame error rate against SNR over all BLE traffic
    /// (`signal::ble::fer`): every packet decoded to
    /// its length, good or failed, in its SNR bin.
    pub fer: crate::signal::ble::fer::FerCurve,
    /// Why nothing is being decoded, when the radio can otherwise stream.
    ///
    /// The BLE decoder needs the working rate `signal::ble::receive::front_end`
    /// states, and needs the tuning to actually be one of the three
    /// advertising channels - two conditions `net_survey`'s occupancy
    /// measurement does not share, so this is its own refusal rather than
    /// reusing `survey_refused`. Same reasoning as that field's own doc: a
    /// refusal nobody can see is a silence.
    pub ble_refused: Option<String>,
    /// The BLE channel a receiver is running on right now, `None` when none
    /// is.
    ///
    /// **Not the same fact as `ble_refused` being `None`.** That field starts
    /// `None` before the first block has arrived and stays `None` while the
    /// section is closed, so reading "not refused" as "running" would put a
    /// decoder on the header that does not exist. This is set only where the
    /// worker actually builds a receiver, and cleared wherever it drops one.
    pub ble_channel: Option<u8>,
    /// How many packets the receiver has decoded on each advertising
    /// channel this session - index 0, 1, 2 for channel 37, 38, 39, the
    /// same low-to-high order [`crate::signal::ble::channel::
    /// advertising_channels_hz`] returns them in.
    ///
    /// **Every decode attempt, not only the CRC-clean ones
    /// [`crate::signal::net::census`] counts.** This answers "how much is
    /// happening on this channel", which a corrupted decode still is real
    /// evidence of; the census answers "which devices are confirmed here",
    /// where an unconfirmed address would be an invented reading. Different
    /// questions, so a different gate. Packet counts per channel, with the
    /// dwell fraction stated, are this field plus [`NetMode::Survey`]'s
    /// rotation always dwelling `1 / advertising_channels_hz().len()` of a pass
    /// on each.
    pub ble_channel_packets: [u64; 3],
    /// Of [`Self::ble_channel_packets`], the ones whose CRC passed, same
    /// indexing: the pair gives each advertising channel its pass rate
    /// A channel a Wi-Fi network sits on shows
    /// it here first, as a rate that falls while the count keeps rising.
    pub ble_channel_crc_ok: [u64; 3],
    /// Why classic Bluetooth has no live receiver at all right now, on the
    /// `net_bt` preset - the same "refused, not silent" discipline
    /// [`Self::ble_refused`] already follows. `Some` only when no
    /// `signal::bt::receive::Receiver` can exist (one per channel
    /// `signal::bt::channel::channels_in_span` and `[net].bt_channels` together
    /// let the worker watch): when the current tuning's span holds no classic
    /// BT channel at all. `None` while it is running, the same as
    /// [`Self::ble_refused`]. It says nothing about whether any *hit* has been found
    /// yet; [`Self::bt_piconets`] is what has been.
    pub bt_refused: Option<String>,
    /// Classic Bluetooth access-code hits since the section opened, newest
    /// first, capped at [`BT_HOP_LIMIT`]: one entry per clean access code any
    /// watched channel's `signal::bt::receive::Receiver` found.
    pub bt_hops: std::collections::VecDeque<BtHop>,
    /// Which classic BT channels the current tuning, span and
    /// `[net].bt_channels` together let the receiver actually watch, low to
    /// high - what `net_bt_hops` reports itself as watching, honestly
    /// narrower than the full 79 (or even the full count a wider capture
    /// could see) whenever the cap is binding. Empty exactly when
    /// [`Self::bt_refused`] is `Some`.
    pub bt_channels_watched: Vec<u8>,
    /// The most classic channels the worker watches at once
    /// (`[net].bt_channels`), published by the worker so a locked step on
    /// the classic view moves by a constant block: the watched list itself
    /// is shorter at the band's edges. `0` before the worker has run.
    pub bt_capacity: usize,
    /// The survey is watching fewer classic channels than its view holds,
    /// because its measured load leaves no room for more
    /// (`signal::net::worker`'s survey budget): what the coexistence key
    /// says rather than showing a count that looks like a quiet band.
    pub bt_load_limited: bool,
    /// Per-LAP UAP narrowing, refined by the payload tie-break: the distinct
    /// UAP values `signal::bt::header:: PiconetClock` still cannot rule out for
    /// that piconet, from every header captured on it so far this session -
    /// usually exactly two, not one, `PiconetClock`'s own doc has the
    /// measurement that found that floor and why a header alone cannot go
    /// lower. A single element means `signal::net::worker`'s own
    /// `payload::break_uap_tie` resolved the tie using a real DH1/DH3/DH5
    /// payload's own CRC-16 - sticky from then on for that LAP, since a
    /// piconet's real UAP does not change mid-session.
    pub bt_uap: std::collections::HashMap<u32, Vec<u8>>,
    /// Every piconet heard this session, one record per LAP
    /// (`signal::bt::piconet`): what the roster draws, counted over the
    /// whole session where [`Self::bt_hops`] keeps a window.
    pub bt_piconets: Vec<crate::signal::bt::piconet::Piconet>,
    /// The roster's cursor, on a LAP: the piconet selected. The hop scatter
    /// shares it, so a piconet picked in either is the one both show.
    pub bt_view: super::Selection<u32>,
    /// How the roster is ordered, `s` and `r` on it. See
    /// [`NetState::bt_roster`].
    pub bt_sort: classic::RosterSort,
    /// How much of the past the hop scatter shows, and how far back it ends.
    pub hop_view: HopView,
    /// Where the Piconet view's packet list is scrolled to, and the packet
    /// it is held at.
    pub packets_view: PacketsView,
    /// The tuning the survey interrupted, so it can be given back.
    ///
    /// **In the state rather than in the task**, for the reason
    /// [`crate::state::SweepState::end`] gives about the same field: the task is
    /// not the only thing that has to put the radio back. Quitting mid-pass
    /// never reaches another iteration of that loop - the process ends - and
    /// `save_config` would write out whichever hop the survey was parked on, so
    /// the app would reopen somewhere in the middle of the band, one position
    /// further along each time.
    pub pre_survey_hz: Option<u64>,
    /// How addresses are shown throughout the section. See [`AddressDisplay`].
    pub address_display: AddressDisplay,
    /// The session's masked numbers. See [`AddressBook`].
    pub address_book: AddressBook,
    /// What each address has advertised about itself, from its packets whose
    /// CRC passed (`signal::net::worker`, `signal::ble::ad::Advertised`):
    /// the company its manufacturer data named, its name, its TX power. Per
    /// address, not per packet, so a device reads the same in every panel and
    /// the export, and a scan response without manufacturer data does not
    /// turn it back into its kind.
    pub advertised: std::collections::HashMap<[u8; 6], crate::signal::ble::ad::Advertised>,
    /// A lock the occupancy cursor's `L` asked for and the survey task has not
    /// applied yet. **The task applies it**, as the survey's hand-back when a
    /// survey is running (`Self::end`) or on its next idle poll when the radio
    /// is already locked, so NET has one path that retunes the radio, never a
    /// second one behind the survey's back.
    pub lock_at: Option<LockTarget>,
    /// The Survey's time cursor: the identity of the history column it is on
    /// (`BandOccupancy::columns_taken`), `None` for now. Shared by both halves
    /// of the instrument: the heatmap moves it, the profile shows that moment.
    pub band_scrub: Option<u64>,
    /// The occupancy profile's cursor, a megahertz cell index (0 is
    /// 2400 MHz). The section's one selection model, keyed by the cell so it
    /// stays
    /// on its megahertz however the panel is resized.
    pub band_cursor: crate::state::Selection<usize>,
    /// The tuning-call measurement the Capability panel's `K` runs
    /// (`signal::retune::measure_calls`): `None` until someone asks, which
    /// the panel shows as "not measured" and never as a default. Kept for the
    /// session: it is a fact about this radio on this host.
    pub retune: Option<RetuneRun>,
}

/// Where the tuning-call measurement stands.
#[derive(Clone, Debug)]
pub enum RetuneRun {
    /// Running on its own thread: the radio is being retuned across the band.
    Measuring,
    /// Finished, and when.
    Done(crate::signal::retune::CallMeasurement, std::time::Instant),
}

impl NetState {
    /// Give the tuner back, and say where the radio belongs.
    ///
    /// **The two ways out of a survey want opposite answers, and treating them
    /// as one was a bug.** Leaving the section ends the survey, so the radio goes
    /// back where it was found. Switching to lock means "stay here": the user
    /// pressed the key while looking at a position, and that position is what
    /// they meant. The first version restored in both cases, while the key
    /// handler logged `NET locked to <the current hop>` - so the log said one
    /// thing and the radio did another.
    ///
    /// `tuned_hz` is `radio.frequency`. Safe on a state that never surveyed, and
    /// safe to call twice: the second call has nothing left to take and answers
    /// with whatever the caller wrote back after the first.
    pub fn end(&mut self, tuned_hz: u64) -> NetExit {
        let pre = self.pre_survey_hz.take();
        if self.mode == NetMode::Lock {
            // Locked by the cursor: where it asked for, not the hop the pass
            // happened to be on.
            match self.lock_at.take() {
                Some(target) => NetExit {
                    tune_hz: target.tune_hz,
                    locked: true,
                    why: Some(target.why),
                },
                None => NetExit {
                    tune_hz: tuned_hz,
                    locked: true,
                    why: None,
                },
            }
        } else {
            NetExit {
                tune_hz: pre.unwrap_or(tuned_hz),
                locked: false,
                why: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock the cursor asked for is where the survey hands the tuner back,
    /// with its reason; without one, a lock stays where the pass left it.
    #[test]
    fn a_cursor_lock_is_where_the_survey_hands_the_tuner_back() {
        let mut net = NetState {
            mode: NetMode::Lock,
            pre_survey_hz: Some(100_000_000),
            ..Default::default()
        };
        net.lock_at = Some(LockTarget {
            tune_hz: 2_442_500_000,
            why: "clear of DC".to_string(),
        });
        let exit = net.end(2_437_000_000);
        assert_eq!(exit.tune_hz, 2_442_500_000);
        assert!(exit.locked);
        assert_eq!(exit.why.as_deref(), Some("clear of DC"));
        assert!(net.lock_at.is_none(), "taken, not left to be applied twice");

        let mut net = NetState {
            mode: NetMode::Lock,
            ..Default::default()
        };
        let exit = net.end(2_437_000_000);
        assert_eq!((exit.tune_hz, exit.why), (2_437_000_000, None));
    }

    /// **Locking means "stay here". Leaving the section means "put it back".**
    ///
    /// Treating the two as one exit was the bug: the survey restored the
    /// pre-survey tuning on both, while the key handler logged
    /// `NET locked to <the current hop>`. The log said one thing and the radio
    /// did another, and the user pressed the key precisely because of what they
    /// were looking at.
    #[test]
    fn locking_keeps_the_position_and_leaving_gives_it_back() {
        // A survey started at 2412 and is currently parked on 2442.
        let mut net = NetState {
            pre_survey_hz: Some(2_412_000_000),
            ..Default::default()
        };

        // Locked: the position the pass is on is the one the user meant.
        net.mode = NetMode::Lock;
        let exit = net.end(2_442_000_000);
        assert_eq!(exit.tune_hz, 2_442_000_000);
        assert!(exit.locked);

        // Still surveying and leaving the section: back where it was found.
        let mut net = NetState {
            pre_survey_hz: Some(2_412_000_000),
            ..Default::default()
        };
        let exit = net.end(2_442_000_000);
        assert_eq!(exit.tune_hz, 2_412_000_000);
        assert!(!exit.locked);
    }

    /// Safe on a state that never surveyed, and safe to call twice, because the
    /// quit path calls it unconditionally and the task may have called it first.
    #[test]
    fn ending_a_survey_that_never_ran_leaves_the_radio_alone() {
        let mut net = NetState::default();
        assert_eq!(net.end(2_437_000_000).tune_hz, 2_437_000_000);

        let mut net = NetState {
            pre_survey_hz: Some(2_412_000_000),
            ..Default::default()
        };
        assert_eq!(net.end(2_442_000_000).tune_hz, 2_412_000_000);
        // The task wrote the answer back; quitting must not move it again.
        assert_eq!(net.end(2_412_000_000).tune_hz, 2_412_000_000);
        assert_eq!(net.pre_survey_hz, None);
    }

    /// Leaving the section after locking does not undo the lock.
    ///
    /// The lock already took the interrupted tuning, so there is nothing left to
    /// restore and the radio stays where the user put it - which is what they
    /// asked for and is why `take` rather than a read is the right call.
    #[test]
    fn leaving_the_section_does_not_undo_a_lock() {
        let mut net = NetState {
            pre_survey_hz: Some(2_412_000_000),
            ..Default::default()
        };
        net.mode = NetMode::Lock;
        assert_eq!(net.end(2_442_000_000).tune_hz, 2_442_000_000);
        // Now the user leaves NET entirely, still locked.
        assert_eq!(net.end(2_442_000_000).tune_hz, 2_442_000_000);
    }
}
