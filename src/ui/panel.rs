// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use crate::state::SdrMetrics;
use ratatui::{layout::Rect, Frame};

/// How a panel is fused to a vertically-adjacent neighbour. When the spectrum
/// sits directly above the waterfall they render as one bonded instrument with a
/// single shared frequency ruler: the spectrum drops its bottom border + its own
/// frequency axis (`Below`), and the waterfall's top border becomes that shared
/// ruler (`Above`). `None` is the normal, standalone framing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bond {
    None,
    Below,
    Above,
}

/// A panel's half of a bonded instrument: which half it draws as, and which
/// panel is the other half.
///
/// **Declared by both halves, never inferred from names.** The engine bonds a
/// centre column of exactly two panels when the upper one declares
/// `Bond::Below` with the lower as its partner and the lower declares
/// `Bond::Above` with the upper as its partner. Either declaration alone is not
/// enough: a panel that can be the top of one instrument must not bond with
/// whatever happens to be stacked under it. Spectrum over waterfall was the only
/// pair, and the engine used to check for their two names; the NET survey's
/// occupancy over coexistence is the second (net-ux-polish-plan Stop 3.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bonding {
    pub role: Bond,
    pub partner: &'static str,
}

/// What makes a panel's readings go stale, i.e. when the engine tags its title
/// `[STALE]` and cools its border.
///
/// Declaring this instead of computing it per panel is what makes the project's
/// "a frozen number must never read as live" rule enforceable in one place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Staleness {
    /// Stale whenever the radio is not streaming. For anything read from
    /// hardware counters: timing, drops, gain staging, IQ balance.
    NotStreaming,
    /// Stale when the newest FFT frame exceeds the device's trace-age limit, or
    /// there is no frame yet. For anything derived from the spectrum.
    FftAge,
    /// Never stale. For panels that show configuration rather than measurement.
    Never,
}

impl Staleness {
    /// Resolve the rule against a metrics snapshot.
    pub fn resolve(self, state: &SdrMetrics) -> bool {
        let age = state
            .waterfall
            .last_fft
            .as_ref()
            .map(|frame| frame.timestamp.elapsed().as_millis());
        let stale = self.decide(state.radio.hw_streaming, age, state.caps.trace_stale_ms);
        stale
            || (self == Staleness::FftAge
                && state.caps.acquisition == crate::hardware::AcquisitionKind::PowerTrace
                && !state.radio.hw_streaming)
    }

    /// The rule itself, on plain inputs: `fft_age_ms` is `None` when no frame has
    /// arrived yet. Split out from [`resolve`](Self::resolve) so the decision can
    /// be tested without building a whole metrics snapshot.
    fn decide(self, streaming: bool, fft_age_ms: Option<u128>, trace_stale_ms: u128) -> bool {
        match self {
            Staleness::NotStreaming => !streaming,
            Staleness::FftAge => fft_age_ms
                .map(|milliseconds| milliseconds > trace_stale_ms)
                .unwrap_or(true),
            Staleness::Never => false,
        }
    }
}

/// How far back into the sample feed a panel's numbers reach - declared so the
/// engine can say, in one place, whether the feed lost anything inside that
/// span.
///
/// Foundation design 13.2: every count in the NET section is a lower bound
/// once the feed has dropped samples, because the three ways a block goes
/// missing are invisible downstream. But "the feed lost something" is only a
/// caveat on the numbers that span the loss: a census accumulated over the
/// session is undercounted by a drop ten minutes ago, the duty cycle of the
/// dwell just finished is not. So a panel declares the span its numbers
/// cover, and the engine tags it [`Tag::FeedLoss`] only when the last loss
/// falls inside it - never computed in the panel, the same rule as
/// [`Staleness`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FeedSpan {
    /// Accumulated for as long as the feed's own account runs: totals, the
    /// census, per-channel counts.
    Session,
    /// Measured over the most recent stretch of this length only: a dwell, a
    /// scrolling window.
    Window(std::time::Duration),
}

impl FeedSpan {
    /// Resolve against a metrics snapshot: did the feed lose anything inside
    /// this span?
    pub fn resolve(self, state: &SdrMetrics) -> bool {
        self.decide(state.net.health.last_loss.map(|t| t.elapsed()))
    }

    /// The rule on plain inputs: `since_loss` is how long ago the feed last
    /// lost anything, `None` if it never has.
    fn decide(self, since_loss: Option<std::time::Duration>) -> bool {
        match (self, since_loss) {
            (_, None) => false,
            (FeedSpan::Session, Some(_)) => true,
            (FeedSpan::Window(span), Some(age)) => age <= span,
        }
    }
}

/// A live-state tag the engine appends to a panel's title after the name.
///
/// Semantic rather than textual on purpose: the panel says *what is true*, the
/// engine decides how it is spelled and coloured, so tags stay consistent across
/// the deck and no panel file names a colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tag {
    /// `[FRZ]` - the readings are a held snapshot, not live.
    Frozen,
    /// `[HOLD]` - the trace is frozen against a captured frame.
    Hold,
    /// `[PAUSED]` - the panel has stopped taking new data because the user said
    /// so.
    ///
    /// Carries two consequences beyond its own spelling, both in
    /// [`chrome::frame`](crate::ui::chrome::frame), and both because a paused
    /// panel is *not advancing* exactly as a stale one is not:
    ///
    /// 1. It cools the frame the way staleness does.
    /// 2. It **suppresses `[STALE]`**. A paused panel is not stale, it is held -
    ///    and printing both would be two answers to one question. The plate says
    ///    which of the two it is; the border only says that it is one of them.
    Paused,
    /// `[×N]` - frames averaged into each history row. Absent at 1.
    Stride(usize),
    /// `[FILTERED]` - a list narrowed to one thing the user picked; the rows
    /// say which, and the tag says the list is not everything.
    Filtered,
    /// `[+N NEW]` - arrivals a paused list is not showing, so its pause says
    /// what it is costing.
    Behind(u64),
    /// `[LE 1M]` / `[LE 2M]` - the PHY the BLE decoder is listening for, so
    /// every row is read against it (net-ux-polish-plan 5.5).
    Phy(crate::signal::ble::Phy),
    /// `[↑N]` - how far back through the history the view is scrolled. Absent at 0.
    Scroll(usize),
    /// `[20 s ◂ now]` / `[2.0 s ◂ -8.0 s]` - the stretch of time a plot
    /// shows and where it ends, in milliseconds: a zoomed or scrubbed plot
    /// says which piece of the past it is, so a gap in it is not read as a
    /// quiet now.
    TimeWindow { span_ms: u64, back_ms: u64 },
    /// `[SURVEY]` - the numbers on this panel were gathered by sampling the
    /// band, not by watching all of it.
    ///
    /// **A duty-cycle-sampled census and a complete capture are different
    /// claims**, and presenting one as the other is what rule 4 exists to
    /// prevent. Design section 13.1 makes the mode part of the reading rather
    /// than a setting, so every panel in the section that carries a number
    /// carries this too, and a structural test says so.
    Survey,
    /// `[LOCK]` - parked on one channel: complete inside it, blind outside it.
    Lock,
    /// `[↓PKTS]` - what orders a table, and which way.
    ///
    /// Design section 9.1: the sort key is shown in the chrome "so the panel
    /// says how it is ordered rather than the user having to remember". A table
    /// whose order is only visible in a marker halfway across the header is one
    /// people read wrong from the other side of the room.
    Sorted(&'static str, bool),
    /// `[FEED LOSS]` - the sample feed dropped blocks inside the span this
    /// panel's numbers cover, so its counts are lower bounds.
    ///
    /// **Never pushed by a panel.** A panel declares its [`FeedSpan`] and the
    /// engine adds this tag when the last loss falls inside it, the way it adds
    /// `[STALE]`: a caveat a panel had to remember to print is one the next
    /// panel would forget.
    FeedLoss,
    /// `[RELATIVE]`, `[TRACEABLE]`, `[REFERENCED]` or `[RELATIVE: REF
    /// EXPIRED]` - what the frequency offsets on this panel are worth, from
    /// the reference the RF bench establishes (`state::RadioState::
    /// offset_basis`). The same words its FREQUENCY REFERENCE card uses, so
    /// one reference reads one way wherever it is shown (rule 5).
    ///
    /// **Never pushed by a panel**, for [`Tag::FeedLoss`]'s reason: a panel
    /// declares that it shows offsets ([`PanelChrome::shows_offsets`]) and
    /// the engine says what they are worth.
    Offsets(crate::state::OffsetBasis),
    /// `[OUI]` - how addresses on this panel are shown, when it is not the
    /// full address (`state::AddressDisplay`). Engine-owned like
    /// [`Tag::Offsets`]: the panel declares that it prints addresses
    /// ([`PanelChrome::shows_addresses`]), and a switch that changes every
    /// panel at once is said on every panel at once.
    Addresses(crate::state::AddressDisplay),
}

/// The *shape* of a panel's frame. Its colour is [`FrameTone`]; the two are
/// separate because the same shape appears in several palettes and pairing them
/// into one enum multiplied out into special cases.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameStyle {
    /// Rounded single border with a `␣Name␣` nameplate: the instrument box every
    /// lab and micro panel sits in.
    Instrument,
    /// Square border with reinforced `┏┓┗┛` corners and a `╴NAME╶` tick-tab
    /// nameplate: the schematic-deck cockpit chrome that surrounds the
    /// instruments.
    Deck,
    /// No frame at all. The panel gets the whole rect and draws edge to edge:
    /// the lab banner and marker bars, which are single lines of readout.
    Borderless,
    /// The panel draws its own frame and receives the outer `Rect` untouched.
    ///
    /// No registered panel declares this any more: since R4h every one of them
    /// says what it is and the engine frames it. It survives as the trait
    /// default, so a panel that forgets `chrome()` renders instead of crashing.
    ///
    /// Nothing checks for that any more. The lint that used to name a panel
    /// falling back here is gone, and no test replaced it, so this is a
    /// convention now rather than a rule.
    ///
    /// The spectrum and the waterfall still have a self-drawn path, but the
    /// layout engine calls it directly rather than through the registry, and only
    /// when the two are **bonded**: there the spectrum drops its bottom edge and
    /// the waterfall's top border *becomes* the shared frequency ruler, which is
    /// content no generic frame can draw. Even then the plate and the border
    /// colour come from the panel's own [`PanelChrome`]; only the border *set* is
    /// its own.
    SelfFramed,
}

/// Which palette slot a frame is drawn in when it is not focused or stale.
///
/// Semantic slots rather than colours, so a panel says what its border means and
/// the engine decides how that looks in each of the six themes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameTone {
    /// The ordinary instrument border.
    Default,
    /// Lit: the primary instruments, which lead the eye on every preset. The
    /// spectrum and the waterfall, and nothing else - an accent every panel wore
    /// would be no accent at all.
    Accent,
    /// Receded: supporting chrome that should not compete with the instruments.
    Dim,
    /// Lit as though focused, for a panel that is carrying the user's attention
    /// without being focusable itself (the footer during an input prompt).
    Focused,
    /// Something needs an answer: the footer while a value is being typed.
    Warn,
    /// Observer mode, where the radio belongs to another process.
    Observer,
}

impl FrameTone {
    pub fn color(self, theme: &crate::Theme) -> ratatui::style::Color {
        match self {
            FrameTone::Default => theme.border_default,
            FrameTone::Accent => theme.border_accent,
            FrameTone::Dim => theme.border_dim,
            FrameTone::Focused => theme.border_focused,
            FrameTone::Warn => theme.status_warn,
            FrameTone::Observer => theme.observer,
        }
    }
}

/// Everything the engine needs to frame a panel. A panel describes itself; it
/// never draws its own border, title or `[STALE]` tag.
///
/// Build one with [`PanelChrome::new`] and the chaining setters:
///
/// ```ignore
/// PanelChrome::new("RF _Diagnostics")
///     .stale_when(Staleness::NotStreaming)
///     .tag_if(state.lab.rf_freeze.is_some(), Tag::Frozen)
/// ```
#[derive(Clone, Debug)]
pub struct PanelChrome {
    /// Display name. A single `_` marks the focus-key letter for the inline
    /// highlight (`"Sig_nal Metrics"` draws `Sig` `n` `al Metrics` with the `n`
    /// lit). Leave the marker out and the engine appends the key in brackets
    /// instead (`"Signal Characterization"` with key `x` draws
    /// `Signal Characterization [X]`), which is the form for keys that are not a
    /// letter of the name. Empty means an untitled box.
    pub title: &'static str,
    /// What makes this panel's readings go stale.
    pub staleness: Staleness,
    /// Border shape.
    pub frame: FrameStyle,
    /// Border palette slot when the frame is neither focused nor stale.
    pub tone: FrameTone,
    /// Live-state tags, drawn after the name in declaration order.
    pub tags: Vec<Tag>,
    /// Trailing detail drawn after the name and tags in the plain label colour,
    /// e.g. the sweep panel's band and dwell. Appended **verbatim**, so a panel
    /// that wants a separating space writes one.
    pub suffix: Option<String>,
    /// How far back into the sample feed this panel's numbers reach, when they
    /// come from it at all. `None` for anything the feed does not count.
    pub feed: Option<FeedSpan>,
    /// Whether this panel shows a transmitter's frequency offset, which
    /// earns it the engine's [`Tag::Offsets`].
    pub offsets: bool,
    /// Whether this panel prints device addresses, which earns it the
    /// engine's [`Tag::Addresses`] whenever they are not shown in full.
    pub addresses: bool,
    /// Whether this panel prints classic LAPs, which only the masked mode
    /// changes (`NetState::show_lap`), so it earns [`Tag::Addresses`] then
    /// and only then: in `oui` a LAP is shown as it is, and a tag saying
    /// otherwise would be wrong.
    pub laps: bool,
}

impl PanelChrome {
    /// A titled instrument box that never goes stale. Add the rest by chaining.
    pub fn new(title: &'static str) -> Self {
        Self {
            title,
            staleness: Staleness::Never,
            frame: FrameStyle::Instrument,
            tone: FrameTone::Default,
            tags: Vec::new(),
            suffix: None,
            feed: None,
            offsets: false,
            addresses: false,
            laps: false,
        }
    }

    /// A `╴NAME╶` deck frame in the dim palette: the cockpit chrome around the
    /// instruments. The one-call shorthand, since every deck panel wants both.
    pub fn deck(title: &'static str) -> Self {
        Self::new(title)
            .frame(FrameStyle::Deck)
            .tone(FrameTone::Dim)
    }

    /// An instrument box with no nameplate, for panels that head their own
    /// content (the micro field views, the sweep strip).
    pub fn untitled() -> Self {
        Self::new("")
    }

    /// "This panel frames itself" - the trait default, and the escape hatch for
    /// the chrome that is not a plain box.
    pub fn self_framed() -> Self {
        Self {
            frame: FrameStyle::SelfFramed,
            ..Self::new("")
        }
    }

    pub fn stale_when(mut self, staleness: Staleness) -> Self {
        self.staleness = staleness;
        self
    }

    pub fn frame(mut self, frame: FrameStyle) -> Self {
        self.frame = frame;
        self
    }

    pub fn tone(mut self, tone: FrameTone) -> Self {
        self.tone = tone;
        self
    }

    /// Append a tag when `cond` holds. The conditional form is the common one,
    /// since tags exist to report live state.
    pub fn tag_if(mut self, cond: bool, tag: Tag) -> Self {
        if cond {
            self.tags.push(tag);
        }
        self
    }

    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    /// Declare that this panel's numbers are counted from the sample feed over
    /// `span`. The engine decides from that whether to add [`Tag::FeedLoss`].
    pub fn counts_from_feed(mut self, span: FeedSpan) -> Self {
        self.feed = Some(span);
        self
    }

    /// Declare that this panel shows a transmitter's frequency offset, so the
    /// engine tags it with what that offset is worth ([`Tag::Offsets`]).
    pub fn shows_offsets(mut self) -> Self {
        self.offsets = true;
        self
    }

    /// Declare that this panel prints classic LAPs, so the engine can say
    /// when they are masked ([`Tag::Addresses`]).
    pub fn shows_laps(mut self) -> Self {
        self.laps = true;
        self
    }

    /// Declare that this panel prints device addresses, so the engine can say
    /// when they are not shown in full ([`Tag::Addresses`]).
    pub fn shows_addresses(mut self) -> Self {
        self.addresses = true;
        self
    }

    /// Add the tags the engine owns - the ones a panel declares the grounds for
    /// but never pushes itself. Called once, where the frame is drawn.
    pub fn with_engine_tags(mut self, state: &SdrMetrics) -> Self {
        if self.feed.is_some_and(|span| span.resolve(state)) {
            self.tags.push(Tag::FeedLoss);
        }
        if self.offsets {
            let basis = state.radio.offset_basis(std::time::Instant::now());
            self.tags.push(Tag::Offsets(basis));
        }
        let display = state.net.address_display;
        let masked = display == crate::state::AddressDisplay::Masked;
        if (self.addresses && display != crate::state::AddressDisplay::Full)
            || (self.laps && masked)
        {
            self.tags.push(Tag::Addresses(display));
        }
        self
    }
}

pub trait Panel: Send + Sync {
    fn name(&self) -> &'static str;
    #[allow(dead_code)]
    fn min_size(&self) -> (u16, u16);

    fn supports_acquisition(&self, acquisition: crate::hardware::AcquisitionKind) -> bool {
        acquisition == crate::hardware::AcquisitionKind::IqSamples
    }

    /// Draw the panel's contents into `area`.
    ///
    /// `area` is the **inner** rect the engine has already carved out, guaranteed
    /// non-empty. Only a panel that fell back to the default
    /// [`FrameStyle::SelfFramed`] gets the outer rect instead, and none should.
    fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        focused: bool,
    );

    /// How the engine should frame this panel: name, focus-key highlight,
    /// staleness rule, border language and live tags.
    ///
    /// Takes the metrics snapshot so tags and the suffix can be live. The
    /// default frames nothing and hands the panel its outer rect.
    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::self_framed()
    }

    /// Which half of a bonded instrument this panel can be, if any. See
    /// [`Bonding`]. `None`, the default, is a panel that always stands alone.
    fn bonding(&self) -> Option<Bonding> {
        None
    }

    /// Draw as one half of a bonded pair. `area` is the **outer** rect: a bonded
    /// half draws its own reduced border set, because the seam between the two
    /// halves is the one frame the engine cannot draw from a single chrome.
    /// Called only for a panel whose [`Self::bonding`] is `Some` and whose
    /// partner is stacked with it, with `bond` equal to its declared role.
    fn render_bonded(
        &self,
        _f: &mut Frame,
        _area: Rect,
        _state: &SdrMetrics,
        _theme: &crate::Theme,
        _focused: bool,
        _bond: Bond,
    ) {
    }

    /// Single character that activates panel-focus mode for this panel.
    /// Returns `None` for panels that don't support focus mode.
    fn focus_key(&self) -> Option<char> {
        None
    }

    /// Keybindings shown in the footer when this panel is focused.
    /// Each entry: (key_label, description). Empty by default.
    /// Do NOT include Esc or Tab - the footer appends those automatically.
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// Preferred rendered height in rows, given the available terminal width and current state.
    /// Used by the layout engine for top/bottom panels. Default: 3 (1 content + 2 borders).
    fn preferred_height(&self, _available_width: u16, _state: &SdrMetrics) -> u16 {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyPanel;

    impl Panel for DummyPanel {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn min_size(&self) -> (u16, u16) {
            (10, 3)
        }
        fn render(
            &self,
            _f: &mut Frame,
            _area: Rect,
            _state: &SdrMetrics,
            _theme: &crate::Theme,
            _focused: bool,
        ) {
        }
    }

    #[test]
    fn panel_name_and_min_size() {
        let p = DummyPanel;
        assert_eq!(p.name(), "dummy");
        assert_eq!(p.min_size(), (10, 3));
    }

    #[test]
    fn chrome_builder_defaults_and_chaining() {
        let plain = PanelChrome::new("Level Diagram");
        assert_eq!(plain.staleness, Staleness::Never);
        assert_eq!(plain.frame, FrameStyle::Instrument);
        assert!(plain.tags.is_empty() && plain.suffix.is_none());

        let built = PanelChrome::new("RF _Diagnostics")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, Tag::Frozen)
            .tag_if(false, Tag::Frozen)
            .suffix(" · held");
        assert_eq!(
            built.tags,
            vec![Tag::Frozen],
            "only the true condition adds a tag"
        );
        assert_eq!(built.suffix.as_deref(), Some(" · held"));
    }

    /// A loss is a caveat only on the numbers whose span it falls inside.
    #[test]
    fn a_feed_loss_caveats_only_the_spans_it_falls_inside() {
        use std::time::Duration;
        let ten_min_ago = Some(Duration::from_secs(600));
        let just_now = Some(Duration::from_millis(300));
        let window = FeedSpan::Window(Duration::from_secs(20));

        // Never lost anything: nothing is caveated, whatever the span.
        assert!(!FeedSpan::Session.decide(None));
        assert!(!window.decide(None));

        // A session total spans every loss there has been.
        assert!(FeedSpan::Session.decide(ten_min_ago));
        assert!(FeedSpan::Session.decide(just_now));

        // A window spans only the recent ones: an old drop says nothing about
        // what was measured since.
        assert!(!window.decide(ten_min_ago));
        assert!(window.decide(just_now));
        assert!(
            window.decide(Some(Duration::from_secs(20))),
            "the edge is inside"
        );
    }

    /// The engine adds the tag; a panel only declares the span. A panel that
    /// declares nothing is never tagged, however lossy the feed.
    #[test]
    fn the_engine_adds_the_feed_loss_tag_from_the_declaration() {
        let mut m = SdrMetrics::fixture();
        m.net.health.last_loss = Some(std::time::Instant::now());

        let declared = PanelChrome::new("Census")
            .counts_from_feed(FeedSpan::Session)
            .with_engine_tags(&m);
        assert!(declared.tags.contains(&Tag::FeedLoss));

        let silent = PanelChrome::new("Capability").with_engine_tags(&m);
        assert!(!silent.tags.contains(&Tag::FeedLoss));

        m.net.health.last_loss = None;
        let clean = PanelChrome::new("Census")
            .counts_from_feed(FeedSpan::Session)
            .with_engine_tags(&m);
        assert!(
            !clean.tags.contains(&Tag::FeedLoss),
            "a clean feed caveats nothing"
        );
    }

    /// The engine says what the offsets are worth; a panel only declares that
    /// it shows some. A panel that shows none is never tagged, reference or
    /// not.
    #[test]
    fn the_engine_adds_the_offset_tag_from_the_declaration() {
        use crate::state::{OffsetBasis, Provenance};
        let mut m = SdrMetrics::fixture();
        let tag = |m: &SdrMetrics| {
            PanelChrome::new("Census")
                .shows_offsets()
                .with_engine_tags(m)
                .tags
                .into_iter()
                .find(|t| matches!(t, Tag::Offsets(_)))
        };
        assert_eq!(
            tag(&m),
            Some(Tag::Offsets(OffsetBasis {
                provenance: Provenance::Unreferenced,
                expired: false
            }))
        );

        m.radio.reference = Some(crate::state::FrequencyReference {
            ppm: 1.0,
            sigma_ppm: 0.1,
            provenance: Provenance::Traceable,
            source: "WWV 10 MHz".to_string(),
            at: std::time::Instant::now(),
            efficiency: None,
            trusted: None,
        });
        assert_eq!(
            tag(&m),
            Some(Tag::Offsets(OffsetBasis {
                provenance: Provenance::Traceable,
                expired: false
            }))
        );

        let silent = PanelChrome::new("Capability").with_engine_tags(&m);
        assert!(!silent.tags.iter().any(|t| matches!(t, Tag::Offsets(_))));
    }

    #[test]
    fn staleness_rules_are_independent_of_each_other() {
        // A dead radio staleness NotStreaming, and nothing else.
        let stale_ms = crate::hardware::IQ_TRACE_STALE_MS;
        assert!(Staleness::NotStreaming.decide(false, Some(0), stale_ms));
        assert!(
            !Staleness::FftAge.decide(false, Some(0), stale_ms),
            "fresh frame, dead radio → live"
        );
        assert!(
            !Staleness::Never.decide(false, None, stale_ms),
            "Never means never"
        );

        // A streaming radio whose FFT has dried up staleness only FftAge.
        assert!(!Staleness::NotStreaming.decide(true, None, stale_ms));
        assert!(
            Staleness::FftAge.decide(true, None, stale_ms),
            "no frame yet → stale"
        );
        assert!(Staleness::FftAge.decide(true, Some(stale_ms + 1), stale_ms));
        assert!(
            !Staleness::FftAge.decide(true, Some(stale_ms), stale_ms),
            "the threshold itself is live"
        );
    }

    #[test]
    fn power_trace_staleness_uses_the_device_limit_and_rx_state() {
        let mut state = SdrMetrics::fixture().streaming().with_carrier(0.0, 20.0);
        let mut caps = (*state.caps).clone();
        caps.acquisition = crate::hardware::AcquisitionKind::PowerTrace;
        caps.trace_stale_ms = 100;
        state.caps = std::sync::Arc::new(caps);
        state.waterfall.last_fft.as_mut().unwrap().timestamp =
            std::time::Instant::now() - std::time::Duration::from_millis(50);

        assert!(!Staleness::FftAge.resolve(&state));
        state.caps = {
            let mut caps = (*state.caps).clone();
            caps.trace_stale_ms = 10;
            std::sync::Arc::new(caps)
        };
        assert!(Staleness::FftAge.resolve(&state));

        state.waterfall.last_fft.as_mut().unwrap().timestamp = std::time::Instant::now();
        state.radio.hw_streaming = false;
        assert!(Staleness::FftAge.resolve(&state));
    }

    #[test]
    fn trace_staleness_uses_the_device_limit() {
        let mut state = SdrMetrics::fixture().streaming().with_carrier(0.0, 20.0);
        std::sync::Arc::make_mut(&mut state.caps).trace_stale_ms = 60_000;
        state.waterfall.last_fft.as_mut().unwrap().timestamp =
            std::time::Instant::now() - std::time::Duration::from_secs(1);
        assert!(!Staleness::FftAge.resolve(&state));

        std::sync::Arc::make_mut(&mut state.caps).trace_stale_ms = 100;
        assert!(Staleness::FftAge.resolve(&state));

        state.waterfall.last_fft = None;
        assert!(Staleness::FftAge.resolve(&state));
    }

    #[test]
    fn trace_age_threshold_is_inclusive_for_each_device() {
        for limit in [0, 100, 500, 60_000] {
            assert!(!Staleness::FftAge.decide(false, Some(limit), limit));
            assert!(Staleness::FftAge.decide(true, Some(limit + 1), limit));
        }
    }
}
