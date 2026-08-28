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
    /// Stale when the newest FFT frame has aged past [`FFT_STALE_MS`], or there
    /// is no frame yet. For anything derived from the spectrum.
    FftAge,
    /// Never stale. For panels that show configuration rather than measurement.
    Never,
}

/// How old the newest FFT frame may get before an [`Staleness::FftAge`] panel
/// calls its readings stale. Shared with `widgets::micro_common::fft_stale`.
pub const FFT_STALE_MS: u128 = 500;

impl Staleness {
    /// Resolve the rule against a metrics snapshot.
    pub fn resolve(self, state: &SdrMetrics) -> bool {
        self.decide(
            state.radio.hw_streaming,
            state
                .waterfall
                .last_fft
                .as_ref()
                .map(|fr| fr.timestamp.elapsed().as_millis()),
        )
    }

    /// The rule itself, on plain inputs: `fft_age_ms` is `None` when no frame has
    /// arrived yet. Split out from [`resolve`](Self::resolve) so the decision can
    /// be tested without building a whole metrics snapshot.
    fn decide(self, streaming: bool, fft_age_ms: Option<u128>) -> bool {
        match self {
            Staleness::NotStreaming => !streaming,
            Staleness::FftAge => fft_age_ms.map(|ms| ms > FFT_STALE_MS).unwrap_or(true),
            Staleness::Never => false,
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
    /// `[↑N]` - how far back through the history the view is scrolled. Absent at 0.
    Scroll(usize),
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
    /// default, so a panel that forgets `chrome()` renders instead of crashing -
    /// and `check-docs.py` names it the next time the lint runs.
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
}

pub trait Panel: Send + Sync {
    fn name(&self) -> &'static str;
    #[allow(dead_code)]
    fn min_size(&self) -> (u16, u16);

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

    #[test]
    fn staleness_rules_are_independent_of_each_other() {
        // A dead radio staleness NotStreaming, and nothing else.
        assert!(Staleness::NotStreaming.decide(false, Some(0)));
        assert!(
            !Staleness::FftAge.decide(false, Some(0)),
            "fresh frame, dead radio → live"
        );
        assert!(!Staleness::Never.decide(false, None), "Never means never");

        // A streaming radio whose FFT has dried up staleness only FftAge.
        assert!(!Staleness::NotStreaming.decide(true, None));
        assert!(Staleness::FftAge.decide(true, None), "no frame yet → stale");
        assert!(Staleness::FftAge.decide(true, Some(FFT_STALE_MS + 1)));
        assert!(
            !Staleness::FftAge.decide(true, Some(FFT_STALE_MS)),
            "the threshold itself is live"
        );
    }
}
