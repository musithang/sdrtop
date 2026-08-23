//! Lab "instrument mode" chrome — the two thin bars that wrap every measurement
//! lab (`[5]`–`[9]`):
//!
//! - [`LabBannerPanel`] (top): `LAB · RF CHAIN [6]   REF —   AVG OFF   CAL —   MKR 2        ▶ LIVE`
//! - [`LabMarkerPanel`] (bottom): `MKR1 92.800 MHz -19.1 dBFS   MKR2 …   Δ …        [hints]`
//!
//! Both are borderless single-line bars (text row + a dim hairline) laid into the
//! lab presets' Top/Bottom slots like the footer — no engine changes. They read
//! the measurement flags from [`LabState`](crate::state::LabState) and the marker
//! list from `SpectrumState` (not duplicated). Every field is width-aware:
//! lower-priority fields drop out whole rather than clip mid-word.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::{SdrMetrics, SpectrumMarker};
use crate::ui::rf_calc::{cascade, estimate_mds_dbm, staging_verdict, system_nf_db};
use crate::ui::panel::{FrameStyle, Panel, PanelChrome};

/// Map an active preset name to its lab banner label and the number key that
/// selects it (the *current* key map, which we keep — see the implementation
/// plan). `None` for any non-lab preset, so the chrome bars no-op if they ever
/// render outside a lab.
pub fn lab_label(preset: &str) -> Option<(&'static str, char)> {
    match preset {
        "lab_iq"     => Some(("I/Q QUALITY", '5')),
        "lab_rf"     => Some(("RF CHAIN",    '6')),
        "lab_timing" => Some(("HOST TIMING", '7')),
        "lab_signal" => Some(("SIGNAL",      '8')),
        "lab_sweep"  => Some(("SWEEP",       '9')),
        _ => None,
    }
}

/// Precise marker-readout frequency: `92.800 MHz` / `433.920 MHz` / `1.234500 GHz`.
fn fmt_freq_mhz(hz: u64) -> String {
    if hz >= 1_000_000_000 {
        format!("{:.6} GHz", hz as f64 / 1e9)
    } else {
        format!("{:.3} MHz", hz as f64 / 1e6)
    }
}

/// `Δ` readout between two markers: frequency span + (optional) level difference,
/// e.g. `5.400 MHz 12.3 dB` or just `5.400 MHz` when a level is unavailable.
fn fmt_delta(df_hz: u64, dl_db: Option<f32>) -> String {
    let f = if df_hz >= 1_000_000_000 {
        format!("{:.6} GHz", df_hz as f64 / 1e9)
    } else {
        format!("{:.3} MHz", df_hz as f64 / 1e6)
    };
    match dl_db {
        Some(d) => format!("{f} {:.1} dB", d.abs()),
        None    => f,
    }
}

/// dBFS level at `freq_hz`, read from the latest FFT frame's bins. `None` if there
/// is no frame yet or the frequency is outside the captured span.
fn level_at_freq(state: &SdrMetrics, freq_hz: u64) -> Option<f32> {
    let fr = state.waterfall.last_fft.as_ref()?;
    let n = fr.bins_dbfs.len();
    if n == 0 { return None; }
    let left = fr.center_freq_hz as f64 - fr.sample_rate / 2.0;
    let frac = (freq_hz as f64 - left) / fr.sample_rate;
    if !(0.0..=1.0).contains(&frac) { return None; }
    let idx = (frac * (n - 1) as f64).round() as usize;
    fr.bins_dbfs.get(idx.min(n - 1)).copied()
}

/// Display width (columns) of a span run — every glyph we use here is single-width.
fn span_w(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

/// A full-width hairline rule in `color`.
fn hairline(iw: usize, color: ratatui::style::Color) -> Line<'static> {
    Line::from(Span::styled("\u{2500}".repeat(iw), Style::default().fg(color)))
}

// ── Banner (top bar) ────────────────────────────────────────────────────────

/// The receive-chain flow string for the Lab RF banner: `ANT▸LNA▸MIX▸VGA▸ADC`,
/// with `AMP` inserted when the front-end amp is on, collapsing to `ANT▸TUNER▸ADC`
/// on a single-tuner radio (no cascade).
fn rf_chain_str(friis_applicable: bool, amp_enabled: bool) -> String {
    if !friis_applicable { return "ANT\u{25B8}TUNER\u{25B8}ADC".to_string(); }
    let mut s = String::from("ANT\u{25B8}");
    if amp_enabled { s.push_str("AMP\u{25B8}"); }
    s.push_str("LNA\u{25B8}MIX\u{25B8}VGA\u{25B8}ADC");
    s
}

/// Lab RF banner middle fields: `CHAIN … · NF … · MDS … · SNR …` (modeled NF/MDS
/// from the live cascade). On a single-tuner radio only the honest CHAIN + SNR show.
fn rf_banner_fields(state: &SdrMetrics) -> Vec<(&'static str, String)> {
    let chain = rf_chain_str(state.caps.friis_applicable, state.radio.amp_enabled);
    let snr   = format!("{:.0} dB", state.signal.peak_to_nf_db);
    if !state.caps.friis_applicable {
        return vec![("CHAIN", chain), ("SNR", snr)];
    }
    let nf  = system_nf_db(&cascade(state.radio.amp_enabled, state.radio.lna_gain, state.radio.vga_gain));
    let mds = match estimate_mds_dbm(state.radio.bb_filter_hz, nf) {
        Some(m) => format!("{m:.0} dBm"),
        None    => "\u{2014}".to_string(),
    };
    vec![
        ("CHAIN", chain),
        ("NF",    format!("{nf:.1} dB")),
        ("MDS",   mds),
        ("SNR",   snr),
    ]
}

/// Lab timing banner middle fields: `CALLBACK … · JITTER … · DRIFT … · DEADLINE …`.
/// DEADLINE reads `✓ met` while no callback misses the budget, else the worst
/// slip as a percentage of the budget (the mockup's `⚠ 130%` / `⚠ 1050%`).
fn timing_banner_fields(t: &crate::state::TimingState) -> Vec<(&'static str, String)> {
    let callback = if t.cb_period_us == 0 {
        "\u{2014}".to_string()
    } else {
        crate::ui::widgets::timing_fmt::fmt_us(t.cb_period_us)
    };
    let deadline = if t.late_callbacks == 0 {
        "\u{2713} met".to_string()
    } else {
        let pct = if t.deadline_budget_us > 0 { t.dev_peak_us * 100 / t.deadline_budget_us } else { 0 };
        format!("\u{26a0} {pct}%")
    };
    vec![
        ("CALLBACK", callback),
        ("JITTER",   format!("\u{00b1}{} \u{00b5}s", t.cb_jitter_us)),
        ("DRIFT",    format!("{:+} ppm", t.cb_period_delta_ppm)),
        ("DEADLINE", deadline),
    ]
}

/// Lab signal banner middle fields: `MOD … · SNR … · CH PWR … · OBW …`. MOD
/// reads `—` while the classifier hasn't committed to a modulation (weak/no
/// carrier at centre) — never a fabricated label.
///
/// Every one of these comes from the latest FFT frame, so when that frame ages out
/// they stop being measurements and become the last thing that was true. Stop RX
/// and the characterization panel below correctly dashed its whole stack while this
/// line went on reading `SNR 47 dB · CH PWR -28.6 dBFS` two rows above it. The
/// `‖ FRZ` lamp at the far right was the only hint, and it is nowhere near the
/// numbers it applies to.
fn signal_banner_fields(state: &SdrMetrics) -> Vec<(&'static str, String)> {
    signal_fields(&state.signal, crate::ui::widgets::micro_common::fft_stale(state))
}

/// The banner fields as a pure function of what was measured and whether it is
/// still current — split out so the staleness rule can be tested without building a
/// whole `SdrMetrics`.
fn signal_fields(sig: &crate::state::SignalState, stale: bool) -> Vec<(&'static str, String)> {
    let dash = || "\u{2014}".to_string();
    if stale {
        return vec![("MOD", dash()), ("SNR", dash()), ("CH PWR", dash()), ("OBW", dash())];
    }
    vec![
        ("MOD",    sig.modulation.label().to_string()),
        ("SNR",    format!("{:.0} dB", sig.peak_to_nf_db)),
        ("CH PWR", if sig.channel_power_dbfs.is_finite() {
            format!("{:.1} dBFS", sig.channel_power_dbfs)
        } else {
            dash()
        }),
        ("OBW", if sig.occupied_bw_hz > 0 {
            crate::ui::widgets::micro_common::fmt_bw(sig.occupied_bw_hz)
        } else {
            dash()
        }),
    ]
}

fn banner_lines(state: &SdrMetrics, theme: &crate::Theme, iw: usize, focused: bool) -> Vec<Line<'static>> {
    let (label, num) = match lab_label(&state.ui.active_preset) {
        Some(x) => x,
        None    => return vec![Line::raw("")],
    };
    let dim  = Style::default().fg(theme.label);
    let bold = Style::default().fg(theme.label).add_modifier(Modifier::BOLD);
    let hi   = Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD);
    let val  = Style::default().fg(theme.value);

    // Left zone: "▸LAB · RF CHAIN [6]" (▸ when the banner holds focus).
    let lead = if focused {
        Span::styled("\u{25B8}", Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD))
    } else {
        Span::raw(" ")
    };
    let left: Vec<Span> = vec![
        lead,
        Span::styled("LAB", bold),
        Span::styled(" \u{00B7} ", dim),
        Span::styled(label, hi),
        Span::styled(" [", dim),
        Span::styled(num.to_string(), hi),
        Span::styled("]", dim),
    ];
    let lw = span_w(&left);

    // Right zone: live / freeze.
    let streaming = state.radio.hw_streaming && !state.observer.active;
    let mut right: Vec<Span> = if streaming {
        vec![Span::styled("\u{25B6} ", Style::default().fg(theme.status_ok)),
             Span::styled("LIVE ", Style::default().fg(theme.status_ok).add_modifier(Modifier::BOLD))]
    } else {
        vec![Span::styled("\u{2016} ", Style::default().fg(theme.status_warn)),
             Span::styled("FRZ ", Style::default().fg(theme.status_warn).add_modifier(Modifier::BOLD))]
    };
    let mut rw = span_w(&right);
    if iw <= lw + rw + 1 { right.clear(); rw = 0; } // too narrow for the right zone

    // Middle fields. Lab RF reads the RF cascade summary (CHAIN/NF/MDS/SNR) rather
    // than the generic spectrum REF/MKR/AVG/CAL set.
    let fields: Vec<(&'static str, String)> = if state.ui.active_preset == "lab_rf" {
        rf_banner_fields(state)
    } else if state.ui.active_preset == "lab_timing" {
        timing_banner_fields(&state.timing)
    } else if state.ui.active_preset == "lab_signal" {
        signal_banner_fields(state)
    } else {
        // In Lab IQ the two markers are the auto-tracked carrier + image, so MKR
        // reads "2" (and "pin" when frozen) rather than the placed-marker count.
        let mkr_str = if state.ui.active_preset == "lab_iq" {
            if state.lab.iq_marker_pin.is_some() { "2 pin".to_string() } else { "2".to_string() }
        } else {
            state.spectrum.markers.len().to_string()
        };
        // In Lab IQ the CAL field reflects the live I/Q auto-cal, not the spectrum
        // reference-trace cal used by the other labs.
        let cal_str = if state.ui.active_preset == "lab_iq" {
            if state.iq.cal.cal_applied      { "\u{2713}".to_string() }
            else if state.iq.cal.cal_pending { "\u{2026}".to_string() }
            else                             { "\u{2014}".to_string() }
        } else {
            state.lab.cal_label().to_string()
        };
        vec![
            ("REF", state.lab.ref_label()),
            ("MKR", mkr_str),
            ("AVG", state.lab.avg_label()),
            ("CAL", cal_str),
        ]
    };
    let mut mid: Vec<Span> = Vec::new();
    let mut mw = 0usize;
    for (lbl, value) in fields {
        let cand = vec![
            Span::raw("   "),
            Span::styled(lbl, dim),
            Span::raw(" "),
            Span::styled(value, val),
        ];
        let cw = span_w(&cand);
        if lw + mw + cw + rw + 1 <= iw { mid.extend(cand); mw += cw; }
    }

    let filler = iw.saturating_sub(lw + mw + rw).max(1);
    let mut spans = left;
    spans.extend(mid);
    spans.push(Span::raw(" ".repeat(filler)));
    spans.extend(right);

    let rule = if focused { theme.border_focused } else { theme.border_dim };
    vec![Line::from(spans), hairline(iw, rule)]
}

// ── Marker bar (bottom bar) ─────────────────────────────────────────────────

fn marker_spans(idx: usize, mk: Option<&SpectrumMarker>, state: &SdrMetrics,
                theme: &crate::Theme) -> Vec<Span<'static>> {
    let dim  = Style::default().fg(theme.label);
    let val  = Style::default().fg(theme.value);
    match mk {
        Some(m) => {
            let lvl = level_at_freq(state, m.freq_hz)
                .map(|d| format!("{d:.1} dBFS"))
                .unwrap_or_else(|| "\u{2014}".to_string());
            vec![
                Span::styled(format!("MKR{idx} "), dim),
                Span::styled(fmt_freq_mhz(m.freq_hz), val),
                Span::raw(" "),
                Span::styled(lvl, val),
            ]
        }
        None => vec![Span::styled(format!("MKR{idx} "), dim),
                     Span::styled("\u{2014}", Style::default().fg(theme.border_dim))],
    }
}

/// Right-side focus hints from the currently focused panel, appended if they fit.
fn append_focus_hints(state: &SdrMetrics, theme: &crate::Theme, iw: usize,
                      used: usize, spans: &mut Vec<Span<'static>>) {
    let key = Style::default().fg(theme.value_hi);
    let dim = Style::default().fg(theme.label);
    let hints: Vec<Span> = state.ui.focused_panel_bindings.iter()
        .flat_map(|(k, l)| vec![
            Span::styled(format!("[{k}] "), key),
            Span::styled(format!("{l}  "), dim),
        ]).collect();
    let hw = span_w(&hints);
    if !hints.is_empty() && used + hw + 2 <= iw {
        let filler = iw.saturating_sub(used + hw);
        spans.push(Span::raw(" ".repeat(filler)));
        spans.extend(hints);
    }
}

/// Lab IQ marker bar: MKR1 = IMAGE, MKR2 = CARRIER (mirror about the LO), and
/// `Δ image` = the measured suppression. Auto-tracks the strongest carrier live
/// unless pinned via `[M]` ([`LabState::iq_marker_pin`]).
fn iq_marker_lines(state: &SdrMetrics, theme: &crate::Theme, iw: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme.label);

    // carrier_image resolves pin → placed marker → auto and returns the carrier /
    // image *levels it actually measured*, so the bar reads the exact same numbers
    // as the scope (no second frequency→bin round-trip to drift on).
    let pin = state.lab.iq_marker_pin;
    let ci  = crate::ui::panels::lab::image_scope::carrier_image(state);

    let slot = |n: usize, name: &str, color: ratatui::style::Color, data: Option<(u64, f32)>| -> Vec<Span<'static>> {
        let mut v = vec![
            Span::styled(format!("MKR{n} \u{00b7} "), dim),
            Span::styled(name.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD)),
        ];
        match data {
            Some((f, lvl)) => {
                v.push(Span::raw(" "));
                v.push(Span::styled(fmt_freq_mhz(f), Style::default().fg(theme.value)));
                v.push(Span::raw("  "));
                v.push(Span::styled(format!("{lvl:.1} dBFS"), Style::default().fg(theme.value)));
            }
            None => {
                v.push(Span::raw(" "));
                v.push(Span::styled("\u{2014}", Style::default().fg(theme.border_dim)));
            }
        }
        v
    };

    let image_data   = ci.as_ref().map(|c| (c.image_hz, c.image_dbfs));
    let carrier_data = ci.as_ref().map(|c| (c.carrier_hz, c.carrier_dbfs));

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    spans.extend(slot(1, "IMAGE", theme.status_warn, image_data));
    let mut used = span_w(&spans);

    let try_add = |cand: Vec<Span<'static>>, used: &mut usize, spans: &mut Vec<Span<'static>>| {
        let cw = span_w(&cand);
        if *used + cw + 1 <= iw { spans.extend(cand); *used += cw; }
    };

    let mut c2 = vec![Span::raw("   ")];
    c2.extend(slot(2, "CARRIER", theme.value_hi, carrier_data));
    try_add(c2, &mut used, &mut spans);

    // Δ image: image level relative to the carrier (signed; normally negative),
    // colour-graded by the true suppression so an inverted pick is not flattered.
    if let Some(c) = &ci {
        let rel = c.image_dbfs - c.carrier_dbfs;
        let scol = if c.suppression_db >= 40.0 { theme.status_ok }
                   else if c.suppression_db >= 20.0 { theme.status_warn }
                   else { theme.status_crit };
        let rel_str = if rel <= 0.0 { format!("\u{2212}{:.1} dB", -rel) }
                      else          { format!("+{rel:.1} dB") };
        let cand = vec![
            Span::raw("   "),
            Span::styled("\u{0394} image ", dim),
            Span::styled(rel_str, Style::default().fg(scol).add_modifier(Modifier::BOLD)),
        ];
        try_add(cand, &mut used, &mut spans);
    }

    if pin.is_some() {
        let cand = vec![
            Span::raw("   "),
            Span::styled("\u{25cf} pinned", Style::default().fg(theme.status_warn)),
        ];
        try_add(cand, &mut used, &mut spans);
    }

    append_focus_hints(state, theme, iw, used, &mut spans);
    vec![hairline(iw, theme.border_dim), Line::from(spans)]
}

/// Lab RF marker bar: the ADC window read as a single line —
/// `CLIP 0 dBFS · PEAK −8 dBFS · Δ headroom +8 dB · NOISE −48 dBFS · SNR 40 dB` —
/// plus the focused panel's key hints. PEAK / headroom carry the staging severity
/// colour so a clipping or starved chain reads at a glance.
fn rf_marker_lines(state: &SdrMetrics, theme: &crate::Theme, iw: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme.label);
    let val = Style::default().fg(theme.value);

    let peak     = state.signal.adc_peak_dbfs;
    let snr      = state.signal.peak_to_nf_db;
    let noise    = peak - snr; // ADC noise floor, dBFS
    let headroom = -peak;
    let (_, sev) = staging_verdict(peak as f64);
    let peak_col = match sev {
        2 => theme.status_crit, 1 => theme.status_warn, _ => theme.status_ok,
    };

    // CLIP reference line is the constant 0 dBFS rail; always shown first.
    let mut spans: Vec<Span> = vec![
        Span::raw(" "),
        Span::styled("CLIP ", dim),
        Span::styled("0 dBFS", val),
    ];
    let mut used = span_w(&spans);

    let try_add = |cand: Vec<Span<'static>>, used: &mut usize, spans: &mut Vec<Span<'static>>| {
        let cw = span_w(&cand);
        if *used + cw + 1 <= iw { spans.extend(cand); *used += cw; }
    };

    try_add(vec![
        Span::raw("   "), Span::styled("PEAK ", dim),
        Span::styled(format!("{peak:.0} dBFS"), Style::default().fg(peak_col).add_modifier(Modifier::BOLD)),
    ], &mut used, &mut spans);

    try_add(vec![
        Span::raw("   "), Span::styled("\u{0394} headroom ", dim),
        Span::styled(format!("{headroom:+.0} dB"), Style::default().fg(peak_col)),
    ], &mut used, &mut spans);

    try_add(vec![
        Span::raw("   "), Span::styled("NOISE ", dim),
        Span::styled(format!("{noise:.0} dBFS"), val),
    ], &mut used, &mut spans);

    try_add(vec![
        Span::raw("   "), Span::styled("SNR ", dim),
        Span::styled(format!("{snr:.0} dB"), Style::default().fg(theme.value).add_modifier(Modifier::BOLD)),
    ], &mut used, &mut spans);

    append_focus_hints(state, theme, iw, used, &mut spans);
    vec![hairline(iw, theme.border_dim), Line::from(spans)]
}

/// `EXCELLENT` → `Excellent`: first letter upper, rest lower.
fn titlecase(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// Lab timing marker bar: the host-timing window as a single line —
/// `JITTER ±42 µs · DRIFT +12 ppm · LATE 0 · BUF 0% · QUALITY ✓ Excellent` —
/// plus the focused panel's key hints. LATE / BUF / QUALITY carry their severity
/// colour so a pressured pipeline reads at a glance.
fn timing_marker_lines(state: &SdrMetrics, theme: &crate::Theme, iw: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme.label);
    let val = Style::default().fg(theme.value);
    let t = &state.timing;

    let mut spans: Vec<Span> = vec![
        Span::raw(" "),
        Span::styled("JITTER ", dim),
        Span::styled(format!("\u{00b1}{} \u{00b5}s", t.cb_jitter_us), val),
    ];
    let mut used = span_w(&spans);

    let try_add = |cand: Vec<Span<'static>>, used: &mut usize, spans: &mut Vec<Span<'static>>| {
        let cw = span_w(&cand);
        if *used + cw + 1 <= iw { spans.extend(cand); *used += cw; }
    };

    try_add(vec![
        Span::raw("   "), Span::styled("DRIFT ", dim),
        crate::ui::widgets::timing_fmt::ppm_span(t.cb_period_delta_ppm, theme),
    ], &mut used, &mut spans);

    let late_col = if t.late_callbacks == 0 { theme.status_ok }
                   else if t.late_callbacks * 20 > t.late_window.max(1) { theme.status_crit }
                   else { theme.status_warn };
    try_add(vec![
        Span::raw("   "), Span::styled("LATE ", dim),
        Span::styled(format!("{}/{}", t.late_callbacks, t.late_window), Style::default().fg(late_col)),
    ], &mut used, &mut spans);

    try_add(vec![
        Span::raw("   "), Span::styled("BUF ", dim),
        Span::styled(format!("{:.0}%", state.iq.buf_fill_pct),
            Style::default().fg(crate::ui::widgets::micro_common::buf_color(state.iq.buf_fill_pct, theme))),
    ], &mut used, &mut spans);

    let q = t.timing_quality;
    let mark = if q.severity() == 0 { "\u{2713}" } else { "\u{26a0}" };
    try_add(vec![
        Span::raw("   "), Span::styled("QUALITY ", dim),
        Span::styled(format!("{mark} {}", titlecase(q.label())),
            Style::default().fg(crate::ui::widgets::timing_fmt::quality_color(q, theme)).add_modifier(Modifier::BOLD)),
    ], &mut used, &mut spans);

    append_focus_hints(state, theme, iw, used, &mut spans);
    vec![hairline(iw, theme.border_dim), Line::from(spans)]
}

/// Lab signal marker bar: `MKR1 … · MKR2 … · Δ … · OBW … · QUALITY …` — the
/// user-placed markers (same read as the generic bar), plus the occupied
/// bandwidth and verdict severity the left rail's card already shows. QUALITY
/// calls `signal_characterization::verdict` directly — one source of truth, so
/// the bottom bar and the card can never disagree (same precedent as Lab IQ's
/// marker bar reading `image_scope::carrier_image`).
fn signal_marker_lines(state: &SdrMetrics, theme: &crate::Theme, iw: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme.label);
    let val = Style::default().fg(theme.value);
    let sig = &state.signal;

    let m1 = state.spectrum.markers.first();
    let m2 = state.spectrum.markers.get(1);

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    spans.extend(marker_spans(1, m1, state, theme));
    let mut used = span_w(&spans);

    let try_add = |cand: Vec<Span<'static>>, used: &mut usize, spans: &mut Vec<Span<'static>>| {
        let cw = span_w(&cand);
        if *used + cw + 1 <= iw { spans.extend(cand); *used += cw; }
    };

    if m2.is_some() {
        let mut c = vec![Span::raw("   ")];
        c.extend(marker_spans(2, m2, state, theme));
        try_add(c, &mut used, &mut spans);
    }

    if let (Some(a), Some(b)) = (m1, m2) {
        let df = (b.freq_hz as i64 - a.freq_hz as i64).unsigned_abs();
        let dl = match (level_at_freq(state, a.freq_hz), level_at_freq(state, b.freq_hz)) {
            (Some(x), Some(y)) => Some(y - x),
            _ => None,
        };
        try_add(vec![
            Span::raw("   "),
            Span::styled("\u{0394} ", dim),
            Span::styled(fmt_delta(df, dl), Style::default().fg(theme.value).add_modifier(Modifier::BOLD)),
        ], &mut used, &mut spans);
    }

    // Same rule as the banner: an aged FFT frame has no occupancy and no verdict to
    // offer. A `QUALITY ✓ Clean` computed from frozen numbers is the worst of the
    // two, because it is a judgement rather than a reading.
    let stale = crate::ui::widgets::micro_common::fft_stale(state);

    let obw_str = if !stale && sig.occupied_bw_hz > 0 {
        crate::ui::widgets::micro_common::fmt_bw(sig.occupied_bw_hz)
    } else {
        "\u{2014}".to_string()
    };
    try_add(vec![
        Span::raw("   "), Span::styled("OBW ", dim),
        Span::styled(obw_str, val),
    ], &mut used, &mut spans);

    let quality = if stale {
        vec![Span::styled("\u{2014}".to_string(), Style::default().fg(theme.stale))]
    } else {
        let (level, ..) = crate::ui::panels::lab::signal_characterization::verdict(
            sig.modulation, sig.peak_to_nf_db, sig.acpr_lower_db, sig.acpr_upper_db, sig.occupied_bw_hz);
        let (mark, col) = match level {
            crate::ui::panels::lab::signal_characterization::VerdictLevel::Clean    => ("\u{2713}", theme.status_ok),
            crate::ui::panels::lab::signal_characterization::VerdictLevel::Caution  => ("\u{26a0}", theme.status_warn),
            crate::ui::panels::lab::signal_characterization::VerdictLevel::NoSignal => ("\u{25cb}", theme.stale),
        };
        vec![Span::styled(format!("{mark} {}", level.short_label()),
                          Style::default().fg(col).add_modifier(Modifier::BOLD))]
    };
    let mut q = vec![Span::raw("   "), Span::styled("QUALITY ", dim)];
    q.extend(quality);
    try_add(q, &mut used, &mut spans);

    append_focus_hints(state, theme, iw, used, &mut spans);
    vec![hairline(iw, theme.border_dim), Line::from(spans)]
}

fn marker_lines(state: &SdrMetrics, theme: &crate::Theme, iw: usize) -> Vec<Line<'static>> {
    if state.ui.active_preset == "lab_iq" {
        return iq_marker_lines(state, theme, iw);
    }
    if state.ui.active_preset == "lab_rf" {
        return rf_marker_lines(state, theme, iw);
    }
    if state.ui.active_preset == "lab_timing" {
        return timing_marker_lines(state, theme, iw);
    }
    if state.ui.active_preset == "lab_signal" {
        return signal_marker_lines(state, theme, iw);
    }
    let dim = Style::default().fg(theme.label);
    let key = Style::default().fg(theme.value_hi);

    let m1 = state.spectrum.markers.first();
    let m2 = state.spectrum.markers.get(1);

    // MKR1 always shown; the rest fill as room allows.
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    spans.extend(marker_spans(1, m1, state, theme));
    let mut used = span_w(&spans);

    let try_add = |cand: Vec<Span<'static>>, used: &mut usize, spans: &mut Vec<Span<'static>>| {
        let cw = span_w(&cand);
        if *used + cw + 1 <= iw { spans.extend(cand); *used += cw; }
    };

    if m2.is_some() {
        let mut c = vec![Span::raw("   ")];
        c.extend(marker_spans(2, m2, state, theme));
        try_add(c, &mut used, &mut spans);
    }

    // Δ between the two markers.
    if let (Some(a), Some(b)) = (m1, m2) {
        let df = (b.freq_hz as i64 - a.freq_hz as i64).unsigned_abs();
        let dl = match (level_at_freq(state, a.freq_hz), level_at_freq(state, b.freq_hz)) {
            (Some(x), Some(y)) => Some(y - x),
            _ => None,
        };
        let c = vec![
            Span::raw("   "),
            Span::styled("\u{0394} ", dim),
            Span::styled(fmt_delta(df, dl), Style::default().fg(theme.value).add_modifier(Modifier::BOLD)),
        ];
        try_add(c, &mut used, &mut spans);
    }

    // Right-side focus hints from the currently focused panel, if any room.
    let hints: Vec<Span> = state.ui.focused_panel_bindings.iter()
        .flat_map(|(k, l)| vec![
            Span::styled(format!("[{k}] "), key),
            Span::styled(format!("{l}  "), dim),
        ]).collect();
    let hw = span_w(&hints);
    if !hints.is_empty() && used + hw + 2 <= iw {
        let filler = iw.saturating_sub(used + hw);
        spans.push(Span::raw(" ".repeat(filler)));
        spans.extend(hints);
    }

    vec![hairline(iw, theme.border_dim), Line::from(spans)]
}

// ── Panels ──────────────────────────────────────────────────────────────────

/// Top instrument-state banner for the lab presets.
pub struct LabBannerPanel;

impl Panel for LabBannerPanel {
    fn name(&self) -> &'static str { "lab_banner" }
    fn min_size(&self) -> (u16, u16) { (20, 1) }
    // `b` focuses the measurement banner to drive REF / averaging / CAL directly.
    fn focus_key(&self) -> Option<char> { Some('b') }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("↑↓", "Ref level"), ("[ ]", "Averaging"), ("C", "Capture cal"), ("R", "Clear ref")]
    }
    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::untitled().frame(FrameStyle::Borderless)
    }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        let lines = banner_lines(state, theme, area.width as usize, focused);
        f.render_widget(Paragraph::new(lines), area);
    }
}

/// Bottom marker / delta readout bar for the lab presets.
pub struct LabMarkerPanel;

impl Panel for LabMarkerPanel {
    fn name(&self) -> &'static str { "lab_marker" }
    fn min_size(&self) -> (u16, u16) { (20, 1) }
    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::untitled().frame(FrameStyle::Borderless)
    }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {
        let lines = marker_lines(state, theme, area.width as usize);
        f.render_widget(Paragraph::new(lines), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A signal state with real numbers in it, as if the FFT had just run.
    fn measured() -> crate::state::SignalState {
        crate::state::SignalState {
            peak_to_nf_db: 47.0,
            channel_power_dbfs: -28.6,
            occupied_bw_hz: 120_000,
            acpr_lower_db: -34.9,
            acpr_upper_db: -35.1,
            modulation: crate::state::Modulation::Wfm,
            ..Default::default()
        }
    }

    #[test]
    fn signal_banner_reads_the_measurement_while_it_is_live() {
        let f = signal_fields(&measured(), false);
        let get = |k: &str| f.iter().find(|(n, _)| *n == k).unwrap().1.clone();
        assert_eq!(get("MOD"), "WFM");
        assert_eq!(get("SNR"), "47 dB");
        assert_eq!(get("CH PWR"), "-28.6 dBFS");
        assert_eq!(get("OBW"), "120.0 kHz");
    }

    #[test]
    fn signal_banner_dashes_every_field_once_the_frame_ages_out() {
        // The bug: stop RX and the characterization panel dashed its whole stack
        // while this line, two rows above it, went on reading `SNR 47 dB · CH PWR
        // -28.6 dBFS` from a frame that was no longer arriving.
        let f = signal_fields(&measured(), true);
        assert_eq!(f.len(), 4, "the field set must not change shape when stale");
        for (name, value) in &f {
            assert_eq!(value, "\u{2014}", "{name} still reads as live");
        }
    }

    #[test]
    fn signal_banner_keeps_its_own_undefined_sentinels() {
        // Live, but nothing measured yet: still dashes, from the sentinels rather
        // than from staleness.
        let f = signal_fields(&crate::state::SignalState::default(), false);
        let get = |k: &str| f.iter().find(|(n, _)| *n == k).unwrap().1.clone();
        assert_eq!(get("MOD"), "\u{2014}");
        assert_eq!(get("CH PWR"), "\u{2014}");
        assert_eq!(get("OBW"), "\u{2014}");
    }

    #[test]
    fn lab_label_maps_current_key_numbers() {
        assert_eq!(lab_label("lab_rf"),     Some(("RF CHAIN", '6')));
        assert_eq!(lab_label("lab_iq"),     Some(("I/Q QUALITY", '5')));
        assert_eq!(lab_label("lab_signal"), Some(("SIGNAL", '8')));
        assert_eq!(lab_label("command_rail"), None);
        assert_eq!(lab_label("spectrum"),     None);
    }

    #[test]
    fn fmt_freq_mhz_picks_unit() {
        assert_eq!(fmt_freq_mhz(92_800_000),    "92.800 MHz");
        assert_eq!(fmt_freq_mhz(433_920_000),   "433.920 MHz");
        assert_eq!(fmt_freq_mhz(1_234_500_000), "1.234500 GHz");
    }

    #[test]
    fn fmt_delta_formats_with_and_without_level() {
        assert_eq!(fmt_delta(5_400_000, Some(-12.3)), "5.400 MHz 12.3 dB");
        assert_eq!(fmt_delta(5_400_000, None),        "5.400 MHz");
    }

    #[test]
    fn titlecase_first_upper_rest_lower() {
        assert_eq!(titlecase("EXCELLENT"), "Excellent");
        assert_eq!(titlecase("POOR"), "Poor");
        assert_eq!(titlecase(""), "");
    }

    #[test]
    fn timing_banner_deadline_met_then_breached() {
        let mut t = crate::state::TimingState {
            cb_period_us: 13_107, cb_jitter_us: 42, deadline_budget_us: 603, ..Default::default()
        };
        // No late callbacks → DEADLINE reads "✓ met".
        let f = timing_banner_fields(&t);
        let deadline = f.iter().find(|(k, _)| *k == "DEADLINE").unwrap();
        assert_eq!(deadline.1, "\u{2713} met");
        assert!(f.iter().any(|(k, v)| *k == "CALLBACK" && v == "13.107 ms"));
        // A worst slip past budget → "⚠ N%" of the budget.
        t.late_callbacks = 3;
        t.dev_peak_us = 6_300; // 6300 / 603 ≈ 1044%
        let f = timing_banner_fields(&t);
        let deadline = f.iter().find(|(k, _)| *k == "DEADLINE").unwrap();
        assert_eq!(deadline.1, "\u{26a0} 1044%");
    }

    #[test]
    fn rf_chain_str_inserts_amp_and_collapses_single_tuner() {
        assert_eq!(rf_chain_str(true, false), "ANT\u{25B8}LNA\u{25B8}MIX\u{25B8}VGA\u{25B8}ADC");
        assert_eq!(rf_chain_str(true, true),  "ANT\u{25B8}AMP\u{25B8}LNA\u{25B8}MIX\u{25B8}VGA\u{25B8}ADC");
        assert_eq!(rf_chain_str(false, true), "ANT\u{25B8}TUNER\u{25B8}ADC", "no cascade → single tuner");
    }
}
