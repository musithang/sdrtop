// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! How the section shows who sent something: an address, a name, raw bytes,
//! a LAP, an access address or a UAP, under the masking mode the user chose,
//! so a device reads the same way on every panel and in every export.

use super::NetState;

/// How every address in the section is shown.
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
    /// exists. A random address has no OUI at all,
    /// so it shows its kind (`signal::ble::address::kind`) instead of a
    /// vendor that would be invented.
    Oui,
    /// `A4-83-E7 #17` or `static   #3`: the same "who" as `Oui`, and a
    /// number instead of any part of the address. For screenshots, demos and
    /// a shared terminal.
    ///
    /// **The number is per session and is not derived from the address**:
    /// [`AddressBook`] hands them out in the order
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

    /// [`Self::show_with`] with no company: the tests' form.
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

    /// `addr`, sent with TxAdd = `random`, as this mode shows it; `number` is
    /// its [`AddressBook`] number, which only `Masked` reads, and `company`
    /// the company its manufacturer data named (`NetState::company`): for a
    /// random address, which has no IEEE block, that company takes the kind's
    /// place, marked with where it came from ([`who_with`]).
    ///
    /// **`width` is the column the table has for it, not a fixed size.** A
    /// table gives the address column what the terminal can spare
    /// (`ui::widgets::table::widen`), up to [`Self::natural_width_with`]: on a wide
    /// screen a registrant's whole name fits, on a narrow one it is cut and
    /// marked `…` (`signal::net::vendor::short_name`), and the rest of the
    /// address (`..09:be`, `#17`) is always whole and at the column's end, so
    /// the rows line up. `None` is the natural form with no padding, for the
    /// export, which is never cut.
    ///
    /// A masked address with no number shows `#-`: every address that reaches
    /// the state is numbered as it arrives, so this is a gap to see, not a
    /// number to invent.
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

    /// [`Self::natural_width_with`] with no company: the tests' form.
    #[cfg(test)]
    pub fn natural_width(self, addr: [u8; 6], random: bool, number: Option<u32>) -> usize {
        self.natural_width_with(addr, random, number, None)
    }

    /// The columns [`Self::show_with`] needs to print `addr` without cutting
    /// anything, and never less than a full address's 17, so a table sized for
    /// the widest row holds every mode.
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
/// a company**: `company` is that identifier, shown as the SIG's name for it
/// (`signal::assigned`, its legal form dropped) or the number itself
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
            let name = crate::signal::assigned::company(id)
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
/// time it is heard, and keeps it.
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

impl NetState {
    /// The company `addr`'s manufacturer data named, if it has named one.
    pub fn company(&self, addr: [u8; 6]) -> Option<u16> {
        self.advertised.get(&addr).and_then(|a| a.company)
    }

    /// `addr`, sent with TxAdd = `random`, in the section's display mode: the
    /// one call every panel and export makes, so a device reads the same way
    /// everywhere.
    ///
    /// `width` as [`AddressDisplay::show_with`] takes it: the column the table has,
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

    /// An advertised name as the address mode allows it: in full, made
    /// printable, except in `masked`, where it is only its length. A name is
    /// often a person's ("Viktor's AirPods"), and masking the address beside
    /// it would otherwise be for show.
    pub fn show_name(&self, name: &str) -> String {
        match self.address_display {
            AddressDisplay::Masked => format!("name, {} chars", name.chars().count()),
            _ => crate::signal::ble::ad::printable(name),
        }
    }

    /// What a link manager message carries that names a device, as the
    /// address mode allows it: a name as an advertised name is shown, a
    /// BD_ADDR as any public address is (its session number given as it
    /// reached the state), and a body no one can read as raw bytes are.
    pub fn show_lmp_identity(&self, i: &crate::signal::bt::lmp::Identifying) -> String {
        use crate::signal::bt::lmp::Identifying;
        match (self.address_display, i) {
            (AddressDisplay::Masked, Identifying::Name(b)) => {
                self.show_name(&String::from_utf8_lossy(b))
            }
            (AddressDisplay::Masked, Identifying::Unknown(b)) => self.show_bytes(b),
            (_, Identifying::Address(a)) => self.show_address(*a, false, None),
            _ => i.full(),
        }
    }

    /// Raw advertising bytes as the address mode allows them: spaced hex, or
    /// in `masked` only how many there are. Manufacturer and service data
    /// can carry an identifier of their own; the company or the service they
    /// belong to is shown beside them either way, as the "who", not the
    /// "which".
    pub fn show_bytes(&self, data: &[u8]) -> String {
        match self.address_display {
            AddressDisplay::Masked => format!("{} bytes", data.len()),
            _ => crate::signal::ble::ad::hex(data),
        }
    }

    /// A classic LAP as the address mode allows it: an inquiry code by its
    /// name in every mode (it is no one's address), any other in hex, or in
    /// `masked` as `#n`, its place in the roster (`bt_piconets` is kept in
    /// the order first heard, the order the hop panel colours by), so one
    /// piconet is one number in every panel and the export, and nothing in a
    /// screenshot turns back into 24 bits of an address.
    pub fn show_lap(&self, lap: u32) -> String {
        if let Some(i) = crate::signal::bt::piconet::Inquiry::of(lap) {
            return i.short().to_string();
        }
        match self.address_display {
            AddressDisplay::Masked => match self.bt_piconets.iter().position(|p| p.lap == lap) {
                Some(k) => format!("#{}", k + 1),
                None => "#?".to_string(),
            },
            _ => format!("{lap:#08x}"),
        }
    }

    /// A connection's access address as the address mode allows it: in hex,
    /// or in `masked` as `#n`, the connection's place in the order heard, so
    /// one connection is one number in every panel. It names a connection,
    /// not a device, but it is unique to that pair while they are linked.
    pub fn show_access_address(&self, aa: u32) -> String {
        match self.address_display {
            AddressDisplay::Masked => {
                let heard = self.ble_connections.len();
                match self
                    .ble_connections
                    .iter()
                    .position(|f| f.connection.access_address() == aa)
                {
                    Some(k) => format!("#{}", heard - k),
                    None => "#?".to_string(),
                }
            }
            _ => format!("{aa:#010x}"),
        }
    }

    /// A UAP value as the address mode allows it: the next 8 bits of the
    /// master's address after its LAP, so masked with it.
    pub fn show_uap(&self, uap: u8) -> String {
        match self.address_display {
            AddressDisplay::Masked => "found".to_string(),
            _ => format!("{uap:#04x}"),
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Masked hides every part of an address, not the address alone: the
    /// advertised name, the raw bytes, a LAP and a UAP. The other modes show
    /// them, and an inquiry code is named in every mode.
    #[test]
    fn masked_hides_names_bytes_laps_and_uaps() {
        let t0 = std::time::Instant::now();
        let mut net = NetState::default();
        crate::signal::bt::piconet::observe(&mut net.bt_piconets, 0x5a_3c71, 3, t0);
        crate::signal::bt::piconet::observe(&mut net.bt_piconets, 0x12_3456, 9, t0);
        for mode in [AddressDisplay::Full, AddressDisplay::Oui] {
            net.address_display = mode;
            assert_eq!(net.show_name("Viktor's AirPods"), "Viktor's AirPods");
            assert_eq!(net.show_bytes(&[0x4c, 0x00]), "4c 00");
            assert_eq!(net.show_lap(0x12_3456), "0x123456");
            assert_eq!(net.show_uap(0x4c), "0x4c");
        }
        net.address_display = AddressDisplay::Masked;
        assert_eq!(net.show_name("Viktor's AirPods"), "name, 16 chars");
        assert_eq!(net.show_bytes(&[0x4c, 0x00]), "2 bytes");
        assert_eq!(net.show_lap(0x12_3456), "#2");
        assert_eq!(net.show_lap(0x5a_3c71), "#1");
        assert_eq!(net.show_uap(0x4c), "found");
        assert_eq!(net.show_lap(0x9E_8B33), "GIAC");
    }

    /// Each mode shows what it promises, uncut for an
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
