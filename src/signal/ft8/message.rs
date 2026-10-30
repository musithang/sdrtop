// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The 77-bit payload to a message: the QEX paper's Table 1, one layout per
//! type, the fields left to right as listed, then `n3` (type 0 only), then
//! `i3`, in the last three bits.
//!
//! A payload is held as the low 77 bits of a `u128`, the first transmitted bit
//! at bit 76, so a field is read from the top down in the order Table 1 lists
//! it.
//!
//! **Who is calling whom is a convention, not a field.** In every two-call
//! message the first call is the station addressed and the second the one
//! transmitting ("K1ABC W9XYZ EN37" is W9XYZ, in EN37, calling K1ABC); after
//! CQ, DE or QRZ the second call is the one transmitting. A locator belongs to
//! the transmitting call. That reading is what [`Message::sender`],
//! [`Message::addressees`] and [`Message::locator`] give; the bits themselves
//! only say first and second.

use super::fields::{
    c28_decode, c58_decode, f71_decode, g15_decode, g25_decode, Malformed, C28, G15,
};
use super::hash::{HashBits, HashBook};

/// ARRL/RAC sections, `S7` values 0 to 83 (`arrl_rac_sections.txt`).
const SECTIONS: [&str; 84] = [
    "AB", "AK", "AL", "AR", "AZ", "BC", "CO", "CT", "DE", "EB", "EMA", "ENY", "EPA", "EWA", "GA",
    "GTA", "IA", "ID", "IL", "IN", "KS", "KY", "LA", "LAX", "MAR", "MB", "MDC", "ME", "MI", "MN",
    "MO", "MS", "MT", "NC", "ND", "NE", "NFL", "NH", "NL", "NLI", "NM", "NNJ", "NNY", "NT", "NTX",
    "NV", "OH", "OK", "ONE", "ONN", "ONS", "OR", "ORG", "PAC", "PR", "QC", "RI", "SB", "SC", "SCV",
    "SD", "SDG", "SF", "SFL", "SJV", "SK", "SNJ", "STX", "SV", "TN", "UT", "VA", "VI", "VT", "WCF",
    "WI", "WMA", "WNY", "WPA", "WTX", "WV", "WWA", "WY", "DX",
];

/// US states and Canadian provinces, `s13` values 8001 to 8065
/// (`states_provinces.txt`).
const STATES: [&str; 65] = [
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "FL", "GA", "HI", "ID", "IL", "IN", "IA", "KS",
    "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM", "NY",
    "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA", "WV",
    "WI", "WY", "NB", "NS", "QC", "ON", "MB", "SK", "AB", "BC", "NWT", "NF", "LB", "NU", "YT",
    "PEI", "DC",
];

/// A suffix a standard call carries in its own bit: `r1` in type 1, `p1` in
/// type 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suffix {
    R,
    P,
}

/// One call place in a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    De,
    Qrz,
    Cq(Option<String>),
    Std {
        call: String,
        suffix: Option<Suffix>,
    },
    /// Sent as a hash; `call` is what the book knew when it was read.
    Hashed {
        bits: HashBits,
        value: u32,
        call: Option<String>,
    },
    NonStd(String),
}

impl Call {
    fn hashed(bits: HashBits, value: u32, book: &HashBook) -> Self {
        Call::Hashed {
            bits,
            value,
            call: book.lookup(bits, value).map(str::to_string),
        }
    }

    fn from_c28(c: C28, suffix: Option<Suffix>, book: &HashBook) -> Self {
        match c {
            C28::De => Call::De,
            C28::Qrz => Call::Qrz,
            C28::Cq(m) => Call::Cq(m),
            C28::Hash22(h) => Call::hashed(HashBits::H22, h, book),
            C28::Call(call) => Call::Std { call, suffix },
        }
    }

    /// As shown: `CQ DX`, `K1ABC/R`, `<PJ4/K1ABC>`, `<...>`.
    pub fn text(&self) -> String {
        match self {
            Call::De => "DE".into(),
            Call::Qrz => "QRZ".into(),
            Call::Cq(None) => "CQ".into(),
            Call::Cq(Some(m)) => format!("CQ {m}"),
            Call::Std { call, suffix } => match suffix {
                None => call.clone(),
                Some(Suffix::R) => format!("{call}/R"),
                Some(Suffix::P) => format!("{call}/P"),
            },
            Call::Hashed { call: Some(c), .. } => format!("<{c}>"),
            Call::Hashed { call: None, .. } => "<...>".into(),
            Call::NonStd(c) => c.clone(),
        }
    }

    /// The station's call, if this place names one: none for CQ, DE and QRZ,
    /// none for a hash nobody has been heard sending in full.
    pub fn station(&self) -> Option<&str> {
        match self {
            Call::Std { call, .. } | Call::NonStd(call) => Some(call),
            Call::Hashed { call, .. } => call.as_deref(),
            _ => None,
        }
    }

    /// The call as heard in full, to teach the hash book.
    fn heard_in_full(&self) -> Option<&str> {
        match self {
            Call::Std { call, .. } | Call::NonStd(call) => Some(call),
            _ => None,
        }
    }
}

/// The exchange of an RTTY Roundup message (`s13`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exchange {
    Serial(u16),
    State(&'static str),
}

/// A message, by the paper's Table 1 types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// 0.0: up to 13 characters.
    FreeText(String),
    /// 0.1: `K1ABC RR73; W9XYZ <KH1/KH7Z> -08`, the DXpedition (hashed)
    /// finishing with one station and sending another its report.
    DxPedition {
        finished: Call,
        to: Call,
        from: Call,
        report: i8,
    },
    /// 0.3 and 0.4: `K1ABC W9XYZ R 6A WI`.
    FieldDay {
        to: Call,
        from: Call,
        ack: bool,
        transmitters: u8,
        class: char,
        section: &'static str,
    },
    /// 0.5: up to 18 hexadecimal digits.
    Telemetry(String),
    /// 1 (with `/R`) and 2 (with `/P`): `K1ABC W9XYZ R EN37`, `CQ W9XYZ EN37`.
    Standard {
        to: Call,
        from: Call,
        ack: bool,
        word: G15,
    },
    /// 3: `TU; K1ABC W9XYZ R 579 WI`.
    RttyRoundup {
        tu: bool,
        to: Call,
        from: Call,
        ack: bool,
        rst: u16,
        exchange: Exchange,
    },
    /// 4: `<W9XYZ> PJ4/K1ABC RRR` or `CQ PJ4/K1ABC`; one call in full, the
    /// other hashed, in the order sent.
    NonStandard { to: Call, from: Call, word: G15 },
    /// 5: `<G4ABC> <PA9XYZ> R 570007 JO22DB`.
    EuVhf {
        to: Call,
        from: Call,
        ack: bool,
        report: u8,
        serial: u16,
        locator: String,
    },
}

/// Reads fields from the top of a 77-bit payload down.
struct Reader {
    bits: u128,
    left: u32,
}

impl Reader {
    fn new(payload: u128) -> Self {
        Self {
            bits: payload & ((1u128 << 77) - 1),
            left: 77,
        }
    }

    fn take(&mut self, n: u32) -> u128 {
        self.left -= n;
        (self.bits >> self.left) & ((1u128 << n) - 1)
    }

    fn u32(&mut self, n: u32) -> u32 {
        self.take(n) as u32
    }

    fn flag(&mut self) -> bool {
        self.take(1) == 1
    }
}

/// The payload's type: `i3`, and `n3` when `i3` is 0.
pub fn type_of(payload: u128) -> (u8, u8) {
    let i3 = (payload & 7) as u8;
    let n3 = ((payload >> 3) & 7) as u8;
    (i3, if i3 == 0 { n3 } else { 0 })
}

/// A payload to its message. A type the paper does not define, or a field in
/// an unassigned range, is refused.
pub fn unpack(payload: u128, book: &HashBook) -> Result<Message, Malformed> {
    let mut r = Reader::new(payload);
    match type_of(payload) {
        (0, 0) => Ok(Message::FreeText(f71_decode(r.take(71))?)),
        (0, 1) => {
            let finished = Call::from_c28(c28_decode(r.u32(28))?, None, book);
            let to = Call::from_c28(c28_decode(r.u32(28))?, None, book);
            let from = Call::hashed(HashBits::H10, r.u32(10), book);
            // `r5`: -30 to +32 dB, even only.
            let report = (r.u32(5) as i8) * 2 - 30;
            Ok(Message::DxPedition {
                finished,
                to,
                from,
                report,
            })
        }
        (0, n3 @ (3 | 4)) => {
            let to = Call::from_c28(c28_decode(r.u32(28))?, None, book);
            let from = Call::from_c28(c28_decode(r.u32(28))?, None, book);
            let ack = r.flag();
            // `n4`: transmitters minus 1 in 0.3, minus 17 in 0.4.
            let transmitters = r.u32(4) as u8 + if n3 == 3 { 1 } else { 17 };
            let class = match r.u32(3) {
                k @ 0..=5 => (b'A' + k as u8) as char,
                _ => return Err(Malformed("k3")),
            };
            let section = *SECTIONS.get(r.u32(7) as usize).ok_or(Malformed("S7"))?;
            Ok(Message::FieldDay {
                to,
                from,
                ack,
                transmitters,
                class,
                section,
            })
        }
        (0, 5) => Ok(Message::Telemetry(format!("{:X}", r.take(71)))),
        (i3 @ (1 | 2), _) => {
            let suffix = if i3 == 1 { Suffix::R } else { Suffix::P };
            let c1 = c28_decode(r.u32(28))?;
            let s1 = r.flag().then_some(suffix);
            let c2 = c28_decode(r.u32(28))?;
            let s2 = r.flag().then_some(suffix);
            let ack = r.flag();
            let word = g15_decode(r.u32(15))?;
            Ok(Message::Standard {
                to: Call::from_c28(c1, s1, book),
                from: Call::from_c28(c2, s2, book),
                ack,
                word,
            })
        }
        (3, _) => {
            let tu = r.flag();
            let to = Call::from_c28(c28_decode(r.u32(28))?, None, book);
            let from = Call::from_c28(c28_decode(r.u32(28))?, None, book);
            let ack = r.flag();
            // `r3`: 529, 539, ... 599.
            let rst = 529 + 10 * r.u32(3) as u16;
            let exchange = match r.u32(13) {
                s @ 0..=7999 => Exchange::Serial(s as u16),
                s @ 8001..=8065 => Exchange::State(STATES[(s - 8001) as usize]),
                _ => return Err(Malformed("s13")),
            };
            Ok(Message::RttyRoundup {
                tu,
                to,
                from,
                ack,
                rst,
                exchange,
            })
        }
        (4, _) => {
            let h12 = r.u32(12);
            let full = Call::NonStd(c58_decode(r.take(58) as u64)?);
            let hashed_second = r.flag();
            let word = match r.u32(2) {
                0 => G15::Blank,
                1 => G15::Rrr,
                2 => G15::Rr73,
                _ => G15::R73,
            };
            let cq = r.flag();
            let hashed = Call::hashed(HashBits::H12, h12, book);
            let (to, from) = if cq {
                (Call::Cq(None), full)
            } else if hashed_second {
                (full, hashed)
            } else {
                (hashed, full)
            };
            Ok(Message::NonStandard { to, from, word })
        }
        (5, _) => {
            let to = Call::hashed(HashBits::H12, r.u32(12), book);
            let from = Call::hashed(HashBits::H22, r.u32(22), book);
            let ack = r.flag();
            // `r3`: 2 to 9, shown as 52 to 59.
            let report = r.u32(3) as u8 + 2;
            let serial = r.u32(11) as u16;
            let locator = g25_decode(r.u32(25))?;
            Ok(Message::EuVhf {
                to,
                from,
                ack,
                report,
                serial,
                locator,
            })
        }
        _ => Err(Malformed("message type")),
    }
}

fn word_text(word: &G15) -> Option<String> {
    match word {
        G15::Grid(g) => Some(g.clone()),
        G15::Blank => None,
        G15::Rrr => Some("RRR".into()),
        G15::Rr73 => Some("RR73".into()),
        G15::R73 => Some("73".into()),
        G15::Report(r) => Some(format!("{r:+03}")),
    }
}

impl Message {
    /// The message as it reads on the air.
    pub fn text(&self) -> String {
        let r = |ack: bool| if ack { " R" } else { "" };
        match self {
            Message::FreeText(t) | Message::Telemetry(t) => t.clone(),
            Message::DxPedition {
                finished,
                to,
                from,
                report,
            } => format!(
                "{} RR73; {} {} {report:+03}",
                finished.text(),
                to.text(),
                from.text()
            ),
            Message::FieldDay {
                to,
                from,
                ack,
                transmitters,
                class,
                section,
            } => format!(
                "{} {}{} {transmitters}{class} {section}",
                to.text(),
                from.text(),
                r(*ack)
            ),
            Message::Standard {
                to,
                from,
                ack,
                word,
            } => {
                let mut s = format!("{} {}", to.text(), from.text());
                match (word, word_text(word)) {
                    // An acknowledged report reads as one word: R-08.
                    (G15::Report(_), Some(w)) if *ack => s += &format!(" R{w}"),
                    (_, Some(w)) => s += &format!("{} {w}", r(*ack)),
                    (_, None) => s += r(*ack),
                }
                s
            }
            Message::RttyRoundup {
                tu,
                to,
                from,
                ack,
                rst,
                exchange,
            } => {
                let ex = match exchange {
                    Exchange::Serial(n) => format!("{n:04}"),
                    Exchange::State(s) => (*s).to_string(),
                };
                format!(
                    "{}{} {}{} {rst} {ex}",
                    if *tu { "TU; " } else { "" },
                    to.text(),
                    from.text(),
                    r(*ack)
                )
            }
            Message::NonStandard { to, from, word } => {
                let mut s = format!("{} {}", to.text(), from.text());
                if let Some(w) = word_text(word) {
                    s += &format!(" {w}");
                }
                s
            }
            Message::EuVhf {
                to,
                from,
                ack,
                report,
                serial,
                locator,
            } => format!(
                "{} {}{} 5{report}{serial:04} {locator}",
                to.text(),
                from.text(),
                r(*ack)
            ),
        }
    }

    /// The transmitting station, if the message names it.
    pub fn sender(&self) -> Option<&str> {
        match self {
            Message::DxPedition { from, .. }
            | Message::FieldDay { from, .. }
            | Message::Standard { from, .. }
            | Message::RttyRoundup { from, .. }
            | Message::NonStandard { from, .. }
            | Message::EuVhf { from, .. } => from.station(),
            Message::FreeText(_) | Message::Telemetry(_) => None,
        }
    }

    /// The stations addressed: none for CQ, two in a DXpedition message.
    pub fn addressees(&self) -> Vec<&str> {
        match self {
            Message::DxPedition { finished, to, .. } => [finished, to]
                .into_iter()
                .filter_map(Call::station)
                .collect(),
            Message::FieldDay { to, .. }
            | Message::Standard { to, .. }
            | Message::RttyRoundup { to, .. }
            | Message::NonStandard { to, .. }
            | Message::EuVhf { to, .. } => to.station().into_iter().collect(),
            Message::FreeText(_) | Message::Telemetry(_) => Vec::new(),
        }
    }

    /// The transmitting station's own locator, where the message carries one.
    pub fn locator(&self) -> Option<&str> {
        match self {
            Message::Standard {
                word: G15::Grid(g), ..
            } => Some(g),
            Message::EuVhf { locator, .. } => Some(locator),
            _ => None,
        }
    }

    /// Every call the message sends in full, for the hash book.
    pub fn calls_in_full(&self) -> Vec<&str> {
        let places: Vec<&Call> = match self {
            Message::DxPedition {
                finished, to, from, ..
            } => vec![finished, to, from],
            Message::FieldDay { to, from, .. }
            | Message::Standard { to, from, .. }
            | Message::RttyRoundup { to, from, .. }
            | Message::NonStandard { to, from, .. }
            | Message::EuVhf { to, from, .. } => vec![to, from],
            Message::FreeText(_) | Message::Telemetry(_) => Vec::new(),
        };
        places.into_iter().filter_map(Call::heard_in_full).collect()
    }
}

/// Messages back to payloads, so every unpacking test round-trips.
#[cfg(test)]
pub mod pack {
    use super::*;
    use crate::signal::ft8::fields::{
        c58_encode, f71_encode, g15_encode, g25_encode, std_call_c28, NTOKENS,
    };

    /// Writes fields from the top of a 77-bit payload down.
    #[derive(Default)]
    struct Writer {
        bits: u128,
        used: u32,
    }

    impl Writer {
        fn put(&mut self, n: u32, v: u128) -> &mut Self {
            assert!(v < (1u128 << n), "{v} does not fit {n} bits");
            self.bits = (self.bits << n) | v;
            self.used += n;
            self
        }

        fn done(&self) -> u128 {
            assert_eq!(self.used, 77);
            self.bits
        }
    }

    fn c28(call: &Call) -> u128 {
        u128::from(match call {
            Call::De => 0,
            Call::Qrz => 1,
            Call::Cq(None) => 2,
            Call::Cq(Some(m)) if m.bytes().all(|b| b.is_ascii_digit()) => {
                3 + m.parse::<u32>().unwrap()
            }
            Call::Cq(Some(m)) => 1003 + m.bytes().fold(0, |n, b| n * 27 + u32::from(b - b'A' + 1)),
            Call::Hashed {
                bits: HashBits::H22,
                value,
                ..
            } => NTOKENS + value,
            Call::Std { call, .. } => std_call_c28(call).expect("a standard call"),
            other => panic!("{other:?} has no c28"),
        })
    }

    fn suffix_bit(call: &Call) -> u128 {
        matches!(
            call,
            Call::Std {
                suffix: Some(_),
                ..
            }
        ) as u128
    }

    fn hash(call: &Call) -> u128 {
        match call {
            Call::Hashed { value, .. } => u128::from(*value),
            other => panic!("{other:?} is not hashed"),
        }
    }

    pub fn pack(m: &Message) -> u128 {
        let mut w = Writer::default();
        match m {
            Message::FreeText(t) => w.put(71, f71_encode(t)).put(3, 0).put(3, 0),
            Message::DxPedition {
                finished,
                to,
                from,
                report,
            } => w
                .put(28, c28(finished))
                .put(28, c28(to))
                .put(10, hash(from))
                .put(5, ((report + 30) / 2) as u128)
                .put(3, 1)
                .put(3, 0),
            Message::FieldDay {
                to,
                from,
                ack,
                transmitters,
                class,
                section,
            } => {
                let (n3, base) = if *transmitters <= 16 { (3, 1) } else { (4, 17) };
                let s = SECTIONS.iter().position(|x| x == section).unwrap();
                w.put(28, c28(to))
                    .put(28, c28(from))
                    .put(1, *ack as u128)
                    .put(4, u128::from(transmitters - base))
                    .put(3, u128::from(*class as u8 - b'A'))
                    .put(7, s as u128)
                    .put(3, n3)
                    .put(3, 0)
            }
            Message::Telemetry(hex) => w
                .put(71, u128::from_str_radix(hex, 16).unwrap())
                .put(3, 5)
                .put(3, 0),
            Message::Standard {
                to,
                from,
                ack,
                word,
            } => {
                let i3 = if [to, from].iter().any(|c| {
                    matches!(
                        c,
                        Call::Std {
                            suffix: Some(Suffix::P),
                            ..
                        }
                    )
                }) {
                    2
                } else {
                    1
                };
                w.put(28, c28(to))
                    .put(1, suffix_bit(to))
                    .put(28, c28(from))
                    .put(1, suffix_bit(from))
                    .put(1, *ack as u128)
                    .put(15, u128::from(g15_encode(word)))
                    .put(3, i3)
            }
            Message::RttyRoundup {
                tu,
                to,
                from,
                ack,
                rst,
                exchange,
            } => {
                let s13 = match exchange {
                    Exchange::Serial(n) => u128::from(*n),
                    Exchange::State(s) => {
                        8001 + STATES.iter().position(|x| x == s).unwrap() as u128
                    }
                };
                w.put(1, *tu as u128)
                    .put(28, c28(to))
                    .put(28, c28(from))
                    .put(1, *ack as u128)
                    .put(3, u128::from((rst - 529) / 10))
                    .put(13, s13)
                    .put(3, 3)
            }
            Message::NonStandard { to, from, word } => {
                let r2 = match word {
                    G15::Blank => 0,
                    G15::Rrr => 1,
                    G15::Rr73 => 2,
                    G15::R73 => 3,
                    other => panic!("{other:?} is not an r2 word"),
                };
                let (h12, full, second, cq) = match (to, from) {
                    (Call::Cq(None), Call::NonStd(f)) => (0, f, 0, 1),
                    (Call::Hashed { value, .. }, Call::NonStd(f)) => (*value, f, 0, 0),
                    (Call::NonStd(f), Call::Hashed { value, .. }) => (*value, f, 1, 0),
                    other => panic!("{other:?} is not a type 4 pair"),
                };
                w.put(12, u128::from(h12))
                    .put(58, u128::from(c58_encode(full)))
                    .put(1, second)
                    .put(2, r2)
                    .put(1, cq)
                    .put(3, 4)
            }
            Message::EuVhf {
                to,
                from,
                ack,
                report,
                serial,
                locator,
            } => w
                .put(12, hash(to))
                .put(22, hash(from))
                .put(1, *ack as u128)
                .put(3, u128::from(report - 2))
                .put(11, u128::from(*serial))
                .put(25, u128::from(g25_encode(locator)))
                .put(3, 5),
        };
        w.done()
    }
}

#[cfg(test)]
mod tests {
    use super::pack::pack;
    use super::*;
    use crate::signal::ft8::fields::hashes;

    fn std(call: &str) -> Call {
        Call::Std {
            call: call.into(),
            suffix: None,
        }
    }

    fn hashed(bits: HashBits, call: &str, book: &HashBook) -> Call {
        let h = hashes(call).unwrap();
        let value = match bits {
            HashBits::H10 => h.h10,
            HashBits::H12 => h.h12,
            HashBits::H22 => h.h22,
        };
        Call::hashed(bits, value, book)
    }

    /// Pack, unpack against the same book, compare, and return the text.
    fn round_trip(m: Message, book: &HashBook) -> String {
        let p = pack(&m);
        assert_eq!(unpack(p, book).as_ref(), Ok(&m));
        m.text()
    }

    #[test]
    fn table_1_examples_read_as_printed() {
        let mut book = HashBook::new();
        book.learn("KH1/KH7Z");
        book.learn("W9XYZ");
        book.learn("G4ABC");
        book.learn("PA9XYZ");
        let cases = [
            (Message::FreeText("TNX BOB 73 GL".into()), "TNX BOB 73 GL"),
            (
                Message::DxPedition {
                    finished: std("K1ABC"),
                    to: std("W9XYZ"),
                    from: hashed(HashBits::H10, "KH1/KH7Z", &book),
                    report: -8,
                },
                "K1ABC RR73; W9XYZ <KH1/KH7Z> -08",
            ),
            (
                Message::FieldDay {
                    to: std("K1ABC"),
                    from: std("W9XYZ"),
                    ack: false,
                    transmitters: 6,
                    class: 'A',
                    section: "WI",
                },
                "K1ABC W9XYZ 6A WI",
            ),
            (
                Message::FieldDay {
                    to: std("W9XYZ"),
                    from: std("K1ABC"),
                    ack: true,
                    transmitters: 17,
                    class: 'B',
                    section: "EMA",
                },
                "W9XYZ K1ABC R 17B EMA",
            ),
            (
                Message::Telemetry("123456789ABCDEF012".into()),
                "123456789ABCDEF012",
            ),
            (
                Message::Standard {
                    to: Call::Std {
                        call: "K1ABC".into(),
                        suffix: Some(Suffix::R),
                    },
                    from: Call::Std {
                        call: "W9XYZ".into(),
                        suffix: Some(Suffix::R),
                    },
                    ack: true,
                    word: G15::Grid("EN37".into()),
                },
                "K1ABC/R W9XYZ/R R EN37",
            ),
            (
                Message::Standard {
                    to: Call::Std {
                        call: "G4ABC".into(),
                        suffix: Some(Suffix::P),
                    },
                    from: Call::Std {
                        call: "PA9XYZ".into(),
                        suffix: Some(Suffix::P),
                    },
                    ack: false,
                    word: G15::Grid("JO22".into()),
                },
                "G4ABC/P PA9XYZ/P JO22",
            ),
            (
                Message::RttyRoundup {
                    tu: false,
                    to: std("K1ABC"),
                    from: std("W9XYZ"),
                    ack: false,
                    rst: 579,
                    exchange: Exchange::State("WI"),
                },
                "K1ABC W9XYZ 579 WI",
            ),
            (
                Message::NonStandard {
                    to: hashed(HashBits::H12, "W9XYZ", &book),
                    from: Call::NonStd("PJ4/K1ABC".into()),
                    word: G15::Rrr,
                },
                "<W9XYZ> PJ4/K1ABC RRR",
            ),
            (
                Message::EuVhf {
                    to: hashed(HashBits::H12, "G4ABC", &book),
                    from: hashed(HashBits::H22, "PA9XYZ", &book),
                    ack: true,
                    report: 7,
                    serial: 7,
                    locator: "JO22DB".into(),
                },
                "<G4ABC> <PA9XYZ> R 570007 JO22DB",
            ),
        ];
        for (m, text) in cases {
            assert_eq!(round_trip(m, &book), text);
        }
    }

    #[test]
    fn the_everyday_messages() {
        let book = HashBook::new();
        let cq = |m: Option<&str>, call: &str, grid: &str| Message::Standard {
            to: Call::Cq(m.map(str::to_string)),
            from: std(call),
            ack: false,
            word: G15::Grid(grid.into()),
        };
        let two = |to: &str, from: &str, ack: bool, word: G15| Message::Standard {
            to: std(to),
            from: std(from),
            ack,
            word,
        };
        assert_eq!(
            round_trip(cq(None, "OK1HZD", "JO70"), &book),
            "CQ OK1HZD JO70"
        );
        assert_eq!(
            round_trip(cq(Some("DX"), "VK3FTX", "QF22"), &book),
            "CQ DX VK3FTX QF22"
        );
        assert_eq!(
            round_trip(cq(Some("123"), "K1ABC", "FN42"), &book),
            "CQ 123 K1ABC FN42"
        );
        assert_eq!(
            round_trip(two("IK2QPF", "K3MTR", true, G15::Report(-14)), &book),
            "IK2QPF K3MTR R-14"
        );
        assert_eq!(
            round_trip(two("LZ1KRB", "4X5TY", false, G15::Report(-7)), &book),
            "LZ1KRB 4X5TY -07"
        );
        assert_eq!(
            round_trip(two("F5RVK", "YO3GCI", true, G15::Report(1)), &book),
            "F5RVK YO3GCI R+01"
        );
        assert_eq!(
            round_trip(two("ON4FTQ", "ZS6DWR", false, G15::Rr73), &book),
            "ON4FTQ ZS6DWR RR73"
        );
        assert_eq!(
            round_trip(two("ON4FTQ", "ZS6DWR", false, G15::R73), &book),
            "ON4FTQ ZS6DWR 73"
        );
        assert_eq!(
            round_trip(two("ON4FTQ", "ZS6DWR", false, G15::Blank), &book),
            "ON4FTQ ZS6DWR"
        );
        assert_eq!(
            round_trip(
                Message::RttyRoundup {
                    tu: true,
                    to: std("K1ABC"),
                    from: std("W9XYZ"),
                    ack: true,
                    rst: 599,
                    exchange: Exchange::Serial(13),
                },
                &book
            ),
            "TU; K1ABC W9XYZ R 599 0013"
        );
        assert_eq!(
            round_trip(
                Message::NonStandard {
                    to: Call::Cq(None),
                    from: Call::NonStd("PJ4/K1ABC".into()),
                    word: G15::Blank,
                },
                &book
            ),
            "CQ PJ4/K1ABC"
        );
    }

    #[test]
    fn who_sends_whom_it_addresses_and_from_where() {
        let book = HashBook::new();
        let m = Message::Standard {
            to: std("HA5ZZT"),
            from: std("JA1VQD"),
            ack: false,
            word: G15::Grid("PM95".into()),
        };
        let m = unpack(pack(&m), &book).unwrap();
        assert_eq!(m.sender(), Some("JA1VQD"));
        assert_eq!(m.addressees(), vec!["HA5ZZT"]);
        assert_eq!(m.locator(), Some("PM95"));
        assert_eq!(m.calls_in_full(), vec!["HA5ZZT", "JA1VQD"]);

        let cq = Message::Standard {
            to: Call::Cq(None),
            from: std("EA8ZQ"),
            ack: false,
            word: G15::Grid("IL18".into()),
        };
        assert_eq!(cq.addressees(), Vec::<&str>::new());
        assert_eq!(cq.sender(), Some("EA8ZQ"));

        // A report is no locator.
        let rpt = Message::Standard {
            to: std("LZ1KRB"),
            from: std("4X5TY"),
            ack: false,
            word: G15::Report(-7),
        };
        assert_eq!(rpt.locator(), None);
    }

    #[test]
    fn a_hash_nobody_was_heard_sending_reads_as_dots_until_heard() {
        let mut book = HashBook::new();
        let m = Message::NonStandard {
            to: hashed(HashBits::H12, "W9XYZ", &book),
            from: Call::NonStd("PJ4/K1ABC".into()),
            word: G15::Rr73,
        };
        let p = pack(&m);
        let before = unpack(p, &book).unwrap();
        assert_eq!(before.text(), "<...> PJ4/K1ABC RR73");
        // The unnamed hash addresses nobody we can name.
        assert_eq!(before.addressees(), Vec::<&str>::new());
        book.learn("W9XYZ");
        assert_eq!(unpack(p, &book).unwrap().text(), "<W9XYZ> PJ4/K1ABC RR73");
    }

    #[test]
    fn a_full_call_teaches_the_book_and_a_later_hash_names_it() {
        let mut book = HashBook::new();
        let full = Message::NonStandard {
            to: Call::Cq(None),
            from: Call::NonStd("YW18FIFA".into()),
            word: G15::Blank,
        };
        for call in unpack(pack(&full), &book).unwrap().calls_in_full() {
            book.learn(call);
        }
        let later = Message::Standard {
            to: std("K1ABC"),
            from: hashed(HashBits::H22, "YW18FIFA", &HashBook::new()),
            ack: false,
            word: G15::Report(-12),
        };
        let read = unpack(pack(&later), &book).unwrap();
        assert_eq!(read.text(), "K1ABC <YW18FIFA> -12");
        assert_eq!(read.sender(), Some("YW18FIFA"));
    }

    #[test]
    fn types_and_ranges_the_paper_leaves_unassigned_are_refused() {
        let book = HashBook::new();
        // i3 = 6 and 7, and the type 0 subtypes 2, 6 and 7.
        for p in [6u128, 7, 2 << 3, 6 << 3, 7 << 3] {
            assert_eq!(unpack(p, &book), Err(Malformed("message type")), "{p}");
        }
        // Field Day class G (k3 = 6).
        let mut fd = pack(&Message::FieldDay {
            to: std("K1ABC"),
            from: std("W9XYZ"),
            ack: false,
            transmitters: 1,
            class: 'A',
            section: "AB",
        });
        fd |= 6 << (6 + 7);
        assert_eq!(unpack(fd, &book), Err(Malformed("k3")));
        // RTTY exchange 8000 and 8066.
        for s13 in [8000u128, 8066] {
            let base = pack(&Message::RttyRoundup {
                tu: false,
                to: std("K1ABC"),
                from: std("W9XYZ"),
                ack: false,
                rst: 599,
                exchange: Exchange::Serial(0),
            });
            assert_eq!(unpack(base | (s13 << 3), &book), Err(Malformed("s13")));
        }
    }

    #[test]
    fn type_of_reads_i3_and_n3() {
        assert_eq!(type_of(0b101_000), (0, 5));
        assert_eq!(type_of(0b101_001), (1, 0));
    }
}
