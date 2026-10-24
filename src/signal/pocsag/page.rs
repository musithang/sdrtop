// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Batches to pages, as ITU-R M.584-2 Annex 1 §1.2, §1.3 and §2 lay them out.
//!
//! A batch is the sync codeword and eight frames of two codewords. A pager's
//! address codeword sits in the frame its identity's three low bits name,
//! so the frame is half of the address: 18 bits from the codeword, 3 from
//! where it was. Its message follows directly, 20 bits a codeword, across
//! frames and batches, until the next address or idle codeword, or two
//! codewords in a row that cannot be read.
//!
//! **What could not be read stays unknown.** A codeword the BCH check refuses
//! still occupies its 20 bits in the message; they are kept as unknown, and
//! every character that touches one is unknown too, rather than read from
//! bits the receiver never had. A page's text can have holes; it never has
//! letters nobody sent.
//!
//! **The content's kind is a decision, and the page says how it was made.**
//! §2.1 gives numeric messages function bits 00 and §2.2 gives alphanumeric
//! ones 11. Those are taken at their word. The other two functions name no
//! format, so there the content itself decides: 7-bit text when it reads as
//! text, digits when it does not, and the page records which way it went.

use super::bch::{self, Checked, IDLE};

/// The two function bits, named as pagers name them: A is 00, D is 11.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Function {
    A,
    B,
    C,
    D,
}

impl Function {
    fn from_bits(bits: u32) -> Self {
        match bits & 0b11 {
            0 => Function::A,
            1 => Function::B,
            2 => Function::C,
            _ => Function::D,
        }
    }

    pub fn letter(self) -> char {
        match self {
            Function::A => 'A',
            Function::B => 'B',
            Function::C => 'C',
            Function::D => 'D',
        }
    }
}

/// Table 3's sixteen symbols. `Spare` is the one the Recommendation leaves
/// unassigned; 1110 and 1111 are drawn there as closing and opening brackets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Numeric {
    Digit(u8),
    Spare,
    Urgent,
    Space,
    Hyphen,
    Close,
    Open,
}

impl Numeric {
    fn from_nibble(n: u8) -> Self {
        match n {
            0..=9 => Numeric::Digit(n),
            10 => Numeric::Spare,
            11 => Numeric::Urgent,
            12 => Numeric::Space,
            13 => Numeric::Hyphen,
            14 => Numeric::Close,
            _ => Numeric::Open,
        }
    }

    /// As a display shows it. The spare code has no symbol of its own in the
    /// table; it is shown as `*`, and stays `Spare` in the page.
    pub fn glyph(self) -> char {
        match self {
            Numeric::Digit(d) => (b'0' + d) as char,
            Numeric::Spare => '*',
            Numeric::Urgent => 'U',
            Numeric::Space => ' ',
            Numeric::Hyphen => '-',
            Numeric::Close => ']',
            Numeric::Open => '[',
        }
    }
}

/// What a page carried. A character is `None` where a bit of it was lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// An address alone: the pager beeps, and that is the message.
    Tone,
    Numeric(Vec<Option<Numeric>>),
    /// International Alphabet No. 5 codes, 7 bits each, as received: what a
    /// national variant shows for `[` or `{` is the display's choice, not
    /// the decoder's.
    Alpha(Vec<Option<u8>>),
}

/// How the content's kind was decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decided {
    /// Function A or D, which §2 assigns to numeric and alphanumeric.
    ByFunction,
    /// Function B or C, which name no format: the content decided, because
    /// exactly one reading of it fits.
    ByContent,
    /// Function B or C, and both readings fit, or neither: the page is shown
    /// as the likelier and says it could be the other.
    Ambiguous,
    /// No message codewords at all.
    NoMessage,
}

/// Why a page ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Address,
    Idle,
    /// Two codewords in a row could not be read (§2.5.1).
    Undecipherable,
    /// The batches stopped: sync lost, or the transmission ended without the
    /// idle codeword it should end on.
    Cut,
}

/// One codeword of a page as it came off the air.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Received {
    pub raw: u32,
    /// The repaired word and the bits flipped, or `None` if it was refused.
    pub checked: Option<Checked>,
}

/// A page: who it was for, what it said, and what it took to read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// The 21-bit identity: 18 bits from the address codeword, 3 from its frame.
    pub ric: u32,
    pub function: Function,
    pub content: Content,
    pub decided: Decided,
    pub end: End,
    /// The address codeword, then every message codeword, in order.
    pub codewords: Vec<Received>,
}

impl Page {
    /// Bits repaired across the whole page.
    pub fn repairs(&self) -> u32 {
        self.codewords
            .iter()
            .filter_map(|c| c.checked)
            .map(|c| c.repairs())
            .sum()
    }

    /// Codewords the check refused.
    pub fn refused(&self) -> usize {
        self.codewords
            .iter()
            .filter(|c| c.checked.is_none())
            .count()
    }

    /// The content as one line of text for a display, with `?` where a
    /// character was lost and `·` where a 7-bit code does not print.
    pub fn text(&self) -> String {
        match &self.content {
            Content::Tone => String::new(),
            Content::Numeric(chars) => chars
                .iter()
                .map(|c| c.map_or('?', Numeric::glyph))
                .collect(),
            Content::Alpha(codes) => codes
                .iter()
                .map(|c| match c {
                    None => '?',
                    Some(b @ 0x20..=0x7E) => *b as char,
                    Some(_) => '·',
                })
                .collect(),
        }
    }
}

/// Builds pages from batches. Holds the page being received across batches.
#[derive(Debug, Default)]
pub struct Assembler {
    open: Option<Open>,
}

#[derive(Debug)]
struct Open {
    ric: u32,
    function: Function,
    /// The message bits in transmission order; `None` for a lost one.
    bits: Vec<Option<bool>>,
    codewords: Vec<Received>,
    refused_in_a_row: u8,
    /// Whether anything followed the address: a page whose message was lost
    /// whole is not a tone-only page.
    followed: bool,
}

impl Assembler {
    /// The sixteen codewords after one sync codeword. Returns the pages this
    /// batch completed.
    pub fn push_batch(&mut self, batch: &[u32; 16]) -> Vec<Page> {
        let mut done = Vec::new();
        for (i, &raw) in batch.iter().enumerate() {
            let frame = (i / 2) as u32;
            let checked = bch::check(raw);
            let received = Received { raw, checked };
            match checked.map(|c| c.word) {
                Some(IDLE) => self.close(End::Idle, &mut done),
                Some(word) if word >> 31 == 0 => {
                    self.close(End::Address, &mut done);
                    self.open = Some(Open {
                        ric: (((word >> 13) & 0x3FFFF) << 3) | frame,
                        function: Function::from_bits(word >> 11),
                        bits: Vec::new(),
                        codewords: vec![received],
                        refused_in_a_row: 0,
                        followed: false,
                    });
                }
                Some(word) => {
                    if let Some(open) = &mut self.open {
                        open.refused_in_a_row = 0;
                        open.followed = true;
                        open.codewords.push(received);
                        open.bits
                            .extend((0..20).rev().map(|b| Some((word >> (11 + b)) & 1 == 1)));
                    }
                }
                None => {
                    let Some(open) = &mut self.open else { continue };
                    open.refused_in_a_row += 1;
                    open.followed = true;
                    if open.refused_in_a_row >= 2 {
                        // §2.5.1: two undecipherable codewords end the
                        // message. The second is not part of it; the first
                        // is dropped with it, since nothing followed to
                        // show it was inside the message rather than after.
                        open.codewords.pop();
                        open.bits.truncate(open.bits.len().saturating_sub(20));
                        self.close(End::Undecipherable, &mut done);
                    } else {
                        // Whether this was a message or an address codeword
                        // is itself unknown; it is held as 20 lost message
                        // bits, and the next codeword decides.
                        open.codewords.push(received);
                        open.bits.extend(std::iter::repeat_n(None, 20));
                    }
                }
            }
        }
        done
    }

    /// The batches stopped. The page in progress ends here.
    pub fn finish(&mut self) -> Option<Page> {
        let mut done = Vec::new();
        self.close(End::Cut, &mut done);
        done.pop()
    }

    fn close(&mut self, end: End, done: &mut Vec<Page>) {
        let Some(mut open) = self.open.take() else {
            return;
        };
        // A refused codeword the page ended on was not the message's.
        while open.codewords.last().is_some_and(|c| c.checked.is_none()) && open.codewords.len() > 1
        {
            open.codewords.pop();
            open.bits.truncate(open.bits.len().saturating_sub(20));
        }
        let (content, decided) = content_of(open.function, &open.bits, open.followed);
        done.push(Page {
            ric: open.ric,
            function: open.function,
            content,
            decided,
            end,
            codewords: open.codewords,
        });
    }
}

fn content_of(function: Function, bits: &[Option<bool>], followed: bool) -> (Content, Decided) {
    if !followed {
        return (Content::Tone, Decided::NoMessage);
    }
    match function {
        Function::A => (Content::Numeric(numeric(bits)), Decided::ByFunction),
        Function::D => (Content::Alpha(alpha(bits)), Decided::ByFunction),
        Function::B | Function::C => {
            let codes = alpha(bits);
            let digits = numeric(bits);
            match (reads_as_text(&codes), reads_as_digits(&digits)) {
                (true, false) => (Content::Alpha(codes), Decided::ByContent),
                (false, true) => (Content::Numeric(digits), Decided::ByContent),
                (true, true) => (Content::Alpha(codes), Decided::Ambiguous),
                (false, false) => (Content::Numeric(digits), Decided::Ambiguous),
            }
        }
    }
}

/// Characters of `width` bits, least significant first, as §2.1 and §2.2
/// send them. A trailing part-character is fill, not a character.
fn chars(bits: &[Option<bool>], width: usize) -> Vec<Option<u8>> {
    bits.chunks_exact(width)
        .map(|c| {
            c.iter()
                .enumerate()
                .try_fold(0u8, |acc, (i, b)| b.map(|b| acc | (u8::from(b) << i)))
        })
        .collect()
}

fn numeric(bits: &[Option<bool>]) -> Vec<Option<Numeric>> {
    let mut out: Vec<Option<Numeric>> = chars(bits, 4)
        .into_iter()
        .map(|c| c.map(Numeric::from_nibble))
        .collect();
    // §2.1: the unwanted part of the last codeword is filled with spaces.
    while out.last() == Some(&Some(Numeric::Space)) {
        out.pop();
    }
    out
}

fn alpha(bits: &[Option<bool>]) -> Vec<Option<u8>> {
    let mut out = chars(bits, 7);
    // §2.2: the unwanted part is filled with non-printing characters such as
    // End of Message, End of Text and Null.
    while out
        .last()
        .is_some_and(|c| c.is_some_and(|b| b < 0x20 || b == 0x7F))
    {
        out.pop();
    }
    out
}

/// Whether 7-bit codes read as a message: all printable (a line break
/// allowed), at least one letter, and mostly letters, digits and spaces.
/// Digits sent as 4-bit symbols and read 7 bits at a time come out as
/// printable runs too (`5551234` reads `U*H!L`); the share of symbols is what
/// tells them from words.
fn reads_as_text(codes: &[Option<u8>]) -> bool {
    let known: Vec<u8> = codes.iter().flatten().copied().collect();
    let plain = known
        .iter()
        .filter(|b| b.is_ascii_alphanumeric() || **b == b' ')
        .count();
    !known.is_empty()
        && known
            .iter()
            .all(|&b| (0x20..=0x7E).contains(&b) || b == b'\n' || b == b'\r')
        && known.iter().any(|b| b.is_ascii_alphabetic())
        && plain * 5 >= known.len() * 4
}

/// Whether 4-bit symbols read as a numeric message: none of them the code
/// Table 3 leaves spare, which no numeric pager is sent.
fn reads_as_digits(symbols: &[Option<Numeric>]) -> bool {
    symbols.iter().flatten().all(|s| *s != Numeric::Spare)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::signal::pocsag::bch::encode;

    /// A message's bits, least significant first per character.
    pub fn bits_of(codes: &[u8], width: usize) -> Vec<bool> {
        codes
            .iter()
            .flat_map(|&c| (0..width).map(move |i| (c >> i) & 1 == 1))
            .collect()
    }

    /// Batches carrying `pages` as a transmitter would: each address in its
    /// frame, idle codewords where nothing is due, the message straight
    /// after its address, across batches, an idle codeword to finish.
    /// Tests only: sdrtop does not transmit.
    pub fn transmit(pages: &[(u32, Function, Vec<bool>)]) -> Vec<[u32; 16]> {
        let mut words: Vec<u32> = Vec::new();
        for (ric, function, bits) in pages {
            let frame = (ric & 7) as usize;
            while words.len() % 16 != frame * 2 {
                words.push(IDLE);
            }
            let f = match function {
                Function::A => 0,
                Function::B => 1,
                Function::C => 2,
                Function::D => 3,
            };
            words.push(encode(((ric >> 3) << 13) | (f << 11)));
            let mut padded = bits.clone();
            while !padded.len().is_multiple_of(20) {
                padded.push(false);
            }
            for chunk in padded.chunks(20) {
                let data = chunk.iter().fold(0u32, |acc, &b| (acc << 1) | u32::from(b));
                words.push(encode(0x8000_0000 | (data << 11)));
            }
        }
        words.push(IDLE);
        while !words.len().is_multiple_of(16) {
            words.push(IDLE);
        }
        words.chunks(16).map(|c| c.try_into().unwrap()).collect()
    }

    fn receive(batches: &[[u32; 16]]) -> Vec<Page> {
        let mut a = Assembler::default();
        let mut pages: Vec<Page> = batches.iter().flat_map(|b| a.push_batch(b)).collect();
        pages.extend(a.finish());
        pages
    }

    fn numeric_bits(text: &str) -> Vec<bool> {
        let table = "0123456789*U -][";
        let codes: Vec<u8> = text.chars().map(|c| table.find(c).unwrap() as u8).collect();
        let mut bits = bits_of(&codes, 4);
        // Fill with spaces to the codeword, as §2.1 asks.
        while !bits.len().is_multiple_of(20) {
            bits.extend(bits_of(&[12], 4));
        }
        bits
    }

    fn alpha_bits(text: &str) -> Vec<bool> {
        let mut bits = bits_of(text.as_bytes(), 7);
        bits.extend(bits_of(&[0x04], 7));
        bits
    }

    #[test]
    fn an_alpha_page_round_trips() {
        let pages = receive(&transmit(&[(
            200_001,
            Function::D,
            alpha_bits("TEST PAGE 1200 BAUD"),
        )]));
        assert_eq!(pages.len(), 1);
        let p = &pages[0];
        assert_eq!((p.ric, p.function), (200_001, Function::D));
        assert_eq!(p.text(), "TEST PAGE 1200 BAUD");
        assert_eq!((p.decided, p.end), (Decided::ByFunction, End::Idle));
        assert_eq!((p.repairs(), p.refused()), (0, 0));
    }

    /// Table 3, every symbol, brackets as the Recommendation draws them.
    #[test]
    fn a_numeric_page_round_trips_with_the_whole_table() {
        let pages = receive(&transmit(&[(
            175_432,
            Function::A,
            numeric_bits("0123456789*U -]["),
        )]));
        assert_eq!(pages[0].text(), "0123456789*U -][");
        assert!(matches!(&pages[0].content, Content::Numeric(c) if c[10] == Some(Numeric::Spare)));
    }

    /// The address's three low bits are its frame: every one of the eight.
    #[test]
    fn the_frame_is_the_addresss_low_bits() {
        for low in 0..8u32 {
            let ric = (12_345 << 3) | low;
            let pages = receive(&transmit(&[(ric, Function::C, Vec::new())]));
            assert_eq!(pages[0].ric, ric);
            assert_eq!(pages[0].content, Content::Tone);
            assert_eq!(pages[0].decided, Decided::NoMessage);
        }
    }

    /// A long message runs across batches, and two pages in a row end each
    /// other.
    #[test]
    fn messages_cross_batches_and_end_at_the_next_address() {
        let long = "NET MEETING 19:00 ON 70CM, ALL WELCOME. BRING A RADIO AND A FRIEND.";
        let batches = transmit(&[
            (200_001, Function::D, alpha_bits(long)),
            (2_504, Function::A, numeric_bits("2045-17")),
        ]);
        assert!(batches.len() >= 2);
        let pages = receive(&batches);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].text(), long);
        assert_eq!(pages[1].text(), "2045-17");
    }

    /// Repairs are counted on the page; a refused codeword's characters are
    /// unknown, not invented, and the rest of the message survives.
    #[test]
    fn repairs_are_counted_and_lost_bits_stay_lost() {
        let mut batches = transmit(&[(200_001, Function::D, alpha_bits("ABCDEFGHIJKLMNOPQRST"))]);
        let at = (200_001 & 7) as usize * 2;
        batches[0][at + 1] ^= 0b1; // one bit in the first message codeword
        batches[0][at + 2] ^= 0b111 << 20; // three in the second: refused
        let p = &receive(&batches)[0];
        assert_eq!(p.repairs(), 1);
        assert_eq!(p.refused(), 1);
        let text = p.text();
        assert!(text.starts_with("AB"), "{text}");
        assert!(text.contains('?'), "{text}");
        assert!(text.ends_with("PQRST"), "{text}");
        assert_eq!(text.chars().count(), 20, "{text}");
    }

    /// Two unreadable codewords in a row end the message (§2.5.1).
    #[test]
    fn two_refused_codewords_end_the_page() {
        let mut batches = transmit(&[(8, Function::D, alpha_bits("HELLO THERE FRIEND"))]);
        batches[0][2] ^= 0b111 << 20;
        batches[0][3] ^= 0b111 << 20;
        let pages = receive(&batches);
        assert_eq!(pages[0].end, End::Undecipherable);
        assert_eq!(
            pages[0].text(),
            "HE",
            "only the first codeword's whole characters"
        );
    }

    /// A page whose whole message was lost is not a tone-only page.
    #[test]
    fn a_message_lost_whole_is_not_a_tone() {
        let mut batches = transmit(&[(8, Function::D, alpha_bits("HELLO THERE FRIEND"))]);
        batches[0][1] ^= 0b111 << 20;
        batches[0][2] ^= 0b111 << 20;
        let p = &receive(&batches)[0];
        assert_eq!(p.end, End::Undecipherable);
        assert_eq!(p.content, Content::Alpha(Vec::new()));
        assert_eq!(p.decided, Decided::ByFunction);
    }

    /// Function B and C name no format: the content decides, and says so,
    /// and where both readings fit the page says it is ambiguous.
    #[test]
    fn functions_b_and_c_are_decided_by_content() {
        let digits = receive(&transmit(&[(16, Function::C, numeric_bits("5551234"))]));
        assert_eq!(digits[0].text(), "5551234");
        assert_eq!(digits[0].decided, Decided::ByContent);

        let text = receive(&transmit(&[(16, Function::B, alpha_bits("CALL BASE"))]));
        assert_eq!(text[0].text(), "CALL BASE");
        assert!(
            matches!(text[0].decided, Decided::ByContent | Decided::Ambiguous),
            "{:?}",
            text[0].decided
        );
        // Whichever it was, it is the one the rule gives for these bits.
        let codes = match &text[0].content {
            Content::Alpha(c) => c.clone(),
            other => panic!("{other:?}"),
        };
        assert!(reads_as_text(&codes));
    }

    /// Pagers name the function bits A to D: 00 is A.
    #[test]
    fn functions_are_lettered_from_00() {
        let letters: String = (0..4).map(|b| Function::from_bits(b).letter()).collect();
        assert_eq!(letters, "ABCD");
    }

    /// The symbol share: words pass, digits read 7 bits at a time do not.
    #[test]
    fn text_is_told_from_digits_read_as_text() {
        let as_codes = |s: &str| s.bytes().map(Some).collect::<Vec<_>>();
        assert!(reads_as_text(&as_codes("CALL BASE")));
        assert!(reads_as_text(&as_codes("WX 14C, 1018 HPA")));
        assert!(!reads_as_text(&as_codes("U*H!L")));
        assert!(!reads_as_text(&as_codes("12345")), "no letter");
    }

    /// A page cut off by the end of the signal is still a page, ended so.
    #[test]
    fn a_page_cut_off_ends_at_the_signal() {
        let batches = transmit(&[(8, Function::D, alpha_bits("CUT"))]);
        let mut a = Assembler::default();
        let mut first = batches[0];
        for w in first.iter_mut().skip(2) {
            *w = encode(0x8000_0000);
        }
        assert!(a.push_batch(&first).is_empty());
        assert_eq!(a.finish().map(|p| p.end), Some(End::Cut));
    }
}
