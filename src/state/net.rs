// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetState` - what the 2.4 GHz receiver is doing right now.
//!
//! Deliberately small, and it will stay smaller than it looks like it should.
//! What belongs here is the state a *header* has to read, because that is the
//! one thing every panel in the section shares: the rest lives with the panel
//! that produced it. See `dev_docs/net-foundation-design.md` section 9.2.

/// Whether the receiver is walking the band or sitting on one channel.
///
/// **This changes what every number below it means**, which is why it is the
/// first field in the header and why it is never absent. A duty cycle measured
/// while sweeping is a sample of a channel; measured while locked it is that
/// channel's whole story. Design section 13.1 makes every panel say which one it
/// is looking at.
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
    /// colours it, which is the rule for every tag. Design section 13.1: the
    /// mode is part of the reading, so this is not optional for a panel here and
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

/// How every address in the section is shown: foundation design 1.1's switch.
///
/// **It changes the presentation, never the measurement.** The census still
/// keys devices by the full address and the cursor still follows one; only
/// what reaches the screen and the export changes, and it changes everywhere
/// at once (`show`), so a device keeps its identity from panel to panel
/// whichever mode is on. The mode is on the chrome of every panel that
/// prints an address (`ui::panel::Tag::Addresses`), except in `Full`, the
/// default, where there is nothing to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AddressDisplay {
    /// `d1:9a:7e:91:27:9e`: the address, in the written octet order.
    #[default]
    Full,
    /// `A4-83-E7 ..09:be` or `static ..27:9e`: who the address says it
    /// belongs to, and enough of the rest to tell two apart across a room.
    ///
    /// A public address's top three octets are its IEEE OUI, shown in the
    /// IEEE's own hyphenated form so it cannot be mistaken for a whole
    /// address; a vendor name replaces it once a cited registry snapshot
    /// exists (net-ux-polish-plan 1.6.d). A random address has no OUI at all,
    /// so it shows its kind (`signal::ble::address::kind`) instead of a
    /// vendor that would be invented.
    Oui,
    /// `A4-83-E7 #17` or `static   #3`: the same "who" as `Oui`, and a
    /// number instead of any part of the address. For screenshots, demos and
    /// a shared terminal.
    ///
    /// **The number is per session and is not derived from the address**
    /// (foundation design 1.1): [`AddressBook`] hands them out in the order
    /// addresses are first heard, so nothing in a screenshot can be turned
    /// back into an address, and the same device reads `#17` in every panel
    /// and the export for as long as the app runs.
    Masked,
}

impl AddressDisplay {
    /// The next mode, for the one key that cycles them.
    pub fn next(self) -> Self {
        match self {
            Self::Full => Self::Oui,
            Self::Oui => Self::Masked,
            Self::Masked => Self::Full,
        }
    }

    /// The word the chrome tag, the log and the export's provenance use.
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Oui => "oui",
            Self::Masked => "masked",
        }
    }

    /// `addr`, sent with TxAdd = `random`, as this mode shows it; `number` is
    /// its [`AddressBook`] number, which only `Masked` reads.
    ///
    /// **`width` is the column the table has for it, not a fixed size.** A
    /// table gives the address column what the terminal can spare
    /// (`ui::widgets::table::widen`), up to [`Self::natural_width`]: on a wide
    /// screen a registrant's whole name fits, on a narrow one it is cut and
    /// marked `…` (`signal::net::vendor::short_name`), and the rest of the
    /// address (`..09:be`, `#17`) is always whole and at the column's end, so
    /// the rows line up. `None` is the natural form with no padding, for the
    /// export, which is never cut.
    ///
    /// A masked address with no number shows `#-`: every address that reaches
    /// the state is numbered as it arrives, so this is a gap to see, not a
    /// number to invent.
    ///
    /// Without a company: the tests' form; the section calls [`Self::show_with`].
    #[cfg(test)]
    pub fn show(
        self,
        addr: [u8; 6],
        random: bool,
        number: Option<u32>,
        width: Option<usize>,
    ) -> String {
        self.show_with(addr, random, number, None, width)
    }

    /// [`Self::show`], knowing the company the address's manufacturer data
    /// named (`NetState::company`): for a random address, which has no
    /// IEEE block, that company takes the kind's place, marked with where it
    /// came from ([`who_with`]).
    pub fn show_with(
        self,
        addr: [u8; 6],
        random: bool,
        number: Option<u32>,
        company: Option<u16>,
        width: Option<usize>,
    ) -> String {
        let tail = match self {
            Self::Full => {
                return addr
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(":")
            }
            Self::Oui => format!("..{:02x}:{:02x}", addr[4], addr[5]),
            Self::Masked => match number {
                Some(n) => format!("#{n}"),
                None => "#-".to_string(),
            },
        };
        let (name, mark) = who_parts(addr, random, company);
        match width {
            None => format!("{name}{mark} {tail}"),
            Some(w) => {
                // The name gives way, never the mark: `Apple·m…` would have
                // lost the one thing saying where the name came from.
                let room = w.saturating_sub(tail.chars().count() + 1).max(1);
                let name_room = room.saturating_sub(mark.chars().count()).max(1);
                let cut = crate::signal::net::vendor::short_name(&name, name_room);
                let who = format!("{cut}{mark}");
                format!("{who:<room$} {tail}")
            }
        }
    }

    /// The columns `show` needs to print `addr` without cutting anything, and
    /// never less than a full address's 17, so a table sized for the widest
    /// row holds every mode.
    #[cfg(test)]
    pub fn natural_width(self, addr: [u8; 6], random: bool, number: Option<u32>) -> usize {
        self.natural_width_with(addr, random, number, None)
    }

    /// [`Self::natural_width`] for [`Self::show_with`].
    pub fn natural_width_with(
        self,
        addr: [u8; 6],
        random: bool,
        number: Option<u32>,
        company: Option<u16>,
    ) -> usize {
        self.show_with(addr, random, number, company, None)
            .chars()
            .count()
            .max(FULL_ADDRESS_WIDTH)
    }
}

/// `a4:83:e7:1c:09:be`: the narrowest an address column is ever drawn.
pub const FULL_ADDRESS_WIDTH: usize = 17;

/// What [`who_with`] appends to a name read from manufacturer data: where it
/// came from, since it is a different source from the IEEE listing with a
/// different meaning (rule 5).
pub const MFR_MARK: &str = "\u{00b7}mfr";

/// Whose address this is, as far as anything we hold can say.
///
/// A public address is looked up in the IEEE's listing
/// (`signal::net::vendor`): the registrant with its legal form dropped, every
/// registrant where the listing gives several, `private` where the holder hid
/// it, and the block itself in the IEEE's hyphenated form (`A4-83-E7`) where
/// the snapshot does not list it, which is the honest answer and cannot pass
/// for a name.
///
/// **A random address has no IEEE block, but its manufacturer data may name
/// a company** (net-ux-polish-plan 5.4, Viktor's decision of 2026-09-22):
/// `company` is that identifier, shown as the SIG's name for it
/// (`signal::ble::assigned`, its legal form dropped) or the number itself
/// where the snapshot does not list it, marked [`MFR_MARK`] - `Apple·mfr`.
/// It says whose data format the device sends, which for a phone is its
/// maker and for a module may not be; the mark keeps that distinct from an
/// IEEE registrant. Without one a random address is its kind
/// (`signal::ble::address::kind`), never a guessed vendor, and a
/// reserved-kind address keeps saying `reserved` either way: that is the
/// more important fact about it.
///
/// [`AddressDisplay::show_with`] prints it in every mode but `Full`, where the
/// whole address takes its place; a panel with room for both (the census
/// detail block) calls this directly rather than deriving a second answer.
pub fn who_with(addr: [u8; 6], random: bool, company: Option<u16>) -> String {
    let (name, mark) = who_parts(addr, random, company);
    format!("{name}{mark}")
}

/// [`who_with`] as its name and its source mark, apart, so a table can cut
/// the one and keep the other.
fn who_parts(addr: [u8; 6], random: bool, company: Option<u16>) -> (String, &'static str) {
    use crate::signal::ble::address::{kind, AddressKind};
    use crate::signal::net::vendor::short_name;
    match (kind(addr, random), company) {
        (AddressKind::Public | AddressKind::Reserved, _) | (_, None) => {
            (registrant_or_kind(addr, random), "")
        }
        (_, Some(id)) => {
            let name = crate::signal::ble::assigned::company(id)
                .map(|n| short_name(n, usize::MAX))
                .unwrap_or_else(|| format!("0x{id:04X}"));
            (name, MFR_MARK)
        }
    }
}

/// The IEEE registrant of a public address, or a random address's kind.
fn registrant_or_kind(addr: [u8; 6], random: bool) -> String {
    use crate::signal::ble::address::{kind, AddressKind};
    use crate::signal::net::vendor::{registrant, short_name, Registrant};
    match kind(addr, random) {
        AddressKind::Public => match registrant(addr) {
            Registrant::Listed(names) => names
                .iter()
                .map(|n| short_name(n, usize::MAX))
                .collect::<Vec<_>>()
                .join(" / "),
            Registrant::Private => "private".to_string(),
            Registrant::NotListed => {
                format!("{:02X}-{:02X}-{:02X}", addr[0], addr[1], addr[2])
            }
        },
        other => other.label().to_string(),
    }
}

/// The session's masked numbers: each address gets the next one the first
/// time it is heard, and keeps it (foundation design 1.1).
///
/// **Assigned on arrival, not on display**, by `signal::net::worker` as each
/// packet reaches the state, so the number says the order devices were heard
/// in whichever panel happens to be on screen, and switching to `masked`
/// halfway through a session does not number them in the order of one table's
/// sort. Never saved: a number is only meaningful inside the session that
/// gave it.
#[derive(Clone, Debug, Default)]
pub struct AddressBook {
    numbers: std::collections::HashMap<[u8; 6], u32>,
}

impl AddressBook {
    /// `addr`'s number, handing out the next one if this is its first time.
    pub fn number(&mut self, addr: [u8; 6]) -> u32 {
        let next = self.numbers.len() as u32 + 1;
        *self.numbers.entry(addr).or_insert(next)
    }

    /// `addr`'s number, if it has been heard.
    pub fn get(&self, addr: [u8; 6]) -> Option<u32> {
        self.numbers.get(&addr).copied()
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
    /// Packets that have entered [`Self::ble_packets`] this session: each
    /// one's [`BlePacket::seq`] is the count at its arrival.
    pub ble_heard: u64,
    /// How the packet list is being read: which packet the cursor is on.
    pub ble_view: BlePacketView,
    /// The PHY the BLE decoder listens for (net-ux-polish-plan 5.5): LE 1M
    /// unless the user switched, `P` on the packet list.
    pub ble_phy: crate::signal::ble::Phy,
    /// The session's frame error rate against SNR over all BLE traffic
    /// (`signal::ble::fer`, net-ux-polish-plan 5.7): every packet decoded to
    /// its length, good or failed, in its SNR bin.
    pub fer: crate::signal::ble::fer::FerCurve,
    /// Why nothing is being decoded, when the radio can otherwise stream.
    ///
    /// B6's decoder needs the working rate `signal::ble::receive::front_end`
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
    /// questions, so a different gate. B11's own exit condition - "packet
    /// counts per channel, with the dwell fraction stated" - is this field
    /// plus [`NetMode::Survey`]'s rotation always dwelling `1 /
    /// advertising_channels_hz().len()` of a pass on each.
    pub ble_channel_packets: [u64; 3],
    /// Of [`Self::ble_channel_packets`], the ones whose CRC passed, same
    /// indexing: the pair gives each advertising channel its pass rate
    /// (net-ux-polish-plan 5.8). A channel a Wi-Fi network sits on shows
    /// it here first, as a rate that falls while the count keeps rising.
    pub ble_channel_crc_ok: [u64; 3],
    /// Why classic Bluetooth has no live receiver at all right now, on the
    /// `net_bt` preset - the same "refused, not silent" discipline
    /// [`ble_refused`] already follows.
    ///
    /// **Narrowed by B15.** B14 landed `signal::bt::access_code`, the
    /// specification-precision primitive, with no live receiver behind it,
    /// so this was `Some` unconditionally. B15 gives it one -
    /// `signal::bt::receive::Receiver`, one per channel
    /// `signal::bt::channel::channels_in_span` and `[net].bt_channels`
    /// together let the worker watch - so this is now `Some` only when that
    /// receiver genuinely cannot exist (the current tuning's span holds no
    /// classic BT channel at all), and `None` while it is running, the same
    /// as [`ble_refused`]. It says nothing about whether any *hit* has been
    /// found yet; [`Self::bt_piconets`] is what has been.
    pub bt_refused: Option<String>,
    /// Classic Bluetooth access-code hits since the section opened, newest
    /// first, capped at [`BT_HOP_LIMIT`] - B15's own record, one entry per
    /// clean access code any watched channel's
    /// `signal::bt::receive::Receiver` found.
    pub bt_hops: std::collections::VecDeque<BtHop>,
    /// Which classic BT channels the current tuning, span and
    /// `[net].bt_channels` together let the receiver actually watch, low to
    /// high - what `net_bt_hops` reports itself as watching, honestly
    /// narrower than the full 79 (or even the full count a wider capture
    /// could see) whenever the cap is binding. Empty exactly when
    /// [`bt_refused`] is `Some`.
    pub bt_channels_watched: Vec<u8>,
    /// Per-LAP UAP narrowing, B16's own live state, refined by B17's own
    /// payload tie-break: the distinct UAP values `signal::bt::header::
    /// PiconetClock` still cannot rule out for that piconet, from every
    /// header captured on it so far this session - usually exactly two,
    /// not one, `PiconetClock`'s own doc has the measurement that found
    /// that floor and why a header alone cannot go lower. A single
    /// element means `signal::net::worker`'s own `payload::break_uap_tie`
    /// resolved the tie using a real DH1/DH3/DH5 payload's own CRC-16 -
    /// sticky from then on for that LAP, since a piconet's real UAP does
    /// not change mid-session.
    pub bt_uap: std::collections::HashMap<u32, Vec<u8>>,
    /// Every piconet heard this session, one record per LAP
    /// (`signal::bt::piconet`): what the roster draws, counted over the
    /// whole session where [`Self::bt_hops`] keeps a window.
    pub bt_piconets: Vec<crate::signal::bt::piconet::Piconet>,
    /// The roster's cursor, on a LAP: the piconet selected. The hop scatter
    /// shares it, so a piconet picked in either is the one both show.
    pub bt_view: super::Selection<u32>,
    /// How much of the past the hop scatter shows, and how far back it ends.
    pub hop_view: HopView,
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
    /// 2400 MHz). Stop 1.1's one selection model, keyed by the cell so it stays
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
    /// `addr`, sent with TxAdd = `random`, in the section's display mode: the
    /// one call every panel and export makes, so a device reads the same way
    /// everywhere.
    ///
    /// The packets the BLE list shows, newest first: the held copy while the
    /// list is held, the live ring otherwise, narrowed to the filter's
    /// address when there is one.
    ///
    /// **The one account of the list**, read by the panel that draws it and
    /// the keys that move through it, so the arrows step through exactly the
    /// rows on screen.
    pub fn ble_shown(&self) -> Vec<&BlePacket> {
        let source = match &self.ble_view.held {
            Some((held, _)) => held,
            None => &self.ble_packets,
        };
        source
            .iter()
            .filter(|p| self.ble_view.filter.is_none_or(|a| p.adv_addr == Some(a)))
            .collect()
    }

    /// Packets that have arrived since the list was held; zero when it is not.
    pub fn ble_behind(&self) -> u64 {
        self.ble_view
            .held
            .as_ref()
            .map_or(0, |(_, at)| self.ble_heard.saturating_sub(*at))
    }

    /// Whether anything is reading addresses into the census: a BLE decoder
    /// with a channel, or one that has fired this session.
    ///
    /// **The one condition an empty census is read by**, so the panel's empty
    /// state, the decode-health panel's dashes and the export's note cannot
    /// disagree about whether an empty table means a quiet room or nobody
    /// counting.
    pub fn counting_addresses(&self) -> bool {
        self.ble_channel.is_some() || self.health.ble.triggered > 0
    }

    /// The company `addr`'s manufacturer data named, if it has named one.
    pub fn company(&self, addr: [u8; 6]) -> Option<u16> {
        self.advertised.get(&addr).and_then(|a| a.company)
    }

    /// `width` as [`AddressDisplay::show`] takes it: the column the table has,
    /// or `None` for the uncut form an export writes.
    pub fn show_address(&self, addr: [u8; 6], random: bool, width: Option<usize>) -> String {
        self.address_display.show_with(
            addr,
            random,
            self.address_book.get(addr),
            self.company(addr),
            width,
        )
    }

    /// What [`Self::show_address`] needs to print `addr` uncut: the width a
    /// table asks for when it sizes its address column.
    pub fn address_width(&self, addr: [u8; 6], random: bool) -> usize {
        self.address_display.natural_width_with(
            addr,
            random,
            self.address_book.get(addr),
            self.company(addr),
        )
    }

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

/// What the receiver missed, and what it never had a chance to see.
///
/// Design section 13.2: this is not a debug panel, it is testimony. Without it
/// every count in the section is a lower bound presented as a total, because the
/// three ways a sample can go missing are all invisible from downstream. The
/// driver drops them before the block is stamped; the bounded feed refuses whole
/// blocks under load and carries no record that it did; and a run broken in the
/// middle takes whatever was being assembled with it.
///
/// Every field here is a count of something that happened, not a rate and not a
/// judgement. The panel does the dividing.
#[derive(Clone, Debug, Default)]
pub struct NetDecodeHealth {
    /// Blocks that reached the worker.
    pub blocks_in: u64,
    /// I/Q pairs in them.
    pub pairs_in: u64,
    /// Times the stream was interrupted: a block that did not continue the one
    /// before it, for either reason.
    ///
    /// **The first block of a run is not one of these.** Opening the section
    /// mid-stream means the first block to arrive is thousands of sequence
    /// numbers past nothing, and counting that as an interruption would put a
    /// phantom gap on the panel every time the user looked at it.
    pub gaps: u64,
    /// A floor on the blocks that went missing, summed over those gaps.
    ///
    /// A floor because the driver says samples went, never how many: one is the
    /// smallest number that is certainly not an overstatement, and zero would
    /// leave the panel calling a lossy link healthy. Same rule as
    /// `BlockPlan::dropped`, which is where this comes from.
    pub blocks_lost: u64,
    /// Unbroken blocks since the last gap.
    pub run_blocks: u64,
    /// Blocks the bounded feed refused in the last poll window, and since the
    /// radio was opened.
    pub refused: u64,
    pub refused_session: u64,
    /// The deepest the feed queue got in the last window.
    pub peak_depth: u64,
    /// When the last block arrived. `None` before the first one.
    pub last_block: Option<std::time::Instant>,
    /// When the feed last lost anything: an interruption, a block the driver
    /// dropped, or a block the bounded feed refused. `None` if it never has.
    ///
    /// The counters above say *how much* went missing; this says *when*, which
    /// is what a panel's feed-loss caveat turns on (`ui::panel::FeedSpan`): a
    /// drop ten minutes ago undercounts a session's census and says nothing
    /// about the dwell that just finished.
    pub last_loss: Option<std::time::Instant>,
    /// How much of real time the worker spends processing the stream: the
    /// wall time it took to handle the last stretch of blocks, over the
    /// stream time those blocks covered. `0.62` is 62 %; above `1.0` the
    /// worker is falling behind and the bounded feed will start refusing
    /// blocks. `None` until a stretch has been measured.
    ///
    /// Foundation design 12.4 makes this a displayed number rather than a
    /// hidden one: every decoder added to the worker spends from it.
    pub decode_load: Option<f64>,
    /// The BLE decode funnel since the section opened
    /// (`signal::ble::receive::Funnel`): triggers, and how each one ended.
    pub ble: crate::signal::ble::receive::Funnel,
    /// Classic access-code hits since the section opened, every one, where
    /// `bt_hops` keeps only the latest few hundred.
    pub bt_hits: u64,
}

/// How many recent PDUs [`NetState::ble_packets`] keeps. A bench instrument
/// is read a screenful at a time, not scrolled back through a session's
/// worth of advertising traffic; old rows fall off the end rather than
/// growing the list forever.
pub const BLE_PACKET_LIMIT: usize = 200;

/// The hop scatter's zoom steps, in milliseconds: from half a second, where
/// single hops of one piconet separate, to a minute, where piconets come and
/// go.
pub const HOP_WINDOWS_MS: [u64; 7] = [500, 1_000, 2_000, 5_000, 10_000, 20_000, 60_000];

/// Which stretch of time the classic hop scatter shows
/// (net-ux-polish-plan 6.2): a zoom step, and how far before now it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HopView {
    /// Index into [`HOP_WINDOWS_MS`].
    pub zoom: usize,
    /// How far before now the view ends, ms; `0` is live.
    pub back_ms: u64,
}

impl Default for HopView {
    /// Twenty seconds ending now: what the scatter showed before it could
    /// zoom.
    fn default() -> Self {
        Self {
            zoom: 5,
            back_ms: 0,
        }
    }
}

impl HopView {
    pub fn span_ms(&self) -> u64 {
        HOP_WINDOWS_MS[self.zoom.min(HOP_WINDOWS_MS.len() - 1)]
    }

    /// A shorter window, keeping where it ends.
    pub fn zoom_in(&mut self) {
        self.zoom = self.zoom.saturating_sub(1);
    }

    /// A longer window, keeping where it ends.
    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom + 1).min(HOP_WINDOWS_MS.len() - 1);
    }

    /// Move a quarter of a window back in time, but not past `oldest_ms`,
    /// the age of the oldest hit kept: a view that ends before anything was
    /// kept would show an empty plot of nothing known.
    pub fn back(&mut self, oldest_ms: u64) {
        self.back_ms = (self.back_ms + self.span_ms() / 4).min(oldest_ms);
    }

    /// Move a quarter of a window toward now.
    pub fn forward(&mut self) {
        self.back_ms = self.back_ms.saturating_sub(self.span_ms() / 4);
    }
}

/// How many recent hits [`NetState::bt_hops`] keeps - a scatter plots a
/// recent time window, not a session's worth of hops, and old rows fall off
/// the end the same way [`BLE_PACKET_LIMIT`] already does for advertising
/// PDUs.
pub const BT_HOP_LIMIT: usize = 500;

/// One classic Bluetooth access-code hit, as [`ui::panels::net::bt_hops::
/// NetBtHopsPanel`] plots it: which channel found it, the LAP the access
/// code carries (free at detection time - see
/// `signal::bt::access_code::find_access_code`'s own doc), and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BtHop {
    pub channel: u8,
    /// The piconet it belongs to: what `net_bt_hops` colours it by
    /// (net-ux-polish-plan 6.2), free at detection time.
    pub lap: u32,
    pub seen: std::time::Instant,
    /// When its access code ended, µs on the stream's sample clock
    /// (`signal::bt::receive::AccessHit::at_us`), and which stream: a new
    /// stream or rate starts that clock again, so a time is only comparable
    /// to one of the same `stream` (net-ux-polish-plan 6.6).
    pub at_us: f64,
    pub stream: u32,
    /// What its header said, once one was captured and joined to it: the
    /// export's header columns. `None` when no header followed.
    pub header: Option<crate::signal::bt::piconet::HeaderRead>,
}

/// One decoded advertising channel PDU, as a panel shows it.
///
/// `crate::signal::ble::pdu::Packet` is the decode itself, pure and knowing
/// nothing about a screen; this adds the two facts a panel needs that decode
/// alone does not carry - which channel it arrived on and when.
#[derive(Clone, Debug)]
pub struct BlePacket {
    /// Its place in the session's arrivals, from 1: what a selection holds on
    /// to, since the ring's positions shift with every packet
    /// (`NetState::ble_heard`).
    pub seq: u64,
    pub channel: u8,
    pub pdu_type: crate::signal::ble::pdu::PduType,
    /// The PHY it was received on: the packet's own, not whatever the
    /// decoder is set to now, since the list outlives a switch.
    pub phy: crate::signal::ble::Phy,
    /// ChSel, where the type defines it (`pdu::Packet::ch_sel`).
    pub ch_sel: bool,
    pub tx_add_random: bool,
    pub rx_add_random: bool,
    pub length: u8,
    pub adv_addr: Option<[u8; 6]>,
    /// The PDU's payload as decoded (`pdu::Packet::payload`): what the AD
    /// structures and a CONNECT_IND's parameters are read from. Bounded by
    /// the ring (`BLE_PACKET_LIMIT`) and by the length field's 6 bits.
    pub payload: Vec<u8>,
    pub crc_ok: bool,
    /// B7: read from the detector's own coherence at the moment this
    /// packet's sync word was found. `None` only at a coherence of one -
    /// noiseless, which does not happen on a radio - never because nothing
    /// was measured.
    pub snr_db: Option<f64>,
    /// The carrier offset as received, in Hz: estimated from the sync word
    /// against its known waveform (`signal::ble::receive`'s data-aided
    /// estimate), with its uncertainty. **Their crystal's error minus our
    /// oscillator's**, never corrected here: the panels take it through
    /// `RadioState::transmitter_offset`, which removes ours when a reference
    /// allows, and the chrome says which of the two the number on screen is.
    pub freq_offset_hz: Option<crate::signal::dsp::uncertainty::Uncertain>,
    /// B8: modulation index, delta-f1 average, delta-f2 maximum and their
    /// ratio, measured from this packet's own on-air symbols. `None` when
    /// the packet was too short, or too unlucky in its particular random
    /// content, to contain a settled run of either kind - see
    /// `signal::ble::measure`'s own doc for what "settled" means here.
    pub modulation: Option<crate::signal::ble::measure::ModulationQuality>,
    /// B9: this packet's own frequency offset, read early and late, and the
    /// drift between them. `None` under the same conditions as
    /// `modulation` - too short a capture to give each half its own
    /// variance.
    pub drift: Option<crate::signal::ble::measure::Drift>,
    pub seen: std::time::Instant,
}

/// How the BLE packet list is being read (net-ux-polish-plan 5.3).
///
/// The cursor holds a packet's [`BlePacket::seq`], not a row: the list is
/// newest first, so every arrival moves every row, and a cursor on a row
/// number would slide to a different packet each time one came in.
#[derive(Clone, Debug, Default)]
pub struct BlePacketView {
    pub selection: super::Selection<u64>,
    /// Only packets from this advertiser address, when set.
    pub filter: Option<[u8; 6]>,
    /// The list as it stood when it was held, and [`NetState::ble_heard`] at
    /// that moment: a copy, so holding the list stops nothing else. The
    /// coexistence marks and the census go on taking every packet.
    pub held: Option<(std::collections::VecDeque<BlePacket>, u64)>,
}

/// How the census table is being read: what orders it, and where the cursor is.
///
/// The cursor is an address, not a row number - see [`super::Selection`],
/// which the census was the first to need and which every other NET list now
/// shares.
#[derive(Clone, Debug, Default)]
pub struct CensusState {
    /// The population, unordered.
    ///
    /// **Stored as found, ordered when drawn.** Keeping it sorted would mean
    /// re-sorting on every packet for a question only the panel asks, and would
    /// put the display's choice of column into the place the measurements live.
    pub devices: Vec<crate::signal::net::census::Device>,
    /// Index into `signal::net::census::SORT_KEYS`.
    pub sort: usize,
    pub descending: bool,
    /// The device the cursor is on, and where the view starts.
    pub selection: super::Selection<[u8; 6]>,
    /// The device selected when the census last lost focus, kept for one
    /// thing only: a layout switch into the BLE list carries it as the
    /// list's filter (`input::net::carry_census_selection`). Leaving focus
    /// clears the selection itself, as it clears every cursor, and the carry
    /// reads this instead; it is spent by the carry that uses it.
    pub chosen: Option<[u8; 6]>,
}

impl CensusState {
    /// The next column along, wrapping.
    ///
    /// One key, cycling, because design section 9.1 asks for the sort key to be
    /// *shown* rather than remembered - and a control the panel advertises in
    /// one direction is one the user can use without being told twice.
    pub fn cycle_sort(&mut self) {
        let keys = crate::signal::net::census::SORT_KEYS.len().max(1);
        self.sort = (self.sort + 1) % keys;
        // Back to the top of the new column rather than wherever the last one
        // left the view: the rows underneath are different rows now.
        self.selection.reset_view();
    }

    pub fn reverse(&mut self) {
        self.descending = !self.descending;
        self.selection.reset_view();
    }

    /// The population in the order the table shows it.
    ///
    /// **The one ordering both the panel and its keys read.** The cursor moves
    /// through rows as drawn; if the key handler ordered the census on its own
    /// terms, or not at all, the cursor would step through a list nobody can
    /// see. It once did exactly that: the handler moved through an empty list,
    /// so the arrows never selected anything.
    pub fn ordered(
        &self,
        now: std::time::Instant,
        radio: &super::RadioState,
    ) -> Vec<crate::signal::net::census::Device> {
        let mut devices = self.devices.clone();
        crate::signal::net::census::order(&mut devices, self.sort, self.descending, now, radio);
        devices
    }

    /// The addresses of [`Self::ordered`], which is what the cursor moves
    /// through.
    pub fn ordered_addresses(
        &self,
        now: std::time::Instant,
        radio: &super::RadioState,
    ) -> Vec<[u8; 6]> {
        self.ordered(now, radio).iter().map(|d| d.address).collect()
    }
}

/// How often the band is written onto the time axis.
///
/// Half a second. A canvas column is a moment, and this is about the finest a
/// person reads a minute-long picture at; faster columns would cost memory and
/// redraw for detail nobody can resolve at two metres, which is the acceptance
/// criterion for the panel that draws them.
pub const COLUMN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// How much of the past the canvas keeps.
///
/// Two minutes at [`COLUMN_INTERVAL`], which is long enough to see a device come
/// and go and short enough that eighty-three cells of it is under a hundred
/// kilobytes.
pub const HISTORY_COLUMNS: usize = 240;

/// One megahertz of the band, as measured over the last dwell.
///
/// A cell that was never inside the observed span has `windows` of zero, and
/// that is the difference between "nothing was transmitting here" and "nobody
/// looked here". The panel draws them differently, because they are different
/// answers and rule 2 is about exactly this.
#[derive(Clone, Copy, Debug, Default)]
pub struct CellReading {
    /// Transform windows this cell was measured over.
    pub windows: u64,
    /// Fraction of them with something in this cell, with the false-alarm floor
    /// removed. Zero is a measurement.
    pub duty: f64,
    /// Mean and peak power in the cell, relative to the converter's full scale.
    pub mean_dbfs: f64,
    pub peak_dbfs: f64,
    /// What fraction of wall time this cell has actually been under observation.
    ///
    /// About one in [`crate::signal::net::survey::Plan::hops`] while surveying,
    /// and one while locked. `None` until there is any elapsed time to be a
    /// fraction of.
    ///
    /// **Measured over the whole watch, not between the last two dwells**, and
    /// the difference is not subtle. A dwell publishes every fifty milliseconds
    /// of observation and a hop lasts a hundred, so two dwells land inside one
    /// visit - and the gap between *those* two is fifty milliseconds of looking
    /// in fifty milliseconds of wall clock, which is one. On a live radio with
    /// five positions this read 87 % where the honest answer is 16 %.
    ///
    /// **The duty cycle is not scaled by this**, and the temptation to is worth
    /// naming: a channel busy all the time, seen for a sixth of the time, is
    /// busy all the time. Scaling its reading to sixteen percent would not be a
    /// sampled measurement, it would be a wrong one. What sampling costs is
    /// certainty, not magnitude, and that is carried by the window count and
    /// reported as an uncertainty.
    pub coverage: Option<f64>,
    /// When this cell was last measured.
    ///
    /// Rule 4 says all testimony is dated, and a survey that has not come back
    /// to a cell for a minute is showing a minute-old reading. The occupancy
    /// panel reads the oldest of these as the span its numbers cover, for the
    /// feed-loss caveat, and prints the selected cell's own age in its cursor
    /// readout.
    pub measured: Option<std::time::Instant>,
    /// Seconds this cell has been under observation since the watch began.
    ///
    /// The numerator of [`Self::coverage`]. Accumulated rather than differenced,
    /// because what a survey costs is only visible over a whole pass and a
    /// difference between two dwells cannot see one.
    pub observed_s: f64,
}

impl CellReading {
    /// Whether anybody looked here.
    pub fn observed(&self) -> bool {
        self.windows > 0
    }
}

/// The band as last measured, one megahertz at a time.
#[derive(Clone, Debug, Default)]
pub struct BandOccupancy {
    /// Empty until the first dwell completes; `occupancy::CELLS` long after.
    pub cells: Vec<CellReading>,
    /// The receiver's own noise floor, which every duty cycle here was measured
    /// against. `None` before the first dwell.
    pub noise_dbfs: Option<f64>,
    /// Whether the floor's preconditions held. When they did not, nothing here
    /// is a measurement and the panel says so rather than drawing it.
    pub trusted: bool,
    /// What the plane looked like, kept because it is the reason `trusted` is
    /// what it is and a panel that only showed the verdict would be asking to be
    /// believed.
    pub tail: f64,
    pub spread: f64,
    /// The band as it was, one column per moment, oldest first.
    ///
    /// **A column is a moment, not a pass.** The survey refreshes any one cell
    /// once per pass, so a column taken faster than that repeats the last
    /// measurement for most cells - which is what the cell's value *is*, and the
    /// coverage line already says how often it is renewed. Tying columns to
    /// passes instead would make the time axis stop when the mode is locked.
    pub history: std::collections::VecDeque<Vec<f32>>,
    /// When the newest column was taken.
    pub last_column: Option<std::time::Instant>,
    /// How many columns have ever been taken, so a column has an identity
    /// that survives the history scrolling: the newest is `columns_taken - 1`,
    /// and one `back` steps before it is `columns_taken - 1 - back`. The time
    /// cursor remembers a moment by this, not by its place on screen.
    pub columns_taken: u64,
    /// When the coverage accounting began.
    ///
    /// Restarted when the mode changes, because survey and lock are different
    /// regimes and averaging across the switch would describe neither.
    pub watch_start: Option<std::time::Instant>,
    /// How long one transform window was. The resolution every duty cycle here
    /// was measured at, and what turns a window count back into seconds.
    pub window_s: f64,
}

impl BandOccupancy {
    /// Fold one dwell into the band, keeping every cell the dwell did not see.
    ///
    /// **This is what makes a survey a survey.** Each dwell measures the slice
    /// the radio was pointed at; the rest of the band keeps what the last pass
    /// found there, with the time it was found. A dwell that replaced the whole
    /// band would leave a receiver seeing a fifth of it reporting the other four
    /// fifths as unobserved on every frame, which is a picture of the receiver
    /// rather than of the band.
    ///
    /// The floor is the receiver's rather than the position's, so the newest one
    /// wins outright: it is a fact about the front end at this gain, and the
    /// front end does not change between hops.
    pub fn absorb(&mut self, dwell: BandOccupancy, now: std::time::Instant) {
        if self.cells.len() != dwell.cells.len() {
            self.cells = vec![CellReading::default(); dwell.cells.len()];
        }
        // The watch begins when the *observing* began, not when the first dwell
        // was published: that dwell already carries the time it took to gather,
        // and counting it against a clock that started afterwards makes the
        // first reading look better than it is.
        let first_dwell =
            dwell.cells.iter().map(|c| c.windows).max().unwrap_or(0) as f64 * dwell.window_s;
        let watch_start = *self
            .watch_start
            .get_or_insert(now - std::time::Duration::from_secs_f64(first_dwell.max(0.0)));
        let watched = now.saturating_duration_since(watch_start).as_secs_f64();
        for (old, new) in self.cells.iter_mut().zip(dwell.cells.iter()) {
            if new.windows == 0 {
                continue;
            }
            let observed_s = old.observed_s + new.windows as f64 * dwell.window_s;
            // A fraction needs something to be a fraction of, and until the
            // watch has run for a moment there is nothing.
            let coverage = (watched > 0.0).then(|| (observed_s / watched).min(1.0));
            *old = CellReading {
                coverage,
                measured: Some(now),
                observed_s,
                ..*new
            };
        }
        self.noise_dbfs = dwell.noise_dbfs;
        self.trusted = dwell.trusted;
        self.tail = dwell.tail;
        self.spread = dwell.spread;
        self.window_s = dwell.window_s;
        self.record_column(now);
    }

    /// Push the band as it stands onto the history, if it is time for a column.
    fn record_column(&mut self, now: std::time::Instant) {
        let due = self
            .last_column
            .is_none_or(|t| now.saturating_duration_since(t) >= COLUMN_INTERVAL);
        if !due || self.cells.is_empty() {
            return;
        }
        self.last_column = Some(now);
        // Unobserved reads as a negative, so the canvas can draw "nobody looked
        // here" differently from "nothing was here" - the same distinction the
        // occupancy profile makes, carried into the time axis.
        self.history.push_back(
            self.cells
                .iter()
                .map(|c| if c.observed() { c.duty as f32 } else { -1.0 })
                .collect(),
        );
        self.columns_taken += 1;
        while self.history.len() > HISTORY_COLUMNS {
            self.history.pop_front();
        }
    }

    /// How many steps back from the newest column the column `id` is, while
    /// the history still holds it.
    pub fn back_of(&self, id: u64) -> Option<usize> {
        let back = self.columns_taken.checked_sub(1)?.checked_sub(id)? as usize;
        (back < self.history.len()).then_some(back)
    }

    /// The identity of the column `back` steps before the newest.
    pub fn id_back(&self, back: usize) -> Option<u64> {
        (back < self.history.len()).then(|| self.columns_taken - 1 - back as u64)
    }

    /// The column `id`, while the history holds it: each cell's duty, negative
    /// where nobody looked.
    pub fn column(&self, id: u64) -> Option<&Vec<f32>> {
        let back = self.back_of(id)?;
        self.history.get(self.history.len() - 1 - back)
    }

    /// Start the coverage accounting again.
    ///
    /// Called when the mode changes. The measurements themselves are kept: they
    /// are still what was on the air. What is thrown away is the accounting of
    /// how often we were looking, because that is the thing the mode changed.
    pub fn restart_watch(&mut self) {
        self.watch_start = None;
        for c in self.cells.iter_mut() {
            c.observed_s = 0.0;
            c.coverage = None;
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

    /// **A column keeps its identity while the history scrolls under it.**
    /// A new column pushes it one step further back, the same id finds the
    /// same values, and once it falls off the end it is gone rather than
    /// quietly naming its neighbour.
    #[test]
    fn a_history_column_is_found_by_its_identity_as_the_history_scrolls() {
        let mut band = BandOccupancy::default();
        let push = |band: &mut BandOccupancy, v: f32| {
            band.history.push_back(vec![v; 3]);
            band.columns_taken += 1;
            while band.history.len() > 4 {
                band.history.pop_front();
            }
        };
        for v in [0.1, 0.2, 0.3] {
            push(&mut band, v);
        }
        let id = band.id_back(1).unwrap();
        assert_eq!(band.column(id).unwrap()[0], 0.2);
        push(&mut band, 0.4);
        assert_eq!(band.back_of(id), Some(2), "one step further back");
        assert_eq!(
            band.column(id).unwrap()[0],
            0.2,
            "and still the same moment"
        );
        push(&mut band, 0.5);
        push(&mut band, 0.6);
        assert_eq!(
            band.back_of(id),
            None,
            "scrolled out of a four-column history"
        );
        assert!(band.column(id).is_none());
        assert_eq!(band.id_back(4), None);
    }

    /// Each mode shows what foundation design 1.1 promises, uncut for an
    /// export, and laid exactly into whatever column a table gives it: the
    /// name grows into a wide column and is cut and marked in a narrow one,
    /// while the rest of the address stays whole at the column's end.
    #[test]
    fn each_address_mode_shows_what_it_promises_in_any_width() {
        use AddressDisplay::{Full, Masked, Oui};
        let apple = [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0xbe];
        let samsung = [0x00, 0x00, 0xf0, 0x12, 0x34, 0x56];
        let hidden = [0xe4, 0xf1, 0x4c, 0x00, 0x00, 0x01];
        let unlisted = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
        let static_random = [0xd1, 0x9a, 0x7e, 0x91, 0x27, 0x9e];
        let rpa = [0x4f, 0x00, 0x11, 0x22, 0x33, 0x44];

        assert_eq!(Full.show(apple, false, None, Some(40)), "a4:83:e7:1c:09:be");
        // Uncut, for the export.
        assert_eq!(Oui.show(apple, false, None, None), "Apple ..09:be");
        assert_eq!(
            Oui.show(samsung, false, None, None),
            "Samsung Electronics ..34:56"
        );
        assert_eq!(Oui.show(hidden, false, None, None), "private ..00:01");
        assert_eq!(Oui.show(unlisted, false, None, None), "02-00-00 ..00:01");
        assert_eq!(Oui.show(static_random, true, None, None), "static ..27:9e");
        assert_eq!(Masked.show(rpa, true, Some(3), None), "RPA #3");
        assert_eq!(Masked.show(rpa, true, None, None), "RPA #-");
        // In a full address's width the name is cut and marked...
        assert_eq!(
            Oui.show(samsung, false, None, Some(17)),
            "Samsung…  ..34:56"
        );
        assert_eq!(
            Masked.show(apple, false, Some(17), Some(17)),
            "Apple         #17"
        );
        // ...and on a wider screen it is whole.
        assert_eq!(
            Oui.show(samsung, false, None, Some(30)),
            "Samsung Electronics    ..34:56"
        );
        for mode in [Full, Oui, Masked] {
            for (a, r) in [
                (apple, false),
                (samsung, false),
                (static_random, true),
                (rpa, true),
            ] {
                for w in [17, 20, 26, 40] {
                    let shown = mode.show(a, r, Some(99_999), Some(w));
                    let want = if mode == Full { 17 } else { w };
                    assert_eq!(shown.chars().count(), want, "{mode:?} at {w}: {shown:?}");
                }
                assert!(mode.natural_width(a, r, Some(1)) >= FULL_ADDRESS_WIDTH);
            }
        }
    }

    /// The same device reads the same way in every mode switch cycle: `next`
    /// visits every mode and comes back.
    #[test]
    fn the_address_modes_cycle_back_to_full() {
        let mut m = AddressDisplay::Full;
        let mut seen = vec![m];
        loop {
            m = m.next();
            if m == AddressDisplay::Full {
                break;
            }
            assert!(!seen.contains(&m), "{m:?} came round twice");
            seen.push(m);
        }
        assert_eq!(seen.len(), 3);
    }

    /// **A masked number is the order of first hearing, and nothing about the
    /// address.** The same two addresses heard in the other order get each
    /// other's numbers, which is the proof the number carries no trace of the
    /// address; and hearing one again does not renumber it.
    #[test]
    fn a_masked_number_is_the_order_of_first_hearing() {
        let a = [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0xbe];
        let b = [0xd1, 0x9a, 0x7e, 0x91, 0x27, 0x9e];

        let mut book = AddressBook::default();
        assert_eq!(book.number(a), 1);
        assert_eq!(book.number(b), 2);
        assert_eq!(book.number(a), 1, "heard again, same number");
        assert_eq!(book.get(b), Some(2));

        let mut other = AddressBook::default();
        assert_eq!(other.number(b), 1);
        assert_eq!(other.number(a), 2);

        assert_eq!(AddressBook::default().get(a), None);
    }
    use std::time::{Duration, Instant};

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

    /// A new column or a reversed one puts different rows under the view, so
    /// the view goes back to the top - but the device the cursor is on is
    /// still that device, and stays selected.
    #[test]
    fn a_resort_resets_the_view_and_keeps_the_device() {
        let device = [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0x01];
        let mut c = CensusState::default();
        c.selection.move_by(&[device], 1);
        c.selection.first_visible = 5;
        c.cycle_sort();
        assert_eq!(c.selection.first_visible, 0);
        assert_eq!(c.selection.selected, Some(device));
        c.selection.first_visible = 5;
        c.reverse();
        assert_eq!(c.selection.first_visible, 0);
        assert_eq!(c.selection.selected, Some(device));
    }

    #[test]
    fn cycling_the_sort_key_walks_the_columns_and_comes_back() {
        let keys = crate::signal::net::census::SORT_KEYS.len();
        let mut c = CensusState::default();
        assert_eq!(c.sort, 0);
        for expected in 1..keys {
            c.cycle_sort();
            assert_eq!(c.sort, expected);
        }
        c.cycle_sort();
        assert_eq!(c.sort, 0, "and round again");

        // Reversing is a separate act from choosing the column.
        assert!(!c.descending);
        c.reverse();
        assert!(c.descending);
        assert_eq!(c.sort, 0, "reversing does not move the column");
    }

    /// One dwell's worth of band: `cells` measured, the rest untouched.
    fn dwell(cells: &[(usize, f64, u64)]) -> BandOccupancy {
        let mut out = BandOccupancy {
            cells: vec![CellReading::default(); 83],
            noise_dbfs: Some(-78.0),
            trusted: true,
            tail: 2.1,
            spread: 30.0,
            window_s: 6.4e-6,
            watch_start: None,
            // A dwell has no past: the history is the band's, and `absorb`
            // owns it.
            history: Default::default(),
            last_column: None,
            columns_taken: 0,
        };
        for &(c, duty, windows) in cells {
            out.cells[c] = CellReading {
                windows,
                duty,
                mean_dbfs: -60.0,
                peak_dbfs: -30.0,
                coverage: None,
                measured: None,
                observed_s: 0.0,
            };
        }
        out
    }

    /// **This is what makes a survey a survey.** Each dwell sees one slice; the
    /// band keeps what the last pass found everywhere else.
    ///
    /// Without it, a receiver seeing a fifth of the band would report the other
    /// four fifths as unobserved on every frame, which is a picture of the
    /// receiver rather than of the band, and the panel's whole
    /// observed-versus-unobserved distinction would collapse to "wherever the
    /// radio happens to be pointed this instant".
    #[test]
    fn a_dwell_folds_into_the_band_rather_than_replacing_it() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();

        band.absorb(dwell(&[(10, 0.4, 8_000)]), t0);
        assert_eq!(band.cells[10].duty, 0.4);
        assert!(band.cells[10].observed());

        // A second dwell somewhere else does not take the first one with it.
        band.absorb(dwell(&[(60, 0.9, 8_000)]), t0 + Duration::from_millis(300));
        assert_eq!(band.cells[60].duty, 0.9);
        assert_eq!(
            band.cells[10].duty, 0.4,
            "the other end of the band is still what the last pass found"
        );
        assert!(band.cells[10].observed());
        // And a cell no pass has reached yet is still unobserved, which is a
        // different answer from empty.
        assert!(!band.cells[30].observed());
    }

    /// Sampling costs certainty, not magnitude.
    ///
    /// A channel busy all the time, watched a sixth of the time, is busy all the
    /// time. Scaling its reading to sixteen percent would not be a sampled
    /// measurement, it would be a wrong one - and it is the obvious thing to
    /// write, which is why it is asserted against.
    ///
    /// The coverage itself is the second half: it is a running average over the
    /// whole watch, so it converges on the fraction of wall time the radio
    /// actually spends here. Measured between the last two dwells instead, it
    /// read 87 % on a live five-position survey, because two dwells fit inside
    /// one hop and the gap between *those* is all observation.
    #[test]
    fn the_coverage_is_reported_and_never_multiplied_into_the_duty_cycle() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();

        // A saturated cell, visited once per 625 ms pass. **Two dwells land
        // inside each visit**, fifty milliseconds apart, because the scan
        // publishes every fifty milliseconds of observation and a hop lasts a
        // hundred. That is the shape the old measure got wrong: the gap between
        // those two is all observation, so it read one, and the panel showed
        // 87 % on a five-position survey.
        for pass in 0..40u32 {
            let visit = t0 + Duration::from_millis(625 * pass as u64);
            band.absorb(dwell(&[(10, 1.0, 8_000)]), visit);
            band.absorb(
                dwell(&[(10, 1.0, 8_000)]),
                visit + Duration::from_millis(51),
            );
        }
        assert_eq!(band.cells[10].duty, 1.0, "still busy all the time");

        let coverage = band.cells[10].coverage.expect("a watch has run");
        // Two dwells of 51 ms in every 625: a hundred milliseconds of looking a
        // pass, which is the sixth a five-position survey spends here.
        let want = 2.0 * 0.0512 / 0.625;
        assert!(
            (coverage - want).abs() < 0.005,
            "51 ms of looking in every 625: wanted {want:.3}, got {coverage:.3}"
        );
    }

    /// The very first reading is not flattered by a clock that started after the
    /// observing did.
    #[test]
    fn the_watch_begins_when_the_looking_did() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();
        band.absorb(dwell(&[(10, 1.0, 8_000)]), t0);
        // One dwell, and nothing but that dwell has happened: the radio has been
        // looking here the whole time it has been looking at all.
        let coverage = band.cells[10].coverage.expect("a watch has run");
        assert!((coverage - 1.0).abs() < 1e-9, "got {coverage}");
    }

    /// Locked, the receiver is looking almost all the time, and the coverage
    /// says so rather than being pinned to one by the mode.
    #[test]
    fn locking_shows_as_coverage_rather_than_being_assumed() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();
        // Back to back: 51 ms of looking every 52 ms of clock.
        for i in 0..40u32 {
            band.absorb(
                dwell(&[(10, 0.3, 8_000)]),
                t0 + Duration::from_millis(52 * i as u64),
            );
        }
        let coverage = band.cells[10].coverage.unwrap();
        assert!(coverage > 0.95, "{coverage}");
        assert!(
            coverage <= 1.0,
            "never more than all of the time: {coverage}"
        );
    }

    /// Switching mode starts the accounting again, and keeps the measurements.
    ///
    /// Survey and lock are different regimes for how often the radio looks at
    /// any one megahertz. Averaging across the switch would describe neither,
    /// and the reading would take a minute to catch up with what the user just
    /// did.
    #[test]
    fn changing_mode_restarts_the_watch_but_keeps_what_was_measured() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();
        for pass in 0..20u32 {
            band.absorb(
                dwell(&[(10, 0.42, 8_000)]),
                t0 + Duration::from_millis(625 * pass as u64),
            );
        }
        assert!(band.cells[10].coverage.unwrap() < 0.2);

        band.restart_watch();
        assert_eq!(band.cells[10].coverage, None, "nothing to be a fraction of");
        assert_eq!(band.cells[10].observed_s, 0.0);
        assert_eq!(band.cells[10].duty, 0.42, "the measurement stands");
        assert!(band.cells[10].observed(), "and the cell is still observed");
    }

    /// The floor is the receiver's, not the position's, so the newest wins.
    #[test]
    fn the_newest_floor_is_the_bands_floor() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();
        band.absorb(dwell(&[(10, 0.3, 8_000)]), t0);
        let mut second = dwell(&[(60, 0.3, 8_000)]);
        second.noise_dbfs = Some(-71.0);
        second.trusted = false;
        band.absorb(second, t0 + Duration::from_millis(300));
        assert_eq!(band.noise_dbfs, Some(-71.0));
        assert!(!band.trusted, "a front end on its rails is on its rails");
    }

    /// **A random address with manufacturer data says whose format it is**,
    /// marked as coming from there: the SIG's name with its legal form off,
    /// or the number where the snapshot does not list it. Without the data it
    /// stays its kind; a public address keeps its IEEE registrant; a reserved
    /// kind keeps saying so.
    #[test]
    fn a_random_address_names_its_manufacturer_data_s_company() {
        use AddressDisplay::*;
        let rpa = [0x4a, 0x11, 0x22, 0x33, 0x09, 0xbe];
        assert_eq!(
            Oui.show_with(rpa, true, None, Some(0x004C), None),
            "Apple\u{00b7}mfr ..09:be"
        );
        assert_eq!(
            Oui.show_with(rpa, true, None, Some(0x7FFF), None),
            "0x7FFF\u{00b7}mfr ..09:be"
        );
        assert_eq!(Oui.show_with(rpa, true, None, None, None), "RPA ..09:be");
        assert_eq!(
            Masked.show_with(rpa, true, Some(3), Some(0x004C), None),
            "Apple\u{00b7}mfr #3"
        );

        let public = [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0xbe];
        assert_eq!(
            Oui.show_with(public, false, None, Some(0x0006), None),
            "Apple ..09:be"
        );
        let reserved = [0x80, 0, 0, 0, 0x09, 0xbe];
        assert_eq!(
            Oui.show_with(reserved, true, None, Some(0x004C), None),
            "reserved ..09:be"
        );
    }

    /// **The name gives way to a narrow column, the mark never does**: cut
    /// to fit, `…` at the cut, `·mfr` whole after it.
    #[test]
    fn a_cut_company_keeps_its_source_mark() {
        let rpa = [0x4a, 0x11, 0x22, 0x33, 0x09, 0xbe];
        // 0x0075: Samsung Electronics Co. Ltd., long enough to be cut.
        let shown = AddressDisplay::Oui.show_with(rpa, true, None, Some(0x0075), Some(17));
        assert_eq!(shown.chars().count(), 17, "{shown}");
        assert!(shown.contains("\u{2026}\u{00b7}mfr ..09:be"), "{shown}");
    }

    /// Through the section's own account: what the worker learned about an
    /// address is how every panel shows it.
    #[test]
    fn the_section_shows_an_address_with_the_company_it_learned() {
        let rpa = [0x4a, 0x11, 0x22, 0x33, 0x09, 0xbe];
        let mut net = NetState {
            address_display: AddressDisplay::Oui,
            ..NetState::default()
        };
        assert_eq!(net.show_address(rpa, true, None), "RPA ..09:be");
        net.advertised.entry(rpa).or_default().company = Some(0x004C);
        assert_eq!(
            net.show_address(rpa, true, None),
            "Apple\u{00b7}mfr ..09:be"
        );
        assert!(net.address_width(rpa, true) >= "Apple\u{00b7}mfr ..09:be".chars().count());
    }
}
