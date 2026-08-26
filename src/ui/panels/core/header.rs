use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::chrome::frame;
use crate::ui::panel::{Panel, PanelChrome};
use crate::ui::widgets::band_plan::band_at;
use crate::ui::widgets::charts::gain_bar_colored;

pub struct HeaderPanel;

/// A "breathing" RX status dot that cycles small→large→small on a ~0.9 s loop.
/// Pure glyph animation — the badge colours never change. All four glyphs are a
/// single terminal column, so the badge width (and the header gap math) is fixed.
/// Only animates while frames are flowing (RX), which is exactly when the UI
/// is already redrawing, so it costs no extra wakeups.
fn rx_pulse_glyph() -> &'static str {
    use std::time::{SystemTime, UNIX_EPOCH};
    const FRAMES: [&str; 4] = ["\u{2219}", "\u{2022}", "\u{25CF}", "\u{2022}"]; // ∙ • ● •
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    FRAMES[((ms / 220) % FRAMES.len() as u128) as usize]
}

/// A dot-leader span that fills `gap` columns: ` ······ ` (one space at each
/// end, dim dots between), connecting a left field to a right one like an
/// engraved instrument readout. Falls back to plain spaces when too short.
fn leader(gap: usize, color: ratatui::style::Color) -> Span<'static> {
    if gap >= 4 {
        Span::styled(
            format!(" {} ", "·".repeat(gap - 2)),
            Style::default().fg(color),
        )
    } else {
        Span::raw(" ".repeat(gap))
    }
}

/// Returns (filled_str, empty_str). Each string is exactly `n` terminal columns.
/// Uses continuous ⅛-block glyphs for smooth sub-cell resolution.
/// Shared with the command rail so the gain bars read identically there.
pub(crate) fn gain_bar(gain: u32, max_gain: u32, n: usize) -> (String, String) {
    crate::ui::widgets::charts::eighth_block_bar(gain, max_gain, n)
}

/// Power-of-ten exponent (in Hz) of the digit the current tuning step acts on:
/// 1 kHz→3, 10 kHz→4, 100 kHz→5, 1 MHz→6, 10 MHz→7. Coarse-but-non-decade steps
/// (5 kHz, 25 kHz, 500 kHz, 5 MHz) collapse onto their leading digit's place,
/// which is the digit a user reads as "the one I'm moving".
fn step_place_exp(step_hz: u64) -> u32 {
    let mut e = 0u32;
    let mut s = step_hz.max(1);
    while s >= 10 {
        s /= 10;
        e += 1;
    }
    e
}

/// Segmented VFO frequency readout: the MHz value rendered digit-by-digit with a
/// thin gap between every character, and the single digit the current tuning step
/// moves underlined + brightened — so you can see at a glance which place `← →`
/// will change. The decimal point is dimmed. Returns the spans; width varies with
/// The MHz readout string the VFO renders, e.g. `"145.500"`. Shared so the big
/// freq-hero formats the frequency identically and the active-digit index lines
/// up with the same characters.
pub(crate) fn vfo_string(freq_hz: u64) -> String {
    format!("{:.3}", freq_hz as f64 / 1_000_000.0)
}

/// Char index (in [`vfo_string`]) of the digit the current tuning step moves, if
/// that digit is on screen. Shared by the small VFO and the big freq-hero so they
/// underline / colour the *same* digit.
pub(crate) fn active_digit_idx(freq_hz: u64, step_hz: u64) -> Option<usize> {
    let s = vfo_string(freq_hz);
    let dot_pos = s.find('.').unwrap_or(s.len());
    let exp = step_place_exp(step_hz);
    if exp >= 6 {
        let from_right = (exp - 6) as usize; // 0 = ones-MHz digit (just left of '.')
        (from_right < dot_pos).then(|| dot_pos - 1 - from_right)
    } else if (3..=5).contains(&exp) {
        Some(dot_pos + 1 + (5 - exp) as usize) // 5→.1xx, 4→..1x, 3→...1
    } else {
        None
    }
}

/// the number of MHz digits (the caller measures it for layout). Shared with the
/// command rail's freq-hero so the segmented readout + active-digit cue match.
pub(crate) fn vfo_spans(
    freq_hz: u64,
    step_hz: u64,
    digit: ratatui::style::Color,
    dot: ratatui::style::Color,
    active: ratatui::style::Color,
) -> Vec<Span<'static>> {
    let s = vfo_string(freq_hz); // e.g. "145.500"
    let active_idx = active_digit_idx(freq_hz, step_hz);

    let chars: Vec<char> = s.chars().collect();
    let mut spans = Vec::with_capacity(chars.len() * 2);
    for (i, c) in chars.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if Some(i) == active_idx {
            Style::default()
                .fg(active)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else if *c == '.' {
            Style::default().fg(dot)
        } else {
            Style::default().fg(digit).add_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(c.to_string(), style));
    }
    spans
}

/// Returns the number of space characters needed between the fw-version field
/// and the right-aligned "AMP … USB …" section in the top band.
/// All length arguments are in terminal columns (chars, not bytes).
fn top_band_gap(
    board_name_len: usize,
    badge_len: usize,
    fw_value_len: usize,
    amp_val_len: usize,
    usb_val_len: usize,
    inner_width: u16,
) -> usize {
    // left side: " " + " DeviceName " + "  " + " BADGE " + "  " + "hackrf fw " + fw_val
    let left = 1 + (2 + board_name_len) + 2 + badge_len + 2 + 10 + fw_value_len;
    // right side: "AMP "(4) + amp_val + "  ·  "(5) + "USB "(4) + usb_val + "  "(2)
    let right = 4 + amp_val_len + 5 + 4 + usb_val_len + 2;
    (inner_width as usize).saturating_sub(left + right)
}

fn top_band_line(state: &SdrMetrics, theme: &crate::Theme, inner_width: u16) -> Line<'static> {
    use ratatui::style::Color;

    // --- Status badge ---
    // RX uses a breathing dot; IDLE/OBSERVER are steady. Every variant is 6
    // columns so `top_band_gap` stays valid.
    let (badge_text, badge_bg, badge_fg): (String, Color, Color) = if state.observer.active {
        (
            " ◈ OBSERVER ".to_string(),
            theme.observer,
            Color::Rgb(4, 6, 15),
        )
    } else if state.radio.hw_streaming {
        (
            format!(" {} RX ", rx_pulse_glyph()),
            theme.status_ok,
            Color::Rgb(3, 15, 6),
        )
    } else {
        (
            " ○ IDLE ".to_string(),
            theme.status_warn,
            Color::Rgb(10, 7, 0),
        )
    };
    let badge_len = badge_text.chars().count();

    // --- Firmware version + label ---
    // Mayhem nightly: "n_XXXXXX"; Mayhem release: "vX.Y.Z" → label as "mayhem fw "
    // Standard HackRF firmware ("2024.02.1", "git-...") → label as "hackrf fw "
    // Both labels are exactly 10 chars so top_band_gap stays valid.
    // Firmware field. RTL-SDR has no on-device firmware (it's host-driven by
    // librtlsdr), so it gets a neutral label instead of "hackrf fw" — including
    // in observer mode. All labels are exactly 10 columns so top_band_gap stays
    // valid.
    let (fw_val, fw_label): (std::sync::Arc<str>, &str) = if state.caps.gain.is_single() {
        let v = if state.observer.active {
            "—"
        } else {
            "librtlsdr"
        };
        (std::sync::Arc::from(v), "rtl-sdr   ")
    } else if state.observer.active {
        (std::sync::Arc::from("—"), "hackrf fw ")
    } else {
        let is_mayhem = state.system.fw_version.starts_with("n_")
            || (state.system.fw_version.starts_with('v')
                && state
                    .system
                    .fw_version
                    .chars()
                    .nth(1)
                    .is_some_and(|c| c.is_ascii_digit()));
        let label = if is_mayhem {
            "mayhem fw "
        } else {
            "hackrf fw "
        };
        (state.system.fw_version.clone(), label)
    };
    let fw_color = if state.observer.active {
        theme.label
    } else {
        theme.value
    };
    let fw_len = fw_val.chars().count();

    // --- AMP value (always 3 terminal columns) ---
    let (amp_val, amp_color) = if state.observer.active {
        ("—  ".to_string(), theme.label)
    } else if state.radio.amp_enabled {
        ("ON ".to_string(), theme.value_hi)
    } else {
        ("OFF".to_string(), theme.label)
    };

    // --- USB value (always 9 terminal columns) ---
    let (usb_val, usb_color) = if state.radio.hw_streaming && state.radio.current_throughput_bps > 0
    {
        let mb = state.radio.current_throughput_bps as f64 / 1_000_000.0;
        (format!("{:4.1} MB/s", mb), theme.value)
    } else {
        ("—        ".to_string(), theme.label) // 1 + 8 spaces = 9 chars
    };

    // --- Gap ---
    let board_len = state.system.board_name.chars().count();
    let gap = top_band_gap(
        board_len,
        badge_len,
        fw_len,
        amp_val.chars().count(),
        usb_val.chars().count(),
        inner_width,
    );

    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!(" {} ", state.system.board_name),
            Style::default()
                .fg(theme.value_hi)
                .bg(Color::Rgb(20, 25, 38))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            badge_text,
            Style::default()
                .fg(badge_fg)
                .bg(badge_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(fw_label, Style::default().fg(theme.label)),
        Span::styled(fw_val.to_string(), Style::default().fg(fw_color)),
        leader(gap, theme.border_dim),
        // HackRF's RF amp or RTL-SDR's tuner AGC — both 3-char labels, so the
        // "{label} " field stays 4 columns and top_band_gap remains valid.
        Span::styled(
            format!("{} ", state.caps.gain.boost_label()),
            Style::default().fg(theme.label),
        ),
        Span::styled(amp_val, Style::default().fg(amp_color)),
        Span::raw("  ·  "),
        Span::styled("USB ", Style::default().fg(theme.label)),
        Span::styled(usb_val, Style::default().fg(usb_color)),
        Span::raw("  "),
    ])
}

/// Compact frequency label for the tuning-range end-caps: "1M", "145M", "1.8G",
/// "6G", "24M", "300k". Whole GHz values drop the decimal ("6G", not "6.0G").
fn fmt_freq_compact(hz: u64) -> String {
    if hz >= 1_000_000_000 {
        let g = hz as f64 / 1e9;
        if (g - g.round()).abs() < 0.05 {
            format!("{:.0}G", g)
        } else {
            format!("{:.1}G", g)
        }
    } else if hz >= 1_000_000 {
        format!("{:.0}M", hz as f64 / 1e6)
    } else if hz >= 1_000 {
        format!("{}k", hz / 1_000)
    } else {
        format!("{hz}")
    }
}

/// Logarithmic position (0..1) of `freq` within the tunable range `[min,max]`.
/// Log, because a receiver's span is enormous (MHz…GHz) — a linear bar would
/// Perceptual exponent for the tuning-dial position. A pure-log axis pushes the
/// low end too far right (1 MHz…120 MHz already eats ~55 % of the bar, then the
/// whole GHz range crawls in the remaining 45 %). A pure-linear axis does the
/// opposite — it crushes everything below ~1 GHz into the first columns. `0.4`
/// is the middle ground: it spreads VHF/UHF readably while still moving the
/// needle at a steady pace up into the GHz range.
const DIAL_GAMMA: f64 = 0.4;

/// Position (0..1) of `freq` within the tunable range `[min,max]` on a `γ`-power
/// axis (see [`DIAL_GAMMA`]). Clamped to the range.
fn range_frac(freq: u64, min: u64, max: u64) -> f64 {
    let lo = (min.max(1) as f64).powf(DIAL_GAMMA);
    let hi = (max.max(1) as f64).powf(DIAL_GAMMA);
    if hi <= lo {
        return 0.0;
    }
    let f = (freq as f64)
        .clamp(min as f64, max as f64)
        .max(1.0)
        .powf(DIAL_GAMMA);
    ((f - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// A plain ruled `├───────┤` rule — fallback for terminals too narrow to fit the
/// live tuning strip.
fn plain_separator(theme: &crate::Theme, outer_width: u16) -> Line<'static> {
    let fill = (outer_width as usize).saturating_sub(2);
    Line::from(vec![
        Span::styled("├", Style::default().fg(theme.border_dim)),
        Span::styled("─".repeat(fill), Style::default().fg(theme.border_default)),
        Span::styled("┤", Style::default().fg(theme.border_dim)),
    ])
}

/// The header's central rule, repurposed from a static "FREQUENCY" label into a
/// live tuning dial: a `γ`-power position bar across the device's whole tunable
/// range, end-capped by the range limits, with a lit `━` rail behind a `◆` needle
/// at the current frequency — and the **band name riding the needle** (e.g.
/// `◆╴2m╶`), so the band you're in sits exactly where the eye lands. `outer_width`
/// is the FULL panel width; rendered at the outer Rect so `├`/`┤` overwrite `│`.
fn band_strip_line(state: &SdrMetrics, theme: &crate::Theme, outer_width: u16) -> Line<'static> {
    compose_band_strip(
        state.radio.frequency,
        state.caps.freq_min_hz,
        state.caps.freq_max_hz,
        theme,
        outer_width,
    )
}

/// Pure core of [`band_strip_line`] — takes the tuned frequency and tunable range
/// directly so it can be unit-tested without a full `SdrMetrics`.
fn compose_band_strip(
    freq: u64,
    fmin: u64,
    fmax: u64,
    theme: &crate::Theme,
    outer_width: u16,
) -> Line<'static> {
    let frac = range_frac(freq, fmin, fmax);
    let lo_lbl = fmt_freq_compact(fmin);
    let hi_lbl = fmt_freq_compact(fmax);

    // Fixed chrome around the track:  ├ ─ ␠ LO ␠ <track> ␠ HI ␠ ─ ┤
    let left_w = 1 + 1 + 1 + lo_lbl.chars().count() + 1;
    let right_w = 1 + hi_lbl.chars().count() + 1 + 1 + 1;
    let track_w = (outer_width as usize).saturating_sub(left_w + right_w);

    if track_w < 8 {
        return plain_separator(theme, outer_width);
    }

    let marker_col = ((frac * (track_w - 1) as f64).round() as usize).min(track_w - 1);

    let dim = theme.border_dim;
    let track = theme.border_default;

    let mut spans = vec![
        Span::styled("├", Style::default().fg(dim)),
        Span::styled("─", Style::default().fg(track)),
        Span::raw(" "),
        Span::styled(lo_lbl, Style::default().fg(theme.label)),
        Span::raw(" "),
    ];
    spans.extend(rail_spans(track_w, marker_col, band_at(freq), theme));
    spans.extend([
        Span::raw(" "),
        Span::styled(hi_lbl, Style::default().fg(theme.label)),
        Span::raw(" "),
        Span::styled("─", Style::default().fg(track)),
        Span::styled("┤", Style::default().fg(dim)),
    ]);
    Line::from(spans)
}

/// The lit-rail dial itself, exactly `track_w` columns: a bright heavy `━` rule up
/// to the `◆` needle, then a faint dashed `┈` rule, with the band-name callout
/// `╴NAME╶` placed against the needle (to its right if it fits, else its left).
/// Position is double-encoded — brightness *and* line weight — so it reads at a
/// glance, and the band label sits right at the needle.
fn rail_spans(
    track_w: usize,
    marker_col: usize,
    band: Option<&'static str>,
    theme: &crate::Theme,
) -> Vec<Span<'static>> {
    let heavy = Style::default().fg(theme.border_accent);
    let faint = Style::default().fg(theme.border_dim);
    let mark = Style::default()
        .fg(theme.value_hi)
        .add_modifier(Modifier::BOLD);
    let cap = Style::default().fg(theme.border_accent);
    let name = Style::default()
        .fg(theme.value_hi)
        .add_modifier(Modifier::BOLD);

    let callout: Vec<Span<'static>> = match band {
        Some(b) => vec![
            Span::styled("╴", cap),
            Span::styled(b, name),
            Span::styled("╶", cap),
        ],
        None => Vec::new(),
    };
    let cw: usize = callout.iter().map(|s| s.width()).sum();

    let heavy_run = |n: usize| Span::styled("━".repeat(n), heavy);
    let faint_run = |n: usize| Span::styled("┈".repeat(n), faint);
    let needle = || Span::styled("◆", mark);

    let mut spans = Vec::with_capacity(6);
    if cw > 0 && marker_col + 1 + cw <= track_w {
        // Callout to the RIGHT of the needle.
        spans.push(heavy_run(marker_col));
        spans.push(needle());
        spans.extend(callout);
        spans.push(faint_run(track_w - marker_col - 1 - cw));
    } else if cw > 0 && marker_col >= cw {
        // No room on the right — tuck the callout to the LEFT of the needle.
        spans.push(heavy_run(marker_col - cw));
        spans.extend(callout);
        spans.push(needle());
        spans.push(faint_run(track_w - marker_col - 1));
    } else {
        // Between bands (or no room): just the lit rail + needle.
        spans.push(heavy_run(marker_col));
        spans.push(needle());
        spans.push(faint_run(track_w - marker_col - 1));
    }
    spans
}

/// Frequency · sample-rate on the left, gain bars right-aligned. Left block
/// (freq + SR): 31 chars. Right block: 42 chars — either HackRF's LNA + VGA, or a
/// single-tuner stage (RTL-SDR) with the second-stage region blanked to the same
/// width so the gap math and right-alignment hold for both.
fn bottom_band_line(state: &SdrMetrics, theme: &crate::Theme, inner_width: u16) -> Line<'static> {
    let active = state.radio.hw_streaming && !state.observer.active;
    let gm = &state.caps.gain;

    // Sample rate: right-padded to 4 chars
    let sr_str = format!("{:4.1}", state.radio.config_sample_rate / 1_000_000.0);

    let freq_color = if state.observer.active {
        theme.label
    } else {
        theme.border_accent
    };
    let val_color = if active { theme.value } else { theme.label };
    let dim = theme.border_dim;

    // ⅛-block gain bar that matches the command rail: a meaning gradient while
    // streaming (LNA green→yellow, VGA cyan→orange), flat dim when idle.
    let gain_bar_spans = |gain: u32, max: u32, lo: Color, hi: Color| -> Vec<Span<'static>> {
        if active {
            gain_bar_colored(gain, max, 8, lo, hi, dim)
        } else {
            let (f, e) = gain_bar(gain, max, 8);
            vec![
                Span::styled(f, Style::default().fg(theme.label)),
                Span::styled(e, Style::default().fg(dim)),
            ]
        }
    };

    // Left block: segmented VFO readout + unit + sample-rate. Its width varies
    // with the number of MHz digits and the active-digit underline, so it is
    // measured (below) rather than assumed, and the trailing gap fills the rest.
    let mut left_spans = vec![Span::raw("  ")];
    left_spans.extend(vfo_spans(
        state.radio.frequency,
        state.spectrum.step_hz,
        freq_color,
        theme.label,
        theme.value_hi,
    ));
    left_spans.extend([
        Span::raw(" "),
        Span::styled("MHz", Style::default().fg(theme.label)),
        Span::raw("    "),
        Span::styled("SR ", Style::default().fg(theme.label)),
        Span::styled(sr_str, Style::default().fg(val_color)),
        Span::styled(" Msps", Style::default().fg(theme.label)),
    ]);
    let left_w: usize = left_spans.iter().map(|s| s.width()).sum();

    // right: primary "LNA/TUN "(4) + bar(8) + " "(1) + val(2) + " dB"(3) + "    "(4)  = 22
    //      + second stage "VGA "(4) + bar(8) + " "(1) + val(2) + " dB"(3) + "  "(2)   = 20  (blank on RTL)
    let right = 22 + 20;
    let gap = (inner_width as usize).saturating_sub(left_w + right);

    // Primary stage: HackRF LNA / RTL-SDR tuner — green → yellow gradient.
    let p_str = format!("{:2}", state.radio.lna_gain);
    let p_label = if gm.is_single() { "TUN " } else { "LNA " };

    let mut spans = left_spans;
    spans.push(leader(gap, theme.border_dim));
    spans.push(Span::styled(p_label, Style::default().fg(theme.label)));
    spans.extend(gain_bar_spans(
        state.radio.lna_gain,
        gm.primary_max_db(),
        theme.status_ok,
        theme.value_hi,
    ));
    spans.extend([
        Span::raw(" "),
        Span::styled(p_str, Style::default().fg(val_color)),
        Span::styled(" dB", Style::default().fg(theme.label)),
        Span::raw("    "),
    ]);

    if gm.has_second_stage() {
        // Secondary stage (HackRF VGA only) — cyan → orange gradient.
        let vga_str = format!("{:2}", state.radio.vga_gain);
        spans.push(Span::styled("VGA ", Style::default().fg(theme.label)));
        spans.extend(gain_bar_spans(
            state.radio.vga_gain,
            62,
            theme.border_accent,
            theme.status_warn,
        ));
        spans.extend([
            Span::raw(" "),
            Span::styled(vga_str, Style::default().fg(val_color)),
            Span::styled(" dB", Style::default().fg(theme.label)),
            Span::raw("  "),
        ]);
    } else {
        // Single-tuner device: blank the 20-col second-stage region to keep width.
        spans.push(Span::raw(" ".repeat(20)));
    }

    Line::from(spans)
}

impl Panel for HeaderPanel {
    fn name(&self) -> &'static str {
        "header"
    }
    fn min_size(&self) -> (u16, u16) {
        (60, 5)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::deck("Radio")
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        // inner.height == 3 when the panel is 5 rows tall. Row positions:
        //   inner.y     → top band
        //   inner.y + 1 → the band strip, drawn at the *outer* width so its ├ and ┤
        //                 end caps land on the side borders and tie into the frame
        //   inner.y + 2 → bottom band
        if inner.height < 3 {
            return;
        }
        let outer = frame::outer_of(inner);
        let top_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        let sep_area = Rect {
            x: outer.x,
            y: inner.y + 1,
            width: outer.width,
            height: 1,
        };
        let bot_area = Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: 1,
        };

        f.render_widget(
            Paragraph::new(top_band_line(state, theme, inner.width)),
            top_area,
        );
        f.render_widget(
            Paragraph::new(band_strip_line(state, theme, outer.width)),
            sep_area,
        );
        f.render_widget(
            Paragraph::new(bottom_band_line(state, theme, inner.width)),
            bot_area,
        );
    }
}

/// A two-row header for the Command Rail layout: just the device-status band and
/// the γ-power tuning dial — the frequency readout and gain bars move into the
/// rail, so the header stays out of the way ("where am I in the range" context
/// only). Reuses the full header's `top_band_line` + `band_strip_line`, so the
/// two stay visually identical. Height 4 (2 inner rows).
pub struct SlimHeaderPanel;

impl Panel for SlimHeaderPanel {
    fn name(&self) -> &'static str {
        "header_slim"
    }
    fn min_size(&self) -> (u16, u16) {
        (60, 4)
    }
    fn preferred_height(&self, _w: u16, _s: &SdrMetrics) -> u16 {
        4
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::deck("Radio")
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        // inner.height == 2 when the panel is 4 rows tall:
        //   inner.y     → device-status band
        //   inner.y + 1 → tuning dial, at outer width so its ├/┤ overwrite the │
        if inner.height < 2 {
            return;
        }
        let outer = frame::outer_of(inner);
        let top_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        let dial_area = Rect {
            x: outer.x,
            y: inner.y + 1,
            width: outer.width,
            height: 1,
        };

        f.render_widget(
            Paragraph::new(top_band_line(state, theme, inner.width)),
            top_area,
        );
        f.render_widget(
            Paragraph::new(band_strip_line(state, theme, outer.width)),
            dial_area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn gain_bar_zero_gain_all_empty() {
        let (filled, empty) = gain_bar(0, 40, 8);
        assert_eq!(filled, "");
        assert_eq!(empty, " ".repeat(8));
    }

    #[test]
    fn gain_bar_full_gain_all_filled() {
        let (filled, empty) = gain_bar(40, 40, 8);
        assert_eq!(filled, "█".repeat(8));
        assert_eq!(empty, "");
    }

    #[test]
    fn step_place_exp_maps_steps_to_digit_place() {
        // decade steps land exactly on their digit
        assert_eq!(step_place_exp(1_000), 3); // 1 kHz
        assert_eq!(step_place_exp(10_000), 4); // 10 kHz
        assert_eq!(step_place_exp(100_000), 5); // 100 kHz
        assert_eq!(step_place_exp(1_000_000), 6); // 1 MHz
        assert_eq!(step_place_exp(10_000_000), 7); // 10 MHz
                                                   // non-decade steps collapse onto their leading digit's place
        assert_eq!(step_place_exp(5_000), 3);
        assert_eq!(step_place_exp(25_000), 4);
        assert_eq!(step_place_exp(500_000), 5);
        assert_eq!(step_place_exp(5_000_000), 6);
    }

    #[test]
    fn vfo_underlines_the_active_digit() {
        let t = Theme::sdr();
        // 145.500 MHz, 10 kHz step → the 10-kHz digit is the first decimal-2 ('0'
        // in ".50"). Exactly one span carries UNDERLINED.
        let spans = vfo_spans(145_500_000, 10_000, t.border_accent, t.label, t.value_hi);
        let underlined: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(underlined.len(), 1, "exactly one active digit");
        // "145.500": frac index 1 (5→exp5,4→exp4) → the '0' after the '5'
        assert_eq!(underlined[0], "0");
        // active digit is brightened, not the plain accent
        let act = spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
            .unwrap();
        assert_eq!(act.style.fg, Some(t.value_hi));
    }

    #[test]
    fn vfo_step_above_screen_underlines_nothing() {
        let t = Theme::sdr();
        // 5 MHz, 10 MHz step → tens-of-MHz digit, which doesn't exist → no underline
        let spans = vfo_spans(5_000_000, 10_000_000, t.border_accent, t.label, t.value_hi);
        let any = spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(!any, "active digit off-screen → nothing underlined");
    }

    #[test]
    fn gain_bar_half_gain() {
        let (filled, empty) = gain_bar(20, 40, 8);
        assert_eq!(filled.chars().count(), 4);
        assert_eq!(empty.chars().count(), 4);
    }

    #[test]
    fn gain_bar_total_always_equals_width() {
        for gain in [0u32, 1, 16, 20, 40] {
            let (f, e) = gain_bar(gain, 40, 8);
            assert_eq!(
                f.chars().count() + e.chars().count(),
                8,
                "gain={gain}: filled({}) + empty({}) != 8",
                f.chars().count(),
                e.chars().count()
            );
        }
    }

    #[test]
    fn top_band_gap_rx_state() {
        // HackRF One (len=10), badge " ● RX " (len=6), fw "2024.02.1" (len=9), inner=78
        // amp_val "ON " (3), usb_val "10.0 MB/s" (9)
        assert_eq!(top_band_gap(10, 6, 9, 3, 9, 78), 9);
    }

    #[test]
    fn top_band_gap_idle_state() {
        // badge " ○ IDLE " is 2 chars wider than RX → gap shrinks by 2
        assert_eq!(top_band_gap(10, 8, 9, 3, 9, 78), 7);
    }

    #[test]
    fn top_band_gap_observer_state() {
        // badge " ◈ OBSERVER " (len=12), fw "—" (len=1)
        assert_eq!(top_band_gap(10, 12, 1, 3, 9, 78), 11);
    }

    #[test]
    fn fmt_freq_compact_units() {
        assert_eq!(fmt_freq_compact(1_000_000), "1M");
        assert_eq!(fmt_freq_compact(145_000_000), "145M");
        assert_eq!(fmt_freq_compact(24_000_000), "24M");
        assert_eq!(fmt_freq_compact(6_000_000_000), "6G"); // whole GHz drops decimal
        assert_eq!(fmt_freq_compact(1_766_000_000), "1.8G");
        assert_eq!(fmt_freq_compact(300_000), "300k");
    }

    #[test]
    fn range_frac_endpoints_monotonic_and_clamp() {
        let (lo, hi) = (1_000_000u64, 6_000_000_000u64);
        assert!((range_frac(lo, lo, hi) - 0.0).abs() < 1e-9, "min → 0");
        assert!((range_frac(hi, lo, hi) - 1.0).abs() < 1e-9, "max → 1");
        // The whole point of the γ-power axis: the low end no longer eats half the
        // bar. 120 MHz sat at ~0.55 on a log axis; here it must be well under a
        // quarter, and the GHz range gets the room instead.
        assert!(
            range_frac(120_000_000, lo, hi) < 0.25,
            "120 MHz should sit in the lower quarter, got {}",
            range_frac(120_000_000, lo, hi)
        );
        assert!(
            range_frac(1_000_000_000, lo, hi) > 0.40,
            "1 GHz should be past the low band, got {}",
            range_frac(1_000_000_000, lo, hi)
        );
        // Strictly increasing with frequency.
        assert!(range_frac(100_000_000, lo, hi) < range_frac(1_000_000_000, lo, hi));
        assert!(range_frac(1_000_000_000, lo, hi) < range_frac(3_000_000_000, lo, hi));
        // out-of-range clamps to the ends
        assert_eq!(range_frac(500_000, lo, hi), 0.0);
        assert_eq!(range_frac(9_000_000_000, lo, hi), 1.0);
    }

    #[test]
    fn rail_spans_always_track_width() {
        // The rail must be exactly track_w columns for every marker position and
        // both with/without a band callout, so the outer width math holds.
        let t = Theme::sdr();
        for track_w in [8usize, 20, 40, 67] {
            for marker in [0usize, 1, track_w / 2, track_w - 2, track_w - 1] {
                for band in [None, Some("2m"), Some("ISM433")] {
                    let w: usize = rail_spans(track_w, marker, band, &t)
                        .iter()
                        .map(|s| s.width())
                        .sum();
                    assert_eq!(
                        w, track_w,
                        "track_w={track_w} marker={marker} band={band:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn band_strip_total_width_matches_outer() {
        // The composed strip must be exactly `outer_width` columns so the ├/┤ caps
        // land on the border and nothing is truncated or padded. Exercised across
        // an in-band frequency (named tab) and an out-of-band one (% tab).
        let t = Theme::sdr();
        for outer in [60u16, 78, 120, 200] {
            for (lbl, line) in [
                (
                    "named",
                    compose_band_strip(145_500_000, 1_000_000, 6_000_000_000, &t, outer),
                ),
                (
                    "percent",
                    compose_band_strip(200_000_000, 1_000_000, 6_000_000_000, &t, outer),
                ),
            ] {
                let w: usize = line.spans.iter().map(|s| s.width()).sum();
                assert_eq!(w, outer as usize, "{lbl} strip width at outer={outer}");
                assert_eq!(line.spans.first().unwrap().content.as_ref(), "├");
                assert_eq!(line.spans.last().unwrap().content.as_ref(), "┤");
            }
        }
    }
}
