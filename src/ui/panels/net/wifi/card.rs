// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiCardPanel` - the Frames view's card: how many frames a second
//! this channel carries, by format, and the selected frame field by field;
//! or, turned with `d`, the account of every preamble found.
//!
//! **The reading is measured over whole seconds.** The frames of this
//! channel in each complete one-second block back from the newest frame
//! heard, on the stream's own clock and within one run of it, over
//! everything heard and never the capture alone (a capture is what is
//! looked at, not what is measured): a part
//! second is not a second, and a break restarts the clock. Its 1 sigma is
//! the standard error over those blocks, given from the third on as the
//! survey gives its own; under a whole second no rate is given at all.
//! Another channel's frames are said beside it, never counted in it.
//!
//! **The selected frame says what was read of it**, field by field, and
//! names what is not read yet rather than leaving it out: a later format's
//! L-SIG is marked as its airtime, not its rate; the channel is the
//! estimate's magnitude across the 52 tones against its own mean, on a
//! window of at least 6 dB so a ripple is not stretched to look like a
//! notch.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::frames::{ink, selection, shown, since_trigger, when};
use crate::signal::wifi::format::Format;
use crate::signal::wifi::hesig::{Bss, HePpdu};
use crate::state::{Family, RowKind, SdrMetrics, WifiRow};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};
use crate::ui::widgets::reading::Reading;

pub struct NetWifiCardPanel;

/// Blocks before a spread is worth a number: the survey's own threshold.
const SPREAD_FROM: usize = 3;

/// Columns of a field's label.
const LABEL: usize = 10;

/// Frames a second over the complete one-second blocks of the run.
#[derive(Debug, PartialEq)]
struct Rate {
    mean: f64,
    sigma: Option<f64>,
    blocks: usize,
    /// This channel's frames in those blocks by family, most first.
    by: Vec<(Family, usize)>,
    /// Another channel's frames a second over the same blocks.
    other_per_s: f64,
}

/// The rate from `rows`, newest first from the newest shown: `None` under
/// a whole second of the run.
fn rate(rows: &[&WifiRow]) -> Option<Rate> {
    let newest = rows.first()?;
    let back = |r: &WifiRow| (newest.start as f64 - r.start as f64) / 20e6;
    let run: Vec<&WifiRow> = rows
        .iter()
        .copied()
        .filter(|r| r.run == newest.run)
        .collect();
    let blocks = run.iter().map(|r| back(r)).fold(0.0, f64::max).floor() as usize;
    if blocks == 0 {
        return None;
    }
    let mut per = vec![0.0f64; blocks];
    let mut by = [0usize; Family::ALL.len()];
    let mut other = 0usize;
    for r in run {
        let k = back(r).floor() as usize;
        if k >= blocks {
            continue;
        }
        match r.family() {
            Some(f) => {
                per[k] += 1.0;
                if let Some(i) = Family::ALL.iter().position(|g| *g == f) {
                    by[i] += 1;
                }
            }
            None => other += 1,
        }
    }
    let n = blocks as f64;
    let mean = per.iter().sum::<f64>() / n;
    let sigma = (blocks >= SPREAD_FROM).then(|| {
        let var = per.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
        (var / n).sqrt()
    });
    let mut by: Vec<(Family, usize)> = Family::ALL
        .iter()
        .copied()
        .zip(by)
        .filter(|(_, c)| *c > 0)
        .collect();
    by.sort_by_key(|f| std::cmp::Reverse(f.1));
    Some(Rate {
        mean,
        sigma,
        blocks,
        by,
        other_per_s: other as f64 / n,
    })
}

/// What a frame says of its network: its colour, a label two networks can
/// share, its direction and the TXOP it protects.
fn network(bss: &Bss) -> String {
    let txop = match bss.txop_us {
        None => "no TXOP".to_string(),
        Some(us) if us < 1_000 => format!("TXOP {us} \u{b5}s"),
        Some(us) => format!("TXOP {:.2} ms", us as f64 / 1e3),
    };
    format!(
        " \u{b7} BSS colour {} \u{b7} {} \u{b7} {txop}",
        bss.colour,
        if bss.uplink { "uplink" } else { "downlink" }
    )
}

fn yes(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// One field of the selected frame: its label, its value and how it reads.
struct Field {
    label: &'static str,
    value: String,
    quiet: bool,
}

fn field(label: &'static str, value: impl Into<String>) -> Field {
    Field {
        label,
        value: value.into(),
        quiet: false,
    }
}

fn quiet(label: &'static str, value: impl Into<String>) -> Field {
    Field {
        label,
        value: value.into(),
        quiet: true,
    }
}

/// The 802.11b PLCP header's fields.
fn plcp(
    preamble: crate::signal::wifi::dsss::header::Preamble,
    header: &Result<
        crate::signal::wifi::dsss::header::Header,
        crate::signal::wifi::dsss::header::Fault,
    >,
) -> Field {
    use crate::signal::wifi::dsss::header::Preamble;
    let p = match preamble {
        Preamble::Long => "long preamble",
        Preamble::Short => "short preamble",
    };
    match header {
        Ok(h) => field(
            "PLCP",
            format!(
                "{p} \u{b7} {} Mb/s \u{b7} {} B \u{b7} {} \u{b5}s \u{b7} clocks locked {}",
                h.rate.mbps(),
                h.octets(),
                h.length_us,
                yes(h.locked)
            ),
        ),
        Err(fault) => field(
            "PLCP",
            format!("{p} \u{b7} header refused: {}", fault.label()),
        ),
    }
}

/// The selected frame's fields, before its channel.
fn fields(row: &WifiRow, state: &SdrMetrics) -> Vec<Field> {
    let now = std::time::Instant::now();
    let mut out = Vec::new();
    let snr = row
        .snr_db
        .map(|s| format!("SNR {s:.1} dB"))
        .unwrap_or_default();
    out.push(match row.offset_hz {
        Some(o) => {
            let t = state.radio.transmitter_offset(o, row.centre_hz(), now).khz;
            field(
                "preamble",
                format!("{:+.2} \u{b1} {:.2} kHz \u{b7} {snr}", t.value(), t.sigma()),
            )
        }
        None => quiet("preamble", "not read: only its start was found"),
    });
    if let Some(c) = &row.clock {
        out.push(field(
            "clock",
            format!(
                "{} symbol clock \u{b7} carrier {} over the data \u{b7} {} symbols",
                Reading::new(c.clock_ppm, "ppm", f64::INFINITY).text(),
                Reading::new(c.carrier_ppm, "ppm", f64::INFINITY).text(),
                c.symbols
            ),
        ));
        let check = &state.net.wifi.clock_check;
        if let Some(mean) = check.mean() {
            let mut v = format!(
                "clock \u{2212} carrier {} over {} frames",
                Reading::new(mean, "ppm", f64::INFINITY).text(),
                check.frames
            );
            if let Some(chi2) = check.chi2_per_dof() {
                v.push_str(&format!(" \u{b7} \u{3c7}\u{b2} {chi2:.1} a frame"));
            }
            out.push(quiet("clocks", v));
        }
    }
    match &row.kind {
        RowKind::Ofdm(heard) => {
            let later = !matches!(heard.format, Some(Format::NonHt));
            let greenfield = matches!(heard.format, Some(Format::Greenfield(Ok(_))));
            out.push(match heard.lsig {
                None if greenfield => quiet("L-SIG", "none in greenfield"),
                None => quiet("L-SIG", "none read"),
                Some(Ok(l)) => field(
                    "L-SIG",
                    format!(
                        "{} Mb/s \u{b7} {} B \u{b7} parity ok{}",
                        l.rate.mbps(),
                        l.length,
                        if later {
                            " \u{b7} its airtime, not its rate"
                        } else {
                            ""
                        }
                    ),
                ),
                Some(Err(why)) => field("L-SIG", format!("refused: {}", why.label())),
            });
            let refused = |label, fault: &str| field(label, format!("refused: {fault}"));
            match heard.format {
                None => {}
                Some(Format::NonHt) => out.push(field("format", "non-HT (802.11a/g)")),
                Some(Format::HtMixed(sig) | Format::Greenfield(sig)) => out.push(match sig {
                    Ok(h) => field(
                        "HT-SIG",
                        format!(
                            "MCS {} \u{b7} {} B \u{b7} STBC {} \u{b7} LDPC {} \u{b7} SGI {} \u{b7} CRC ok",
                            h.mcs,
                            h.length,
                            h.stbc,
                            yes(h.ldpc),
                            yes(h.short_gi)
                        ),
                    ),
                    Err(f) => refused("HT-SIG", f.label()),
                }),
                Some(Format::Vht(sig)) => out.push(match sig {
                    Ok(()) => field("VHT-SIG-A", "CRC ok \u{b7} a 5 GHz format"),
                    Err(f) => refused("VHT-SIG-A", f.label()),
                }),
                Some(Format::He { sig_a, .. }) => out.push(match sig_a {
                    Ok(a) => {
                        let name = match a.ppdu {
                            HePpdu::Su => "HE SU",
                            HePpdu::ErSu => "HE ER SU",
                            HePpdu::Mu => "HE MU",
                            HePpdu::Tb => "HE TB",
                        };
                        field("HE-SIG-A", format!("{name}{}", network(&a.bss)))
                    }
                    Err(f) => refused("HE-SIG-A", f.label()),
                }),
                Some(Format::USig { er, sig }) => out.push(match sig {
                    Ok(u) => {
                        let name = match u.version {
                            0 => "EHT".to_string(),
                            v => format!("PHY version {v}, after EHT"),
                        };
                        let er = if er { ", extended range" } else { "" };
                        field("U-SIG", format!("{name}{er}{}", network(&u.bss)))
                    }
                    Err(f) => refused("U-SIG", f.label()),
                }),
            }
        }
        RowKind::Dsss { preamble, header } => out.push(plcp(*preamble, header)),
        RowKind::OtherOfdm { shift_mhz } => out.push(quiet(
            "frame",
            format!("another channel's, {shift_mhz:+} MHz away: only its start was read"),
        )),
        RowKind::OtherDsss {
            shift_mhz,
            preamble,
            header,
        } => {
            out.push(quiet(
                "frame",
                format!("another channel's, {shift_mhz:+} MHz away, read at its own centre"),
            ));
            out.push(plcp(*preamble, header));
        }
    }
    out
}

/// `value` cut into rows of `width` at its dots, where it can be.
fn wrap_at_dots(value: &str, width: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    for part in value.split(" \u{b7} ") {
        let cur = out.last_mut().expect("never empty");
        let add = if cur.is_empty() {
            part.to_string()
        } else {
            format!(" \u{b7} {part}")
        };
        if !cur.is_empty() && cur.chars().count() + add.chars().count() > width {
            out.push(part.to_string());
        } else {
            cur.push_str(&add);
        }
    }
    out
}

/// A rule across the card with `head` in it.
fn rule(head: &str, width: usize, theme: &crate::Theme) -> Line<'static> {
    let lead = format!(" \u{2500} {head} ");
    let rest = width.saturating_sub(lead.chars().count() + 1);
    Line::from(Span::styled(
        format!("{lead}{}", "\u{2500}".repeat(rest)),
        Style::default().fg(theme.border_dim),
    ))
}

/// The reading on top: frames a second in big digits, or why there is none.
fn reading(rows: &[&WifiRow], width: usize, theme: &crate::Theme, out: &mut Vec<Line<'static>>) {
    let label = Style::default().fg(theme.label);
    let Some(r) = rate(rows) else {
        let own = rows.iter().filter(|r| r.family().is_some()).count();
        // Frames kept from before a break, and still no whole second since:
        // the feed losing blocks is why, not too little heard.
        let broken = rows.iter().any(|r| r.run != rows[0].run);
        let why = if broken {
            "no unbroken second of the stream yet: the feed lost blocks within the last second"
                .to_string()
        } else {
            format!("fewer than a second of frames yet \u{b7} {own} of this channel's")
        };
        for l in crate::ui::chrome::wrap(&why, width.saturating_sub(2), 2) {
            out.push(Line::from(Span::styled(format!(" {l}"), label)));
        }
        return;
    };
    let digits = if r.mean >= 10.0 {
        format!("{:.0}", r.mean)
    } else {
        format!("{:.1}", r.mean)
    };
    let big: Vec<String> = (0..3)
        .map(|i| {
            digits
                .chars()
                .map(|c| crate::ui::widgets::bigdigits::glyph(c)[i])
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let blocks = format!(
        "{} one-second block{}",
        r.blocks,
        if r.blocks == 1 { "" } else { "s" }
    );
    let side = match r.sigma {
        Some(s) => [
            String::new(),
            format!("\u{b1} {s:.1} a second"),
            format!("1 \u{3c3}, {blocks}"),
        ],
        None => [
            String::new(),
            "a second".to_string(),
            format!("over {blocks}; a spread from {SPREAD_FROM}"),
        ],
    };
    let hi = Style::default()
        .fg(theme.value_hi)
        .add_modifier(Modifier::BOLD);
    for (b, t) in big.into_iter().zip(side) {
        out.push(Line::from(vec![
            Span::styled(format!(" {b}"), hi),
            Span::styled(format!("  {t}"), label),
        ]));
    }
    if r.other_per_s > 0.0 {
        out.push(Line::from(Span::styled(
            format!(" + {:.1} a second from other channels", r.other_per_s),
            label,
        )));
    }
    // The split by format: one bar, then its key.
    let total: usize = r.by.iter().map(|(_, n)| n).sum();
    if total == 0 {
        return;
    }
    out.push(Line::raw(""));
    let bar = width.saturating_sub(2);
    let mut spans = vec![Span::raw(" ")];
    for (f, n) in &r.by {
        let cells = ((*n as f64 / total as f64) * bar as f64).round() as usize;
        spans.push(Span::styled(
            "\u{2588}".repeat(cells),
            Style::default().fg(theme.wifi(*f)),
        ));
    }
    out.push(Line::from(spans));
    let mut line = vec![Span::raw(" ")];
    let mut used = 1;
    for (f, n) in &r.by {
        let item = format!("{} {:.0}%   ", f.label(), 100.0 * *n as f64 / total as f64);
        let w = 2 + item.chars().count();
        if used + w > width && used > 1 {
            out.push(Line::from(std::mem::take(&mut line)));
            line.push(Span::raw(" "));
            used = 1;
        }
        line.push(Span::styled(
            "\u{25a0} ",
            Style::default().fg(theme.wifi(*f)),
        ));
        line.push(Span::styled(item, label));
        used += w;
    }
    out.push(Line::from(line));
}

/// The selected frame's channel: magnitude per tone in braille, its range,
/// and what it is.
fn channel(row: &WifiRow, compact: bool, theme: &crate::Theme, out: &mut Vec<Line<'static>>) {
    let label = Style::default().fg(theme.label);
    let stale = Style::default().fg(theme.stale);
    let head = Span::styled(format!(" {:<LABEL$}", "channel"), label);
    let Some(c) = &row.channel else {
        let why = match row.kind {
            RowKind::Dsss { .. } | RowKind::OtherDsss { .. } => {
                "an 802.11b frame has no OFDM estimate"
            }
            RowKind::OtherOfdm { .. } => "another channel's: none estimated",
            RowKind::Ofdm(_) => "not estimated on this frame",
        };
        out.push(Line::from(vec![head, Span::styled(why, stale)]));
        return;
    };
    let db: Vec<f32> = c.iter().map(|v| f32::from(*v) / 100.0).collect();
    let lo = db.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let rows = if compact { 2 } else { 3 };
    let plot =
        crate::ui::widgets::charts::braille_profile_in(&db, 26, rows, lo.min(-3.0), hi.max(3.0));
    let ink = Style::default().fg(ink(row, theme));
    for (i, p) in plot.into_iter().enumerate() {
        let lead = if i == 0 {
            head.clone()
        } else {
            Span::raw(" ".repeat(LABEL + 1))
        };
        let mut spans = vec![lead, Span::styled(p, ink)];
        if i == 0 {
            spans.push(Span::styled(format!("  {hi:+.1} dB"), label));
        } else if i == rows - 1 {
            spans.push(Span::styled(format!("  {lo:+.1} dB"), label));
        }
        out.push(Line::from(spans));
    }
    if !compact {
        out.push(Line::from(Span::styled(
            format!(
                "{}-26 \u{b7} |H| across the 52 tones, against its mean \u{b7} +26",
                " ".repeat(LABEL + 1)
            ),
            label,
        )));
    }
}

/// A plain state in place of the reading: its head and why.
fn state_lines(head: &str, why: &str, width: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(Span::styled(
        format!(" {head}"),
        Style::default()
            .fg(theme.value)
            .add_modifier(Modifier::BOLD),
    ))];
    for l in crate::ui::chrome::wrap(why, width.saturating_sub(2), 4) {
        out.push(Line::from(Span::styled(
            format!(" {l}"),
            Style::default().fg(theme.label),
        )));
    }
    out
}

fn lines(
    state: &SdrMetrics,
    width: usize,
    height: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    if let Some(s) = super::frames_silence(state) {
        let what = format!("{} {}", s.why, s.keys_text());
        return state_lines(s.card, &what, width, theme);
    }
    let shown = shown(state);
    let ch = crate::signal::net::band::wifi_channel(state.radio.frequency)
        .map_or("this channel".to_string(), |c| format!("channel {c}"));
    if shown.is_empty() {
        return state_lines(
            "LISTENING",
            &format!("on {ch}'s centre: no frame of {ch} yet"),
            width,
            theme,
        );
    }
    // Over everything heard, whatever the capture shows: a capture is
    // what is looked at, not what is measured.
    let rows = &shown[..];
    let mut out = Vec::new();
    if rows.iter().any(|r| r.family().is_some()) {
        reading(rows, width, theme, &mut out);
    } else {
        out.extend(state_lines(
            "ONLY OTHER CHANNELS",
            &format!(
                "{} frames from channels 5, 10 or 15 MHz away, on the river as they \
                 were read; none of {ch}'s own",
                rows.len()
            ),
            width,
            theme,
        ));
    }
    out.push(Line::raw(""));
    out.extend(lower(state, width, height, theme, ""));
    out
}

/// The card's lower part: the selected frame field by field, or with `d`
/// the decode account; with `back`, the key that leaves it, where the list
/// shows it in its own place.
pub(super) fn lower(
    state: &SdrMetrics,
    width: usize,
    height: usize,
    theme: &crate::Theme,
    back: &str,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let view = &state.net.wifi.frames_view;
    if view.decode {
        out.push(rule(
            &format!("DECODE ACCOUNT \u{b7} d: the selected frame{back}"),
            width,
            theme,
        ));
        out.extend(super::super::shared::decode_health::wifi_lines(
            state, theme, width,
        ));
        return out;
    }
    let Some((c, at)) = selection(state) else {
        return out;
    };
    let row = &c.rows[at];
    out.push(rule(
        &format!(
            "SELECTED  {} from the trigger \u{b7} d: the decode account{back}",
            when(since_trigger(row, c)).trim()
        ),
        width,
        theme,
    ));
    // Short of rows the channel takes two and the MAC line goes first.
    let compact = height < 22;
    let value_w = width.saturating_sub(LABEL + 2).max(10);
    for f in fields(row, state) {
        let style = Style::default().fg(if f.quiet { theme.stale } else { theme.value });
        for (i, chunk) in wrap_at_dots(&f.value, value_w).into_iter().enumerate() {
            let lead = if i == 0 { f.label } else { "" };
            out.push(Line::from(vec![
                Span::styled(format!(" {lead:<LABEL$}"), Style::default().fg(theme.label)),
                Span::styled(chunk, style),
            ]));
        }
    }
    channel(row, compact, theme, &mut out);
    if !compact {
        out.push(Line::from(vec![
            Span::styled(
                format!(" {:<LABEL$}", "MAC"),
                Style::default().fg(theme.label),
            ),
            Span::styled(
                "not read yet: it is in the payload",
                Style::default().fg(theme.stale),
            ),
        ]));
    }
    out
}

/// Whether the card has room on a screen `cols` wide: at 191 and 152 it
/// does, at 100 the list and the river need every column.
pub fn fits(cols: u16) -> bool {
    cols >= 140
}

/// Whether the card is on screen now, by the width the app last drew at;
/// unknown reads as wide.
pub fn on_screen(state: &SdrMetrics) -> bool {
    state.ui.screen_cols == 0 || fits(state.ui.screen_cols)
}

/// The reading in words, for the river's title where the card has no room:
/// `None` until there is a rate to give.
pub(super) fn reading_words(state: &SdrMetrics) -> Option<String> {
    let r = rate(&shown(state))?;
    let mean = if r.mean >= 10.0 {
        format!("{:.0}", r.mean)
    } else {
        format!("{:.1}", r.mean)
    };
    Some(match r.sigma {
        Some(s) => format!("{mean} \u{b1} {s:.1} frames a second"),
        None => format!("{mean} frames a second"),
    })
}

impl Panel for NetWifiCardPanel {
    fn name(&self) -> &'static str {
        "net_wifi_card"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 10)
    }

    fn column_width(&self, body: u16) -> Option<u16> {
        (!fits(body)).then_some(0)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let chrome = PanelChrome::new("Frames a second")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .counts_from_feed(FeedSpan::Session);
        let view = &state.net.wifi.frames_view;
        // Only while the selected frame's fields are on it: not in place of
        // a silence, nor under the decode account.
        let offset = super::frames_silence(state).is_none()
            && !view.decode
            && selection(state).is_some_and(|(c, at)| c.rows[at].offset_hz.is_some());
        if offset {
            chrome.shows_offsets()
        } else {
            chrome
        }
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let text = lines(state, inner.width as usize, inner.height as usize, theme);
        f.render_widget(Paragraph::new(text), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::{rate, NetWifiCardPanel, Rate};
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::signal::wifi::dsss::header::{Header, Preamble, Rate as DRate};
    use crate::signal::wifi::format::{Format, Heard};
    use crate::signal::wifi::hesig::{Bss, HeKind, HePpdu, HeSigA};
    use crate::signal::wifi::htsig::HtSig;
    use crate::signal::wifi::lsig::{LSig, Refused};
    use crate::signal::wifi::signal_field::Rate as LRate;
    use crate::state::fixture::draw;
    use crate::state::{Family, RowKind, SdrMetrics, WifiClock, WifiRow};

    fn on_eleven() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "wifi_channel".to_string();
        m.ui.active_preset = "wifi_frames".to_string();
        m.radio.frequency = 2_462_000_000;
        m.radio.config_sample_rate = 20e6;
        m.net.wifi.ran = true;
        m.net.wifi.channel = Some(11);
        m.net.mode = crate::state::NetMode::Lock;
        m
    }

    fn non_ht() -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: Some(Ok(LSig {
                rate: LRate::R24,
                length: 20,
            })),
            format: Some(Format::NonHt),
        })
    }

    fn ht() -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: Some(Ok(LSig {
                rate: LRate::R6,
                length: 2_000,
            })),
            format: Some(Format::HtMixed(Ok(HtSig {
                mcs: 7,
                cbw40: false,
                length: 1_500,
                stbc: 0,
                ldpc: false,
                short_gi: true,
                ness: 0,
            }))),
        })
    }

    /// A frame `before_s` seconds before the newest's place at 10 s.
    fn at(before_s: f64, kind: RowKind) -> WifiRow {
        WifiRow {
            seq: 0,
            start: ((10.0 - before_s) * 20e6) as u64,
            run: 0,
            when: std::time::Instant::now(),
            carrier_hz: 2_462e6,
            kind,
            airtime_us: Some(200),
            gap_us: None,
            offset_hz: Some(Uncertain::from_sigma(-48_000.0, 200.0)),
            snr_db: Some(27.0),
            channel: None,
            clock: None,
            epoch: 0,
        }
    }

    /// The ring as one capture, set off by its newest frame.
    fn captured(m: &mut SdrMetrics) {
        let rows: Vec<WifiRow> = m.net.wifi.rows.iter().copied().collect();
        let t = rows[rows.len() - 1];
        m.net.wifi.scope.capture = Some(crate::state::Capture {
            number: 3,
            zoom: crate::state::FrameZoom::Rhythm,
            trigger: crate::state::Trigger::Any,
            epoch: 0,
            from: t.start.saturating_sub(32_000),
            to: t.start + 288_000,
            trigger_seq: t.seq,
            rows,
            lost: Vec::new(),
        });
    }

    fn text(m: &SdrMetrics, w: u16, h: u16) -> String {
        draw(NetWifiCardPanel, w, h, m).join("\n")
    }

    /// Ten, twelve and fourteen of this channel's frames in the three
    /// whole seconds back from the newest, a frame in the part-second
    /// before them (not a whole block, so not counted), and two of another
    /// channel's: twelve a second, its 1 sigma the standard error over
    /// three blocks, the split over the same frames, the other channel
    /// apart.
    fn three_seconds() -> Vec<WifiRow> {
        let mut rows = Vec::new();
        for (block, n) in [(0, 10), (1, 12), (2, 14)] {
            for i in 0..n {
                let kind = if i % 2 == 0 { ht() } else { non_ht() };
                rows.push(at(
                    f64::from(block) + 0.9 * f64::from(i) / f64::from(n),
                    kind,
                ));
            }
        }
        rows.push(at(1.5, RowKind::OtherOfdm { shift_mhz: 5 }));
        rows.push(at(1.6, RowKind::OtherOfdm { shift_mhz: 5 }));
        rows.push(at(3.5, ht()));
        rows.sort_by_key(|r| std::cmp::Reverse(r.start));
        rows
    }

    #[test]
    fn frames_a_second_over_whole_seconds() {
        let rows = three_seconds();
        let refs: Vec<&WifiRow> = rows.iter().collect();
        let r = rate(&refs).expect("three whole seconds");
        assert_eq!(r.blocks, 3);
        assert!((r.mean - 12.0).abs() < 1e-9, "{r:?}");
        let se = 2.0 / 3f64.sqrt();
        assert!((r.sigma.unwrap() - se).abs() < 1e-9, "{r:?}");
        assert_eq!(r.by, vec![(Family::NonHt, 18), (Family::Ht, 18)]);
        assert!((r.other_per_s - 2.0 / 3.0).abs() < 1e-9, "{r:?}");

        // Under three blocks the spread is not worth a number, as the
        // survey's is not; under one there is no rate.
        let two: Vec<&WifiRow> = refs
            .iter()
            .copied()
            .filter(|r| r.start as f64 >= 7.4 * 20e6)
            .collect();
        let r = rate(&two).unwrap();
        assert_eq!((r.blocks, r.sigma), (2, None));
        let short: Vec<&WifiRow> = refs.iter().copied().take(5).collect();
        assert_eq!(rate(&short), None::<Rate>);
    }

    /// The reading on the card: its spread, what it was measured over, the
    /// split's key and the other channels in words.
    #[test]
    fn the_card_states_its_reading_and_what_it_was_measured_over() {
        let mut m = on_eleven();
        for r in three_seconds().into_iter().rev() {
            m.net.wifi.push_row(r);
        }
        let out = text(&m, 70, 27);
        assert!(out.contains("\u{b1} 1.2 a second"), "{out}");
        assert!(out.contains("1 \u{3c3}, 3 one-second blocks"), "{out}");
        assert!(
            out.contains("HT 50%") && out.contains("non-HT 50%"),
            "{out}"
        );
        assert!(out.contains("0.7 a second from other channels"), "{out}");
    }

    /// **What gives way**: the card keeps its place at 191 and 152 columns
    /// and gives it up at 100, where its reading moves into the river's
    /// title: the two are never both on screen and never neither.
    #[test]
    fn the_card_gives_way_on_a_narrow_screen_and_its_reading_moves() {
        let (engine, _) =
            crate::app::App::build_ui("wifi_frames", &std::collections::HashMap::new(), None, true);
        let theme = crate::Theme::sdr();
        let mut m = on_eleven();
        for r in three_seconds().into_iter().rev() {
            m.net.wifi.push_row(r);
        }
        for (w, h, card) in [(191u16, 41u16, true), (152, 32, true), (100, 30, false)] {
            m.ui.screen_cols = w;
            let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            t.draw(|f| engine.draw(f, &m, &theme)).unwrap();
            let buf = t.backend().buffer().clone();
            let text: Vec<String> = (0..h)
                .map(|y| (0..w).map(|x| buf.get(x, y).symbol()).collect())
                .collect();
            let all = text.join("\n");
            let river = text.iter().find(|l| l.contains("River")).unwrap();
            assert_eq!(all.contains("Frames a second"), card, "{w} x {h}:\n{all}");
            assert_eq!(
                river.contains("12 \u{b1} 1.2 frames a second"),
                !card,
                "{w} x {h}: {river}"
            );
            // Given way, the body takes the room: the river runs the width.
            if !card {
                assert!(river.trim_end().ends_with('\u{256e}'), "{river}");
                assert_eq!(river.chars().count(), usize::from(w));
            }
        }
    }

    /// Seconds of frames, but broken by the feed's losses into runs each
    /// shorter than one: the card says the breaks are why, not that it has
    /// heard too little.
    #[test]
    fn a_feed_that_keeps_breaking_is_named_as_the_reason() {
        let mut m = on_eleven();
        for run in 0..4u64 {
            for i in 0..5 {
                m.net.wifi.push_row(WifiRow {
                    run,
                    ..at(4.0 - run as f64 - 0.1 * f64::from(i), ht())
                });
            }
        }
        let out = text(&m, 70, 27);
        assert!(
            out.contains("no unbroken second of the stream yet"),
            "{out}"
        );
        assert!(out.contains("blocks"), "{out}");
        assert!(!out.contains("fewer than a second"), "{out}");
    }

    /// Under a whole second of frames, no rate is given.
    #[test]
    fn under_a_second_no_rate_is_given() {
        let mut m = on_eleven();
        m.net.wifi.push_row(at(0.5, ht()));
        m.net.wifi.push_row(at(0.0, ht()));
        let out = text(&m, 70, 27);
        assert!(out.contains("fewer than a second of frames yet"), "{out}");
        assert!(!out.contains("one-second blocks"), "{out}");
    }

    /// **The selected frame, field by field**: its preamble with 1 sigma,
    /// its clock and the session's, L-SIG marked as a later format's
    /// airtime, HT-SIG, the channel across its 52 tones, and the MAC
    /// header named as not read yet.
    #[test]
    fn the_selected_frame_is_shown_field_by_field() {
        let mut m = on_eleven();
        let mut channel = [0i16; 52];
        channel[0] = 130;
        channel[51] = -200;
        let clock = WifiClock {
            clock_ppm: Uncertain::from_sigma(-19.6, 0.4),
            carrier_ppm: Uncertain::from_sigma(-19.4, 0.1),
            preamble_ppm: Uncertain::from_sigma(-19.5, 1.0),
            symbols: 68,
            when: std::time::Instant::now(),
        };
        m.net.wifi.clock_check.add(&clock);
        m.net.wifi.push_row(WifiRow {
            channel: Some(channel),
            clock: Some(clock),
            ..at(0.0, ht())
        });
        captured(&mut m);
        let out = text(&m, 70, 27);
        for want in [
            "SELECTED  +0.000 ms from the trigger",
            "48.00 \u{b1} 0.20 kHz",
            "SNR 27.0 dB",
            "symbol clock",
            "68 symbols",
            "6 Mb/s \u{b7} 2000 B \u{b7} parity ok \u{b7} its airtime, not its rate",
            "MCS 7 \u{b7} 1500 B \u{b7} STBC 0 \u{b7} LDPC no \u{b7} SGI yes \u{b7} CRC ok",
            "+1.3 dB",
            "-2.0 dB",
            "|H| across the 52 tones, against its mean",
            "not read yet: it is in the payload",
        ] {
            assert!(out.contains(want), "{want} in\n{out}");
        }
    }

    /// An 802.11b frame's header; HE's network; a refused L-SIG by its
    /// check; another channel's frame as what little was read of it.
    #[test]
    fn each_kind_of_frame_says_what_was_read_of_it() {
        let cases: [(RowKind, &[&str]); 4] = [
            (
                RowKind::Dsss {
                    preamble: Preamble::Long,
                    header: Ok(Header {
                        rate: DRate::M1,
                        locked: true,
                        extension: false,
                        length_us: 304,
                    }),
                },
                &[
                    "long preamble \u{b7} 1 Mb/s \u{b7} 38 B \u{b7} 304 \u{b5}s",
                    "an 802.11b frame has no OFDM estimate",
                ],
            ),
            (
                RowKind::Ofdm(Heard {
                    lsig: Some(Ok(LSig {
                        rate: LRate::R6,
                        length: 300,
                    })),
                    format: Some(Format::He {
                        kind: HeKind::SuOrTb,
                        sig_a: Ok(HeSigA {
                            ppdu: HePpdu::Su,
                            bss: Bss {
                                colour: 23,
                                uplink: true,
                                txop_us: None,
                            },
                        }),
                    }),
                }),
                &["HE SU \u{b7} BSS colour 23 \u{b7} uplink \u{b7} no TXOP"],
            ),
            (
                RowKind::Ofdm(Heard {
                    lsig: Some(Err(Refused::Parity)),
                    format: None,
                }),
                &["refused: parity"],
            ),
            (
                RowKind::OtherOfdm { shift_mhz: -5 },
                &["another channel's, -5 MHz away: only its start was read"],
            ),
        ];
        for (kind, wants) in cases {
            let mut m = on_eleven();
            m.net.wifi.push_row(at(0.0, kind));
            captured(&mut m);
            let out = text(&m, 70, 27);
            for want in wants {
                assert!(out.contains(want), "{want} in\n{out}");
            }
        }
    }

    /// Short of rows the channel takes two and the MAC line goes first;
    /// narrow, a field's value wraps at its dots.
    #[test]
    fn it_gives_way_short_and_narrow() {
        let mut m = on_eleven();
        m.net.wifi.push_row(WifiRow {
            channel: Some([0; 52]),
            ..at(0.0, ht())
        });
        captured(&mut m);
        let out = text(&m, 70, 18);
        assert!(!out.contains("payload"), "{out}");
        assert!(out.contains("+0.0 dB"), "{out}");
        let out = text(&m, 54, 27);
        let ht_sig = out.lines().position(|l| l.contains("HT-SIG")).unwrap();
        let next = out.lines().nth(ht_sig + 1).unwrap();
        assert!(next.contains("SGI yes"), "{out}");
    }

    /// `d`: the decode account in place of the selected frame, decode
    /// health's Wi-Fi rows and not the feed's.
    #[test]
    fn d_turns_the_card_to_the_decode_account() {
        let mut m = on_eleven();
        m.net.wifi.push_row(at(0.0, ht()));
        captured(&mut m);
        m.net.health.wifi.this_channel = 1;
        m.net.wifi.frames_view.decode = true;
        let out = text(&m, 70, 27);
        assert!(out.contains("DECODE ACCOUNT"), "{out}");
        assert!(
            out.contains("this channel") && out.contains("802.11b SYNCs"),
            "{out}"
        );
        assert!(
            !out.contains("blocks lost") && !out.contains("SELECTED"),
            "{out}"
        );
    }

    /// Not locked on a channel, the card says NOT LOCKED and how to.
    #[test]
    fn unlocked_the_card_says_how_to_lock_a_channel() {
        let mut m = on_eleven();
        m.net.mode = crate::state::NetMode::Survey;
        let out = text(&m, 70, 27);
        assert!(out.contains("NOT LOCKED"), "{out}");
        assert!(out.contains("Survey"), "{out}");
    }

    /// The silences: not measured, listening, and only other channels.
    #[test]
    fn the_card_says_what_it_cannot_measure() {
        let mut m = on_eleven();
        m.radio.config_sample_rate = 2e6;
        m.net.wifi.refused = crate::signal::net::wifi::rate_refusal(2e6);
        assert!(text(&m, 70, 27).contains("NOT MEASURED"));

        let out = text(&on_eleven(), 70, 27);
        assert!(out.contains("LISTENING"), "{out}");
        assert!(out.contains("no frame of channel 11 yet"), "{out}");

        let mut m = on_eleven();
        m.net
            .wifi
            .push_row(at(0.5, RowKind::OtherOfdm { shift_mhz: 5 }));
        m.net
            .wifi
            .push_row(at(0.0, RowKind::OtherOfdm { shift_mhz: -5 }));
        let out = text(&m, 70, 27);
        assert!(out.contains("ONLY OTHER CHANNELS"), "{out}");
        assert!(out.contains("2 frames"), "{out}");
    }
}
