// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiFramesPanel` - what is on this channel, frame by frame: the
//! frames heard on its centre, newest first, each with what was read of it
//! and nothing more.
//!
//! **A row says what was read.** Its format by its signalling; its rate and
//! length only where a field read today states them, so a later format's
//! L-SIG (6 Mb/s whatever its own rate, a placeholder that spoofs the
//! frame's airtime) is never shown as its rate; its airtime where it was
//! stated; the gap from the end of the frame before, unknown after a frame
//! that stated no length and across a break; its offset with its 1 sigma
//! and its SNR. Another channel's frame is named by how far away it is.
//!
//! **Newest first, held at a frame** as the Classic list is: live, the
//! newest frame is the selected one; held, the list draws from one frame
//! down while newer ones arrive above, and the cursor moves through it.
//! The ring keeps what it keeps, and the title says how far back that
//! reaches, so a held list never passes for the whole story.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::wifi::format::{Format, Heard};
use crate::signal::wifi::hesig::{HeKind, HePpdu};
use crate::state::{FramesView, RowKind, SdrMetrics, WifiRow, WIFI_ROWS_KEPT};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness, Tag};

pub struct NetWifiFramesPanel;

/// Width the offset's 1 sigma needs, and the length's.
const FULL: usize = 110;
const MID: usize = 94;

/// The frames heard on the radio's centre, newest first. The survey's
/// visits to other channels leave their frames in the ring too; they are
/// those channels' lists, not this one's.
pub fn shown(state: &SdrMetrics) -> Vec<&WifiRow> {
    let centre = state.radio.frequency as f64;
    state
        .net
        .wifi
        .rows
        .iter()
        .rev()
        .filter(|r| (r.carrier_hz - centre).abs() < 1.0)
        .collect()
}

/// Where the list starts in what it shows: the newest while live, the held
/// frame while held, which is also how many arrived since. `None` when the
/// held frame has left the ring.
pub(super) fn top(shown: &[&WifiRow], view: &FramesView) -> Option<usize> {
    match view.held {
        None => Some(0),
        Some(seq) => shown.iter().position(|r| r.seq == seq),
    }
}

/// Where the selected frame is in what is shown: the newest while live.
pub(super) fn at(shown: &[&WifiRow], view: &FramesView) -> Option<usize> {
    let top = top(shown, view)?;
    let cursor = if view.held.is_some() { view.cursor } else { 0 };
    (top < shown.len()).then(|| (top + cursor).min(shown.len() - 1))
}

/// Rows below the held frame (or the newest), as far as the cursor goes.
pub fn below(state: &SdrMetrics) -> usize {
    let shown = shown(state);
    top(&shown, &state.net.wifi.frames_view).map_or(0, |t| shown.len().saturating_sub(t + 1))
}

/// From `newest` to `row`, in microseconds: on the stream's own clock
/// within one run, by when each was judged across a break.
pub(super) fn since(row: &WifiRow, newest: &WifiRow) -> f64 {
    if row.run == newest.run {
        (row.start as f64 - newest.start as f64) / 20.0
    } else {
        -newest
            .when
            .saturating_duration_since(row.when)
            .as_secs_f64()
            * 1e6
    }
}

pub(super) fn when(us: f64) -> String {
    if us.abs() < 1e6 {
        format!("{:>9.3} ms", us / 1e3)
    } else {
        format!("{:>9.3} s ", us / 1e6)
    }
}

fn gap(g: Option<f64>) -> String {
    match g {
        None => "\u{b7}".to_string(),
        Some(g) if g.abs() < 1_000.0 => format!("{g:.1}"),
        Some(g) => format!("{:.1} ms", g / 1e3),
    }
}

/// A format's name, as its signalling said it.
fn format_name(heard: &Heard) -> &'static str {
    match heard.format {
        None => "unreadable",
        Some(Format::NonHt) => "non-HT",
        Some(Format::HtMixed(_)) => "HT",
        Some(Format::Greenfield(Ok(_))) => "HT GF",
        Some(Format::Greenfield(Err(_))) => "unreadable",
        Some(Format::Vht(_)) => "VHT",
        Some(Format::He { sig_a: Ok(a), .. }) => match a.ppdu {
            HePpdu::Su => "HE SU",
            HePpdu::ErSu => "HE ER SU",
            HePpdu::Mu => "HE MU",
            HePpdu::Tb => "HE TB",
        },
        Some(Format::He { kind, .. }) => match kind {
            HeKind::SuOrTb => "HE",
            HeKind::Mu => "HE MU",
            HeKind::ErSu => "HE ER SU",
        },
        Some(Format::USig { sig: Ok(u), .. }) if u.version == 0 => "EHT",
        Some(Format::USig { sig: Ok(_), .. }) => "after EHT",
        Some(Format::USig { sig: Err(_), .. }) => "unreadable",
    }
}

/// What a row's columns say: its format's name, its rate, its length and,
/// for HE and later, its network.
fn said(kind: &RowKind) -> (String, String, String, String) {
    let preamble = |p| match p {
        crate::signal::wifi::dsss::header::Preamble::Long => "long",
        crate::signal::wifi::dsss::header::Preamble::Short => "short",
    };
    let dsss = |header: &Result<_, _>| match header {
        Ok(h) => {
            let h: &crate::signal::wifi::dsss::header::Header = h;
            (
                format!("{} Mb/s", h.rate.mbps()),
                format!("{} B", h.octets()),
            )
        }
        Err(_) => (String::new(), String::new()),
    };
    let network = |bss: &crate::signal::wifi::hesig::Bss| {
        format!(
            "colour {} {}",
            bss.colour,
            if bss.uplink { "\u{2191}" } else { "\u{2193}" }
        )
    };
    match kind {
        RowKind::Ofdm(heard) => {
            let (rate, length) = match (heard.format, heard.lsig) {
                (Some(Format::NonHt), Some(Ok(l))) => {
                    (format!("{} Mb/s", l.rate.mbps()), format!("{} B", l.length))
                }
                (Some(Format::HtMixed(Ok(h)) | Format::Greenfield(Ok(h))), _) => {
                    (format!("MCS {}", h.mcs), format!("{} B", h.length))
                }
                _ => (String::new(), String::new()),
            };
            let extra = match heard.format {
                Some(Format::He { sig_a: Ok(a), .. }) => network(&a.bss),
                Some(Format::USig { sig: Ok(u), .. }) => network(&u.bss),
                _ => String::new(),
            };
            (format_name(heard).to_string(), rate, length, extra)
        }
        RowKind::Dsss {
            preamble: p,
            header,
        } => {
            let (rate, length) = dsss(header);
            (format!("11b {}", preamble(*p)), rate, length, String::new())
        }
        RowKind::OtherOfdm { shift_mhz } => (
            format!("{shift_mhz:+} MHz \u{b7} OFDM"),
            String::new(),
            String::new(),
            String::new(),
        ),
        RowKind::OtherDsss {
            shift_mhz, header, ..
        } => {
            let (rate, length) = dsss(header);
            (
                format!("{shift_mhz:+} MHz \u{b7} 11b"),
                rate,
                length,
                String::new(),
            )
        }
    }
}

/// The SNR's height in eighths over the river's fixed 0 to 24 dB, never
/// below one, so the row and the river draw one frame alike.
fn eighth(snr_db: f64) -> char {
    const EIGHTHS: [char; 8] = [
        '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];
    let level = ((snr_db / 24.0 * 8.0).ceil() as i64).clamp(1, 8) as usize;
    EIGHTHS[level - 1]
}

/// A row's colour: its family's, or the other channels' quiet one.
pub(super) fn ink(row: &WifiRow, theme: &crate::Theme) -> Color {
    row.family()
        .map_or(theme.wifi_other_channel, |f| theme.wifi(f))
}

fn header(width: usize) -> String {
    let mut s = format!(
        " {:<13}{:>8}  {:<17}{:<10}",
        "when", "gap \u{b5}s", "format", "rate"
    );
    if width >= MID {
        s.push_str(&format!("{:>8}", "length"));
    }
    let off = if width >= FULL { 17 } else { 10 };
    s.push_str(&format!("{:>9}  {:<off$}SNR", "airtime", "offset"));
    s
}

fn row_line(
    row: &WifiRow,
    newest: &WifiRow,
    selected: bool,
    width: usize,
    state: &SdrMetrics,
    now: std::time::Instant,
    theme: &crate::Theme,
) -> Line<'static> {
    let base = if selected {
        Style::default()
            .fg(theme.value_hi)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.value)
    };
    let quiet = Style::default().fg(theme.label);
    let colour = Style::default().fg(ink(row, theme));
    let (name, rate, length, extra) = said(&row.kind);
    let other = row.family().is_none();
    let mut spans = vec![
        Span::styled(
            if selected { "\u{258c}" } else { " " },
            Style::default().fg(theme.border_accent),
        ),
        Span::styled(format!("{:<13}", when(since(row, newest))), quiet),
        Span::styled(format!("{:>8}  ", gap(row.gap_us)), base),
        Span::styled(
            if other {
                "\u{258c} "
            } else {
                "\u{2586}\u{2586}"
            },
            colour,
        ),
        Span::styled(format!(" {name:<14}{rate:<10}"), base),
    ];
    if width >= MID {
        spans.push(Span::styled(format!("{length:>8}"), base));
    }
    let air = row
        .airtime_us
        .map_or("\u{b7}".to_string(), |us| format!("{us} \u{b5}s"));
    let off_w = if width >= FULL { 17 } else { 10 };
    let off = row.offset_hz.map_or(String::new(), |o| {
        let t = state.radio.transmitter_offset(o, row.carrier_hz, now).khz;
        if width >= FULL {
            format!("{:+.1} \u{b1}{:.1} kHz", t.value(), t.sigma())
        } else {
            format!("{:+.1} kHz", t.value())
        }
    });
    spans.push(Span::styled(format!("{air:>9}  {off:<off_w$}"), base));
    match row.snr_db {
        Some(snr) => {
            spans.push(Span::styled(format!("{snr:>5.1} dB "), base));
            spans.push(Span::styled(eighth(snr).to_string(), colour));
        }
        None => spans.push(Span::styled(format!("{:>5}    ", "\u{b7}"), quiet)),
    }
    if width >= FULL && !extra.is_empty() {
        spans.push(Span::styled(format!("  {extra}"), quiet));
    }
    Line::from(spans)
}

fn keys(width: usize) -> &'static str {
    if width >= FULL {
        " \u{2191}\u{2193} scroll, holding at the newest   H hold   End live   z zoom   d the decode account"
    } else {
        " \u{2191}\u{2193} scroll   H hold   End live   z zoom   d the decode account"
    }
}

fn lines(
    state: &SdrMetrics,
    width: usize,
    height: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let quiet = Style::default().fg(theme.label);
    let note = |s: String| Line::from(Span::styled(s, Style::default().fg(theme.stale)));
    let mut out = vec![Line::from(Span::styled(header(width), quiet))];
    let shown = shown(state);
    let view = &state.net.wifi.frames_view;
    let silent = super::silence(state);
    if let Some(s) = &silent {
        out.push(note(if shown.is_empty() {
            format!(" nothing can be heard: {}", s.why)
        } else {
            format!(
                " nothing can be heard now: {}; the frames below are from before",
                s.why
            )
        }));
    }
    if shown.is_empty() {
        if silent.is_none() {
            let ch = crate::signal::net::band::wifi_channel(state.radio.frequency)
                .map_or("this channel".to_string(), |c| format!("channel {c}"));
            out.push(note(format!(" nothing heard on {ch} yet")));
        }
        return out;
    }
    let Some(top) = top(&shown, view) else {
        out.push(note(format!(
            " the held frame has left the {WIFI_ROWS_KEPT} kept; End: back to live"
        )));
        return out;
    };
    let selected = at(&shown, view).unwrap_or(top);
    let body = height.saturating_sub(out.len() + 1).max(1);
    // The window scrolls to keep the cursor in it.
    let from = top + (selected - top).saturating_sub(body - 1);
    let now = std::time::Instant::now();
    let newest = shown[top];
    for (i, row) in shown.iter().enumerate().skip(from).take(body) {
        out.push(row_line(
            row,
            newest,
            i == selected,
            width,
            state,
            now,
            theme,
        ));
    }
    while out.len() + 1 < height {
        out.push(Line::raw(""));
    }
    out.push(Line::from(Span::styled(keys(width), quiet)));
    out
}

impl Panel for NetWifiFramesPanel {
    fn name(&self) -> &'static str {
        "net_wifi_frames"
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 5)
    }

    fn focus_key(&self) -> Option<char> {
        // `f` is the frequency everywhere; `e` is a Bluetooth list's letter,
        // and no Bluetooth panel shares a screen with a Wi-Fi one.
        Some('e')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("\u{2191}\u{2193}", "scroll, holding the list at its newest"),
            ("H", "hold the list, or let it run"),
            ("End", "back to live"),
            (
                "z",
                "the river's zoom: 1 ms, 40 \u{b5}s or 4 \u{b5}s a cell",
            ),
            ("d", "the card: the selected frame, or the decode account"),
        ]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let shown = shown(state);
        let view = &state.net.wifi.frames_view;
        let mut chrome = PanelChrome::new("Fram_es")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .counts_from_feed(FeedSpan::Session);
        if let (Some(newest), Some(oldest)) = (shown.first(), shown.last()) {
            let span = newest.when.saturating_duration_since(oldest.when);
            chrome.suffix = Some(format!(
                " \u{b7} {} kept \u{b7} {:.1} s",
                shown.len(),
                span.as_secs_f64()
            ));
        }
        if shown.iter().any(|r| r.offset_hz.is_some()) {
            chrome = chrome.shows_offsets();
        }
        // Held, the list is paused by the user and says what the pause is
        // costing.
        let behind = top(&shown, view).unwrap_or(shown.len()) as u64;
        chrome
            .tag_if(view.held.is_some(), Tag::Paused)
            .tag_if(view.held.is_some() && behind > 0, Tag::Behind(behind))
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
    use super::NetWifiFramesPanel;
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::signal::wifi::dsss::header::{Header, Preamble, Rate as DRate};
    use crate::signal::wifi::format::{Format, Heard};
    use crate::signal::wifi::hesig::{Bss, HeKind, HePpdu, HeSigA};
    use crate::signal::wifi::htsig::HtSig;
    use crate::signal::wifi::lsig::{LSig, Refused};
    use crate::signal::wifi::signal_field::Rate;
    use crate::state::fixture::draw;
    use crate::state::{RowKind, SdrMetrics, WifiRow};

    /// The Frames view on channel 11 at 20 Msps, streaming.
    fn on_eleven() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "wifi_channel".to_string();
        m.ui.active_preset = "wifi_frames".to_string();
        m.radio.frequency = 2_462_000_000;
        m.radio.config_sample_rate = 20e6;
        m.net.wifi.ran = true;
        m.net.wifi.channel = Some(11);
        m
    }

    /// A frame `start_us` into the run, heard on channel 11's centre.
    fn row(start_us: f64, kind: RowKind, airtime_us: Option<u32>) -> WifiRow {
        WifiRow {
            seq: 0,
            start: (start_us * 20.0) as u64,
            run: 0,
            when: std::time::Instant::now(),
            carrier_hz: 2_462e6,
            kind,
            airtime_us,
            gap_us: None,
            offset_hz: Some(Uncertain::from_sigma(-48_000.0, 200.0)),
            snr_db: Some(27.0),
            channel: None,
            clock: None,
        }
    }

    fn lsig(rate: Rate, length: u16) -> Option<Result<LSig, Refused>> {
        Some(Ok(LSig { rate, length }))
    }

    fn non_ht(rate: Rate, length: u16) -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: lsig(rate, length),
            format: Some(Format::NonHt),
        })
    }

    fn ht(mcs: u8, length: u16) -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: lsig(Rate::R6, 2_000),
            format: Some(Format::HtMixed(Ok(HtSig {
                mcs,
                cbw40: false,
                length,
                stbc: 0,
                ldpc: false,
                short_gi: true,
                ness: 0,
            }))),
        })
    }

    fn he_su(colour: u8, uplink: bool) -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: lsig(Rate::R6, 300),
            format: Some(Format::He {
                kind: HeKind::SuOrTb,
                sig_a: Ok(HeSigA {
                    ppdu: HePpdu::Su,
                    bss: Bss {
                        colour,
                        uplink,
                        txop_us: None,
                    },
                }),
            }),
        })
    }

    fn header(rate: DRate, length_us: u16) -> Header {
        Header {
            rate,
            locked: true,
            extension: false,
            length_us,
        }
    }

    fn dsss(length_us: u16) -> RowKind {
        RowKind::Dsss {
            preamble: Preamble::Long,
            header: Ok(header(DRate::M1, length_us)),
        }
    }

    fn unreadable() -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: Some(Err(Refused::Parity)),
            format: None,
        })
    }

    fn text(m: &SdrMetrics, w: u16, h: u16) -> String {
        draw(NetWifiFramesPanel, w, h, m).join("\n")
    }

    /// The line that says `what`.
    fn line<'a>(out: &'a str, what: &str) -> &'a str {
        out.lines()
            .find(|l| l.contains(what))
            .unwrap_or_else(|| panic!("no line with {what} in\n{out}"))
    }

    /// The rows under the column titles, newest first.
    fn rows(out: &str) -> Vec<&str> {
        out.lines()
            .skip_while(|l| !l.contains("gap \u{b5}s"))
            .skip(1)
            .filter(|l| l.contains(" dB") || l.contains(" MHz \u{b7} "))
            .collect()
    }

    /// **Each frame says what was read, and only that.** A frame's format
    /// by its signalling, its rate and length where a field read today
    /// states them (never L-SIG's placeholder for a later format), its
    /// airtime where stated, its offset with 1 sigma and its SNR; HE with
    /// its BSS colour and direction; another channel's frame by its shift.
    #[test]
    fn every_kind_of_frame_reads_as_its_signalling_said() {
        let mut m = on_eleven();
        let w = &mut m.net.wifi;
        w.push_row(row(0.0, non_ht(Rate::R24, 20), Some(28)));
        w.push_row(row(44.0, ht(7, 1_500), Some(200)));
        w.push_row(row(1_000.0, he_su(23, true), Some(120)));
        w.push_row(row(2_000.0, dsss(304), Some(496)));
        // Nothing another channel's OFDM frame said was read: only where it
        // began, as the worker keeps it.
        w.push_row(WifiRow {
            offset_hz: None,
            snr_db: None,
            ..row(3_000.0, RowKind::OtherOfdm { shift_mhz: -5 }, None)
        });
        w.push_row(row(
            4_000.0,
            RowKind::OtherDsss {
                shift_mhz: 10,
                preamble: Preamble::Short,
                header: Ok(header(DRate::M2, 100)),
            },
            Some(196),
        ));
        w.push_row(row(5_000.0, unreadable(), None));
        let out = text(&m, 191, 16);
        assert!(out.contains("Frames"), "the nameplate: {out}");

        let r = line(&out, "non-HT");
        for want in [
            "24 Mb/s",
            "20 B",
            "28 \u{b5}s",
            "48.0 \u{b1}0.2 kHz",
            "27.0 dB",
        ] {
            assert!(r.contains(want), "{want} in {r}");
        }
        let r = line(&out, "MCS 7");
        assert!(r.contains(" HT ") && r.contains("1500 B"), "{r}");
        assert!(r.contains("16.0"), "a SIFS after the RTS: {r}");
        assert!(!r.contains("Mb/s"), "L-SIG's 6 Mb/s is not HT's rate: {r}");
        let r = line(&out, "HE SU");
        assert!(r.contains("colour 23 \u{2191}"), "{r}");
        assert!(!r.contains("Mb/s") && !r.contains(" B "), "{r}");
        let r = line(&out, "11b long");
        assert!(r.contains("1 Mb/s") && r.contains("38 B"), "{r}");
        let r = line(&out, "-5 MHz \u{b7} OFDM");
        assert!(!r.contains("\u{b5}s ") && !r.contains("kHz"), "{r}");
        let r = line(&out, "+10 MHz \u{b7} 11b");
        assert!(r.contains("2 Mb/s") && r.contains("196 \u{b5}s"), "{r}");
        assert!(line(&out, "unreadable").contains("\u{b7}"), "{out}");
    }

    /// **Newest first, the newest selected while live**: its time is the
    /// list's zero and the others count back from it.
    #[test]
    fn the_newest_frame_is_on_top_and_selected() {
        let mut m = on_eleven();
        m.net
            .wifi
            .push_row(row(0.0, non_ht(Rate::R24, 20), Some(28)));
        m.net
            .wifi
            .push_row(row(64.0, non_ht(Rate::R6, 14), Some(44)));
        let out = text(&m, 191, 16);
        let r = rows(&out);
        assert_eq!(r.len(), 2, "{out}");
        assert!(
            r[0].contains("0.000 ms") && r[0].contains("6 Mb/s"),
            "{out}"
        );
        assert!(
            r[1].contains("-0.064 ms") && r[1].contains("24 Mb/s"),
            "{out}"
        );
        assert!(
            r[0].contains('\u{258c}'),
            "the selected row is marked: {out}"
        );
        assert!(!r[1].contains('\u{258c}'), "{out}");
    }

    /// The gap is unknown after a frame that stated no length, and written
    /// in milliseconds past one.
    #[test]
    fn a_gap_is_unknown_after_no_length_and_in_ms_past_one() {
        let mut m = on_eleven();
        let w = &mut m.net.wifi;
        w.push_row(row(0.0, non_ht(Rate::R24, 20), Some(28)));
        w.push_row(row(2_528.0, non_ht(Rate::R6, 14), Some(44)));
        w.push_row(row(3_000.0, RowKind::OtherOfdm { shift_mhz: 5 }, None));
        w.push_row(row(3_100.0, non_ht(Rate::R12, 30), Some(40)));
        let out = text(&m, 191, 16);
        let r = rows(&out);
        assert!(r[2].contains("2.5 ms"), "{out}");
        let gap = r[0].split("ms").nth(1).unwrap().trim_start();
        assert!(gap.starts_with('\u{b7}'), "after a start alone: {out}");
    }

    /// **What gives way as it narrows**: first the offset's sigma, then
    /// the length; the rest stays.
    #[test]
    fn the_columns_give_way_as_the_list_narrows() {
        let mut m = on_eleven();
        m.net.wifi.push_row(row(0.0, ht(7, 1_500), Some(200)));
        let out = text(&m, 98, 10);
        let r = line(&out, "MCS 7");
        assert!(!r.contains('\u{b1}') && r.contains("48.0 kHz"), "{r}");
        assert!(r.contains("1500 B") && r.contains("27.0 dB"), "{r}");
        let out = text(&m, 90, 10);
        let r = line(&out, "MCS 7");
        assert!(!r.contains("1500 B"), "{r}");
        assert!(r.contains("200 \u{b5}s") && r.contains("27.0 dB"), "{r}");
    }

    /// **Held, the list stays at its frame** and says how many arrived
    /// since; the held frame gone from the ring is said, not guessed at.
    #[test]
    fn a_held_list_stays_at_its_frame() {
        let mut m = on_eleven();
        for i in 0..3 {
            m.net
                .wifi
                .push_row(row(100.0 * i as f64, non_ht(Rate::R6, 10 + i), Some(40)));
        }
        m.net.wifi.frames_view.held = Some(2);
        let out = text(&m, 191, 16);
        let r = rows(&out);
        assert_eq!(r.len(), 2, "{out}");
        assert!(r[0].contains("11 B") && r[0].contains("0.000 ms"), "{out}");
        assert!(out.contains("HELD") || out.contains("PAUSED"), "{out}");
        assert!(out.contains('1'), "{out}");

        m.net.wifi.frames_view.held = Some(99);
        let out = text(&m, 191, 16);
        assert!(
            out.contains("the held frame has left the 2048 kept; End: back to live"),
            "{out}"
        );
    }

    /// The cursor selects a frame further down, and the rows follow it.
    #[test]
    fn the_window_follows_the_cursor() {
        let mut m = on_eleven();
        for i in 0..30u16 {
            m.net.wifi.push_row(row(
                100.0 * f64::from(i),
                non_ht(Rate::R6, 100 + i),
                Some(40),
            ));
        }
        m.net.wifi.frames_view.held = Some(30);
        m.net.wifi.frames_view.cursor = 20;
        let out = text(&m, 191, 16);
        let marked: Vec<&str> = rows(&out)
            .into_iter()
            .filter(|l| l.contains('\u{258c}'))
            .collect();
        assert_eq!(marked.len(), 1, "{out}");
        assert!(marked[0].contains("109 B"), "the 21st newest: {out}");
    }

    /// Frames heard on another channel's centre (the survey's visits) are
    /// not this channel's list.
    #[test]
    fn only_frames_heard_on_this_centre_are_listed() {
        let mut m = on_eleven();
        m.net.wifi.push_row(WifiRow {
            carrier_hz: 2_437e6,
            ..row(0.0, non_ht(Rate::R54, 99), Some(30))
        });
        m.net
            .wifi
            .push_row(row(100.0, non_ht(Rate::R6, 14), Some(44)));
        let out = text(&m, 191, 16);
        assert_eq!(rows(&out).len(), 1, "{out}");
        assert!(!out.contains("54 Mb/s"), "{out}");
    }

    /// **Listening**: nothing of this channel yet is said in words, never
    /// an empty list that would read as a quiet channel.
    #[test]
    fn listening_says_nothing_has_been_heard_yet() {
        let out = text(&on_eleven(), 191, 16);
        assert!(out.contains("nothing heard on channel 11 yet"), "{out}");
    }

    /// **Not measured**: why nothing can be heard, from the shared Wi-Fi
    /// silence; frames kept from before are still listed, and said to be.
    #[test]
    fn when_nothing_can_be_heard_it_says_why() {
        let mut m = on_eleven();
        m.radio.config_sample_rate = 2e6;
        m.net.wifi.refused = crate::signal::net::wifi::rate_refusal(2e6);
        let out = text(&m, 191, 16);
        assert!(out.contains("nothing can be heard"), "{out}");
        assert!(out.contains("20 Msps") || out.contains("20.000"), "{out}");

        m.net
            .wifi
            .push_row(row(0.0, non_ht(Rate::R6, 14), Some(44)));
        let out = text(&m, 191, 16);
        assert!(out.contains("from before"), "{out}");
        assert_eq!(rows(&out).len(), 1, "{out}");
    }
}
