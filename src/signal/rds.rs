//! RDS — the protocol layer: block synchronisation, group assembly, and the
//! fields the panel shows (PI, PTY, programme service name, RadioText).
//!
//! The DSP that recovers the bitstream from the 57 kHz subcarrier lives in
//! [`super::rds_demod`]; everything here operates on a stream of already-recovered
//! bits, which makes the whole protocol testable without a radio or a waveform.
//!
//! Structure of the data, briefly, because the constants below only make sense
//! against it: RDS sends 26-bit *blocks* — 16 information bits followed by a
//! 10-bit checkword — in *groups* of four. The checkword is the CRC of the
//! information word XOR'd with a per-position **offset word**, which is what makes
//! the blocks self-locating: a receiver that computes the syndrome of a candidate
//! 26-bit window and gets one of the five offset words back has found a block
//! boundary and knows which position it is looking at.

/// Generator polynomial x¹⁰+x⁸+x⁷+x⁵+x⁴+x³+1, with the x¹⁰ term included.
const POLY: u32 = 0x5B9;

/// Bits per block: 16 information + 10 check.
pub const BLOCK_BITS: u32 = 26;

/// Offset words, per block position within a group. `C_PRIME` marks the "version
/// B" third block, which carries the PI code again instead of new data.
pub const OFFSET_A:       u16 = 0b0011111100;
pub const OFFSET_B:       u16 = 0b0110011000;
pub const OFFSET_C:       u16 = 0b0101101000;
pub const OFFSET_C_PRIME: u16 = 0b1101010000;
pub const OFFSET_D:       u16 = 0b0110110100;

/// Which position in a group a block occupies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockOffset { A, B, C, CPrime, D }

impl BlockOffset {
    /// The offset word XORed into this position's checkword. Only the encoder
    /// needs it — a decoder compares syndromes against the constants directly.
    #[cfg(test)]
    pub fn word(self) -> u16 {
        match self {
            BlockOffset::A      => OFFSET_A,
            BlockOffset::B      => OFFSET_B,
            BlockOffset::C      => OFFSET_C,
            BlockOffset::CPrime => OFFSET_C_PRIME,
            BlockOffset::D      => OFFSET_D,
        }
    }

    /// Index within the group, 0..=3. `C` and `C'` share position 2.
    pub fn index(self) -> usize {
        match self {
            BlockOffset::A => 0,
            BlockOffset::B => 1,
            BlockOffset::C | BlockOffset::CPrime => 2,
            BlockOffset::D => 3,
        }
    }

    /// The offset expected at the position after this one, or `None` after `D`
    /// (where the next block starts a new group at `A`).
    pub fn next(self) -> BlockOffset {
        match self {
            BlockOffset::A => BlockOffset::B,
            // Which of C / C' follows is decided by the group's version bit, so a
            // decoder must accept either at this position.
            BlockOffset::B => BlockOffset::C,
            BlockOffset::C | BlockOffset::CPrime => BlockOffset::D,
            BlockOffset::D => BlockOffset::A,
        }
    }
}

/// Remainder of a 26-bit block divided by the generator polynomial.
///
/// Because the encoder XORs the offset word into the checkword, and the syndrome
/// is linear, an error-free block's syndrome comes back as **exactly the offset
/// word that was used** — which is what turns the checkword into a position
/// marker as well as an error check.
pub fn syndrome(block: u32) -> u16 {
    let mut reg: u32 = 0;
    for i in (0..BLOCK_BITS).rev() {
        reg = (reg << 1) | ((block >> i) & 1);
        if reg & 0x400 != 0 { reg ^= POLY; }
    }
    (reg & 0x3FF) as u16
}

/// The checkword for an information word at a given block position: the CRC of
/// the information word, XOR the offset word. This is the transmitter's side of
/// the protocol, which sdrtop never performs — it exists so the decoder can be
/// proved against known-good data.
#[cfg(test)]
pub fn checkword(info: u16, offset: u16) -> u16 {
    syndrome((info as u32) << 10) ^ offset
}

/// Assemble a complete 26-bit block from an information word and a position.
/// Transmitter-side, and so test-only — see [`checkword`].
#[cfg(test)]
pub fn encode_block(info: u16, offset: BlockOffset) -> u32 {
    ((info as u32) << 10) | checkword(info, offset.word()) as u32
}

/// Identify a 26-bit window as a block at some position, if its syndrome matches
/// one of the offset words. `None` means the window is not a valid block — either
/// it straddles a boundary, or it has bit errors.
pub fn identify(block: u32) -> Option<BlockOffset> {
    match syndrome(block) {
        s if s == OFFSET_A       => Some(BlockOffset::A),
        s if s == OFFSET_B       => Some(BlockOffset::B),
        s if s == OFFSET_C       => Some(BlockOffset::C),
        s if s == OFFSET_C_PRIME => Some(BlockOffset::CPrime),
        s if s == OFFSET_D       => Some(BlockOffset::D),
        _ => None,
    }
}

/// The information word carried by a block.
pub fn info_of(block: u32) -> u16 { ((block >> 10) & 0xFFFF) as u16 }

/// Programme type names, indexed by the 5-bit PTY field (RDS / European table).
pub const PTY_NAMES: [&str; 32] = [
    "None", "News", "Current Affairs", "Information", "Sport", "Education", "Drama",
    "Culture", "Science", "Varied", "Pop Music", "Rock Music", "Easy Listening",
    "Light Classical", "Serious Classical", "Other Music", "Weather", "Finance",
    "Children", "Social Affairs", "Religion", "Phone In", "Travel", "Leisure",
    "Jazz Music", "Country Music", "National Music", "Oldies Music", "Folk Music",
    "Documentary", "Alarm Test", "Alarm",
];

/// Everything the decoder has learned about the station.
///
/// Text fields are only exposed once confirmed (see [`RdsDecoder`]), so a
/// mis-decoded character never reaches the display and then gets corrected in
/// front of the user.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RdsData {
    /// Programme Identification — the station's unique code. Present as soon as
    /// one valid block A arrives, which makes it the fastest thing to appear.
    pub pi:  Option<u16>,
    /// Programme type, as a 5-bit code; index into [`PTY_NAMES`].
    pub pty: Option<u8>,
    /// Traffic Programme / Traffic Announcement flags.
    pub tp:  bool,
    pub ta:  bool,
    /// Programme Service name, 8 characters. `None` until every position has been
    /// confirmed at least twice.
    pub ps:  Option<String>,
    /// RadioText, up to 64 characters, trimmed of trailing padding.
    pub rt:  Option<String>,
    /// Groups accepted since the decoder last reset — the honest measure of how
    /// well reception is going.
    pub groups_ok: u32,
}

/// Number of consistent sightings before a text character is published.
///
/// RDS has no forward error correction here beyond the block CRC, and a block can
/// pass its CRC with an undetected error. Requiring a character to arrive twice
/// with the same value costs one extra transmission of that group — under a second
/// in practice — and removes almost all of the flicker of wrong glyphs that a
/// naive decoder shows on a weak signal.
const CONFIRM_COUNT: u8 = 2;

/// Text buffer that publishes a character only once it has been seen the same way
/// [`CONFIRM_COUNT`] times running.
#[derive(Clone, Debug)]
struct ConfirmedText {
    chars:   Vec<u8>,
    pending: Vec<u8>,
    hits:    Vec<u8>,
}

impl ConfirmedText {
    fn new(len: usize) -> Self {
        Self { chars: vec![b' '; len], pending: vec![0; len], hits: vec![0; len] }
    }

    fn set(&mut self, i: usize, c: u8) {
        if i >= self.chars.len() { return; }
        if self.pending[i] == c {
            self.hits[i] = self.hits[i].saturating_add(1);
        } else {
            self.pending[i] = c;
            self.hits[i] = 1;
        }
        if self.hits[i] >= CONFIRM_COUNT {
            self.chars[i] = c;
        }
    }

    fn confirmed_any(&self) -> bool {
        self.hits.iter().any(|&h| h >= CONFIRM_COUNT)
    }

    /// The text as a display string: control characters become spaces, and
    /// trailing padding is trimmed.
    fn text(&self, limit: usize) -> String {
        let end = limit.min(self.chars.len());
        let s: String = self.chars[..end].iter()
            .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { ' ' })
            .collect();
        s.trim_end().to_string()
    }

    fn clear(&mut self) {
        self.chars.fill(b' ');
        self.pending.fill(0);
        self.hits.fill(0);
    }
}

/// Consumes a bitstream and produces [`RdsData`].
///
/// Synchronisation is deliberately conservative. The decoder hunts for a valid
/// block anywhere in the stream, then, once locked, expects blocks at the exact
/// positions the group structure predicts. A run of unrecognised blocks drops it
/// back to hunting rather than letting it drift and emit rubbish.
pub struct RdsDecoder {
    /// Sliding 26-bit window over the incoming bits.
    window:   u32,
    /// Bits seen since the last block boundary while locked.
    bit_count: u32,
    /// Where in the group the next block is expected, once synchronised.
    expect:   Option<BlockOffset>,
    /// Consecutive failures at an expected position, before giving up the lock.
    misses:   u32,
    /// Information words of the group being assembled.
    group:    [Option<u16>; 4],
    ps:       ConfirmedText,
    rt:       ConfirmedText,
    /// RadioText transmissions are restarted by toggling this flag; a change means
    /// the message has been replaced and the buffer must not blend old with new.
    rt_ab:    Option<bool>,
    rt_len:   usize,
    /// PI awaiting corroboration, and how many times it has been seen running.
    pi_pending: Option<u16>,
    pi_hits:  u8,
    data:     RdsData,
}

/// Consecutive missed blocks that cost the decoder its lock.
const MAX_MISSES: u32 = 3;

impl Default for RdsDecoder {
    fn default() -> Self { Self::new() }
}

impl RdsDecoder {
    pub fn new() -> Self {
        Self {
            window: 0, bit_count: 0, expect: None, misses: 0,
            group: [None; 4],
            ps: ConfirmedText::new(8),
            rt: ConfirmedText::new(64),
            rt_ab: None,
            rt_len: 0,
            pi_pending: None,
            pi_hits: 0,
            data: RdsData::default(),
        }
    }

    pub fn data(&self) -> &RdsData { &self.data }

    /// Whether the decoder currently holds block synchronisation.
    pub fn locked(&self) -> bool { self.expect.is_some() }

    /// Forget everything — used when the sample stream breaks, since bits from
    /// either side of a gap do not belong to the same message.
    pub fn reset(&mut self) {
        let keep_pi = self.data.pi;
        *self = Self::new();
        // The PI code identifies the station and does not change mid-reception;
        // keeping it avoids a visible flicker across a brief dropout.
        self.data.pi = keep_pi;
    }

    /// Feed one recovered bit.
    pub fn push_bit(&mut self, bit: u8) {
        self.window = ((self.window << 1) | (bit as u32 & 1)) & 0x3FF_FFFF;

        match self.expect {
            // Hunting: test every window position for any valid block.
            None => {
                if let Some(off) = identify(self.window) {
                    self.accept(off);
                    self.expect = Some(off.next());
                    self.bit_count = 0;
                    self.misses = 0;
                }
            }
            // Locked: only look on the predicted boundary.
            Some(expected) => {
                self.bit_count += 1;
                if self.bit_count < BLOCK_BITS { return; }
                self.bit_count = 0;

                match identify(self.window) {
                    Some(off) if off.index() == expected.index() => {
                        self.accept(off);
                        self.misses = 0;
                        self.expect = Some(off.next());
                    }
                    _ => {
                        // A block that fails its check is dropped, not guessed at.
                        self.misses += 1;
                        if self.misses >= MAX_MISSES {
                            self.expect = None;
                            self.group = [None; 4];
                        } else {
                            self.expect = Some(expected.next());
                        }
                    }
                }
            }
        }
    }

    fn accept(&mut self, off: BlockOffset) {
        let info = info_of(self.window);
        if off == BlockOffset::A {
            // A new group starts here; anything half-assembled is abandoned.
            self.group = [None; 4];
            // PI is published only once corroborated. A random 26-bit window
            // matches one of the five offset syndromes about once in 200, so over
            // a few hundred bits of noise a chance "block A" is not unlikely at
            // all — and one of those must never put a station code on screen.
            // Two sightings of the *same* word is evidence; one is not.
            if self.pi_pending == Some(info) {
                self.pi_hits = self.pi_hits.saturating_add(1);
            } else {
                self.pi_pending = Some(info);
                self.pi_hits = 1;
            }
            if self.pi_hits >= CONFIRM_COUNT {
                self.data.pi = Some(info);
            }
        }
        self.group[off.index()] = Some(info);
        if self.group.iter().all(|b| b.is_some()) {
            self.commit_group();
            self.group = [None; 4];
        }
    }

    fn commit_group(&mut self) {
        let (Some(_a), Some(b), Some(c), Some(d)) =
            (self.group[0], self.group[1], self.group[2], self.group[3]) else { return };

        self.data.groups_ok = self.data.groups_ok.saturating_add(1);

        // Block B always carries group type, version, TP and PTY.
        let group_type = (b >> 12) & 0xF;
        let version_b  = (b >> 11) & 1 == 1;
        self.data.tp   = (b >> 10) & 1 == 1;
        self.data.pty  = Some(((b >> 5) & 0x1F) as u8);

        match group_type {
            // 0A / 0B — programme service name, two characters per group.
            0 => {
                self.data.ta = (b >> 4) & 1 == 1;
                let idx = (b & 0x3) as usize * 2;
                self.ps.set(idx,     (d >> 8) as u8);
                self.ps.set(idx + 1, (d & 0xFF) as u8);
                if self.ps.confirmed_any() {
                    self.data.ps = Some(self.ps.text(8));
                }
            }
            // 2A / 2B — RadioText. 2A carries four characters in blocks C and D;
            // 2B carries two in block D and repeats the PI in block C.
            2 => {
                let ab = (b >> 4) & 1 == 1;
                if self.rt_ab != Some(ab) {
                    // The A/B flag toggled: this is a different message, so the
                    // buffer is cleared rather than showing two messages spliced.
                    self.rt.clear();
                    self.rt_ab = Some(ab);
                    self.rt_len = 0;
                }
                let seg = (b & 0xF) as usize;
                if version_b {
                    let idx = seg * 2;
                    self.rt.set(idx,     (d >> 8) as u8);
                    self.rt.set(idx + 1, (d & 0xFF) as u8);
                    self.rt_len = self.rt_len.max(idx + 2);
                } else {
                    let idx = seg * 4;
                    self.rt.set(idx,     (c >> 8) as u8);
                    self.rt.set(idx + 1, (c & 0xFF) as u8);
                    self.rt.set(idx + 2, (d >> 8) as u8);
                    self.rt.set(idx + 3, (d & 0xFF) as u8);
                    self.rt_len = self.rt_len.max(idx + 4);
                }
                if self.rt.confirmed_any() {
                    self.data.rt = Some(self.rt.text(self.rt_len.max(1)));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the 104-bit stream of one group from four information words.
    fn encode_group(info: [u16; 4], version_b: bool) -> Vec<u8> {
        let offs = [
            BlockOffset::A,
            BlockOffset::B,
            if version_b { BlockOffset::CPrime } else { BlockOffset::C },
            BlockOffset::D,
        ];
        let mut bits = Vec::with_capacity(104);
        for (w, off) in info.iter().zip(offs.iter()) {
            let block = encode_block(*w, *off);
            for i in (0..BLOCK_BITS).rev() {
                bits.push(((block >> i) & 1) as u8);
            }
        }
        bits
    }

    /// Group type in the top four bits of block B; version A is the zero bit
    /// below it, so both are simply absent from a group 0A word.
    const TP: u16 = 1 << 10;
    const GROUP_2A: u16 = 2 << 12;

    /// Block B for a group 0A: type 0, version A, TP, PTY, and the PS segment.
    fn block_b_ps(pty: u8, segment: u16) -> u16 {
        TP | ((pty as u16 & 0x1F) << 5) | segment
    }

    #[test]
    fn syndrome_of_a_valid_block_is_its_offset_word() {
        // The property the whole synchroniser rests on.
        for off in [BlockOffset::A, BlockOffset::B, BlockOffset::C,
                    BlockOffset::CPrime, BlockOffset::D] {
            for info in [0x0000u16, 0x1234, 0xABCD, 0xFFFF] {
                let block = encode_block(info, off);
                assert_eq!(syndrome(block), off.word(),
                           "offset {off:?} info {info:#06x}");
                assert_eq!(identify(block), Some(off));
                assert_eq!(info_of(block), info);
            }
        }
    }

    #[test]
    fn a_corrupted_block_fails_identification() {
        let block = encode_block(0x1234, BlockOffset::A);
        // Every single-bit error must be caught — that is the minimum the CRC owes.
        for bit in 0..BLOCK_BITS {
            let bad = block ^ (1 << bit);
            assert!(identify(bad).is_none(), "single-bit error at {bit} slipped through");
        }
    }

    #[test]
    fn decoder_recovers_a_group_and_confirms_pi_on_the_second() {
        let mut d = RdsDecoder::new();
        let group = || encode_group([0xB201, block_b_ps(10, 0), 0, 0x4142], false);
        for b in group() { d.push_bit(b); }
        // A whole group is proof enough for everything the group itself carries…
        assert_eq!(d.data().pty, Some(10));
        assert!(d.data().tp);
        assert_eq!(d.data().groups_ok, 1);
        assert!(d.locked());
        // …but not yet for PI, which a single chance syndrome hit could also
        // produce. One more sighting of the same word settles it.
        assert_eq!(d.data().pi, None);
        for b in group() { d.push_bit(b); }
        assert_eq!(d.data().pi, Some(0xB201));
        assert_eq!(d.data().groups_ok, 2);
    }

    #[test]
    fn a_lone_chance_block_never_publishes_a_station_code() {
        // The failure this guards: a 26-bit window matches an offset syndrome by
        // luck, and a station code appears out of noise. One block A is not
        // evidence — and two *different* ones are not either.
        let mut d = RdsDecoder::new();
        for b in encode_group([0xB201, block_b_ps(10, 0), 0, 0x4142], false).into_iter().take(26) {
            d.push_bit(b);
        }
        assert_eq!(d.data().pi, None);
        let mut d = RdsDecoder::new();
        for pi in [0xB201u16, 0x1234] {
            for b in encode_group([pi, block_b_ps(10, 0), 0, 0x4142], false) { d.push_bit(b); }
        }
        assert_eq!(d.data().pi, None, "disagreeing sightings must not confirm either");
    }

    #[test]
    fn decoder_assembles_a_programme_service_name() {
        let mut d = RdsDecoder::new();
        // "TESTFM__" over four segments, sent twice so every character confirms.
        let pairs: [(u16, u16); 4] = [
            (0, u16::from_be_bytes(*b"TE")),
            (1, u16::from_be_bytes(*b"ST")),
            (2, u16::from_be_bytes(*b"FM")),
            (3, u16::from_be_bytes(*b"  ")),
        ];
        for _ in 0..CONFIRM_COUNT {
            for (seg, chars) in pairs {
                for b in encode_group([0xB201, block_b_ps(3, seg), 0, chars], false) {
                    d.push_bit(b);
                }
            }
        }
        assert_eq!(d.data().ps.as_deref(), Some("TESTFM"));
    }

    #[test]
    fn a_single_sighting_does_not_publish_text() {
        let mut d = RdsDecoder::new();
        // One pass only: below the confirmation threshold, so nothing is shown.
        for seg in 0..4u16 {
            let chars = u16::from_be_bytes([b'A' + seg as u8, b'Z']);
            for b in encode_group([0xB201, block_b_ps(3, seg), 0, chars], false) {
                d.push_bit(b);
            }
        }
        assert_eq!(d.data().ps, None, "unconfirmed text must not reach the display");
        // …but the PI, which needs no confirmation, is already there.
        assert_eq!(d.data().pi, Some(0xB201));
    }

    #[test]
    fn decoder_assembles_radiotext() {
        let mut d = RdsDecoder::new();
        let text = b"HELLO WORLD ";
        for _ in 0..CONFIRM_COUNT {
            for seg in 0..3u16 {
                let i = seg as usize * 4;
                let c = u16::from_be_bytes([text[i], text[i + 1]]);
                let dd = u16::from_be_bytes([text[i + 2], text[i + 3]]);
                // Group 2A: type 2, version A, A/B flag clear, segment in low bits.
                let b = GROUP_2A | TP | (3 << 5) | seg;
                for bit in encode_group([0xB201, b, c, dd], false) {
                    d.push_bit(bit);
                }
            }
        }
        assert_eq!(d.data().rt.as_deref(), Some("HELLO WORLD"));
    }

    #[test]
    fn radiotext_ab_toggle_clears_the_old_message() {
        let mut d = RdsDecoder::new();
        let send = |d: &mut RdsDecoder, text: &[u8; 4], ab: u16| {
            for _ in 0..CONFIRM_COUNT {
                let c = u16::from_be_bytes([text[0], text[1]]);
                let dd = u16::from_be_bytes([text[2], text[3]]);
                let b = (2 << 12) | (1 << 10) | (ab << 4) | (3 << 5);
                for bit in encode_group([0xB201, b, c, dd], false) { d.push_bit(bit); }
            }
        };
        send(&mut d, b"AAAA", 0);
        assert_eq!(d.data().rt.as_deref(), Some("AAAA"));
        // A toggled flag means a new message; the old text must not survive in it.
        send(&mut d, b"BBBB", 1);
        let rt = d.data().rt.clone().unwrap_or_default();
        assert!(!rt.contains('A'), "old message leaked into the new one: {rt:?}");
    }

    #[test]
    fn decoder_finds_sync_from_an_arbitrary_bit_offset() {
        let mut d = RdsDecoder::new();
        // Junk before the stream: the hunt must not be confused by it.
        for b in [1u8, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0] { d.push_bit(b); }
        for _ in 0..CONFIRM_COUNT {
            for seg in 0..4u16 {
                let chars = u16::from_be_bytes(*b"OK");
                for b in encode_group([0x1234, block_b_ps(1, seg), 0, chars], false) {
                    d.push_bit(b);
                }
            }
        }
        assert_eq!(d.data().pi, Some(0x1234));
        assert!(d.data().groups_ok >= 4);
    }

    #[test]
    fn sustained_garbage_drops_the_lock() {
        let mut d = RdsDecoder::new();
        for b in encode_group([0xB201, block_b_ps(3, 0), 0, 0x4142], false) { d.push_bit(b); }
        assert!(d.locked());
        // Enough invalid blocks in a row and the decoder must let go rather than
        // keep publishing from a boundary it no longer believes in.
        for i in 0..(BLOCK_BITS * (MAX_MISSES + 1)) {
            d.push_bit((i % 2) as u8);
        }
        assert!(!d.locked(), "decoder held a lock it should have lost");
    }

    #[test]
    fn reset_keeps_the_station_identity_only() {
        let mut d = RdsDecoder::new();
        for _ in 0..CONFIRM_COUNT {
            for seg in 0..4u16 {
                for b in encode_group([0xB201, block_b_ps(3, seg), 0, 0x4142], false) {
                    d.push_bit(b);
                }
            }
        }
        assert!(d.data().ps.is_some());
        d.reset();
        // PI identifies the station and survives a dropout; text does not, since
        // bits from either side of a gap are not the same message.
        assert_eq!(d.data().pi, Some(0xB201));
        assert_eq!(d.data().ps, None);
        assert_eq!(d.data().groups_ok, 0);
        assert!(!d.locked());
    }

    #[test]
    fn pty_table_is_complete_and_indexable() {
        assert_eq!(PTY_NAMES.len(), 32);
        assert_eq!(PTY_NAMES[0], "None");
        assert_eq!(PTY_NAMES[31], "Alarm");
    }
}
