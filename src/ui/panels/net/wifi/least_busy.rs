// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiLeastBusyPanel` - the survey's answer, and what it rests on.
//!
//! **A channel is named only when the visits carry it.** The quietest
//! channel and every channel within 2 sigma of it (their standard errors
//! combined) are tied; one channel is named when it stands alone, a short
//! run of them when a few do, and when all thirteen tie the card says "not
//! told apart yet" in words and how many more passes would set the first
//! one apart, at the spread a visit has now. Nothing is named before every
//! channel has been visited, nor before each has the three visits a
//! spread needs. No other threshold: the instrument does not know the
//! reader's devices, so it never says "use channel 6".
//!
//! With a channel selected in the survey (or the radio locked on one), the
//! card is that channel's: its busy share and spread, its frames by
//! format, how many were read on its centre and heard there from other
//! channels. What cannot be measured is said as `NOT MEASURED`, with why.
//! Under 36 columns only the headline and its reason are kept.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    Frame,
};

use super::{locked, pct, put, silence};
use crate::state::{Family, LeastBusy, SdrMetrics};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};
use crate::ui::widgets::bigdigits;

pub struct NetWifiLeastBusyPanel;

/// Below this many columns the card keeps its headline and reason only.
const COMPACT: u16 = 36;

/// Words wrapped to `w` columns.
fn wrap(text: &str, w: usize) -> Vec<String> {
    let mut out: Vec<String> = vec![String::new()];
    for word in text.split(' ') {
        let line = out.last_mut().expect("never empty");
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > w {
            out.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    out
}

/// Channels as runs: 1, 2, 3 and 6 is `1-3 6` (or `1 to 3, 6`).
fn runs(channels: &[u8], dash: &str, sep: &str) -> String {
    let mut out: Vec<(u8, u8)> = Vec::new();
    for &c in channels {
        match out.last_mut() {
            Some(r) if r.1 + 1 == c => r.1 = c,
            _ => out.push((c, c)),
        }
    }
    out.iter()
        .map(|(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}{dash}{b}")
            }
        })
        .collect::<Vec<_>>()
        .join(sep)
}

/// Big digits, three rows from `y`.
fn big(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style, area: Rect) {
    let mut cx = x;
    for c in text.chars() {
        let g = bigdigits::glyph(c);
        for (r, row) in g.iter().enumerate() {
            put(buf, cx, y + r as u16, row, style, area);
        }
        cx += bigdigits::GLYPH_W as u16 + 1;
    }
}

/// `1 more pass`, `2 more passes`.
fn passes(n: usize) -> String {
    if n == 1 {
        "1 more pass".to_string()
    } else {
        format!("{n} more passes")
    }
}

/// The headline, the big figure (when there is one) and the reason.
fn verdict(state: &SdrMetrics) -> (String, Option<String>, String) {
    let s = &state.net.wifi.survey;
    // Listening time a pass gives each channel, from the visits so far.
    let visits: usize = s.visits.iter().map(Vec::len).sum();
    let listened: f64 = s.visits.iter().flatten().map(|v| v.seconds).sum();
    let pass_s = if visits > 0 {
        listened / visits as f64 * 13.0
    } else {
        0.0
    };
    match s.least_busy() {
        LeastBusy::FirstPass { visited } => (
            "FIRST PASS".into(),
            None,
            format!("{visited} of 13 channels visited. The least busy is named once every channel has been, and told apart from the rest by more than 2σ."),
        ),
        LeastBusy::NoSpreadYet { passes } => (
            "NO SPREAD YET".into(),
            None,
            format!("A spread needs three visits a channel: {} ({:.0} s of listening).", self::passes(passes), passes as f64 * pass_s),
        ),
        LeastBusy::Alone(ch) => (
            "LEAST BUSY".into(),
            Some(ch.to_string()),
            format!("Channel {ch} stands apart from every other by more than 2σ."),
        ),
        LeastBusy::Few(chs) => (
            "LEAST BUSY".into(),
            Some(runs(&chs, "-", " ")),
            format!("Channels {}: none stands apart from the quietest by more than 2σ.", runs(&chs, " to ", ", ")),
        ),
        LeastBusy::NotApart { passes } => (
            "NOT TOLD APART YET".into(),
            None,
            match passes {
                Some(n) => format!("No channel stands apart from the quietest by more than 2σ. The first would after about {} ({:.0} s of listening).", self::passes(n), n as f64 * pass_s),
                None => "No channel stands apart from the quietest by more than 2σ, and every one reads the same.".into(),
            },
        ),
    }
}

fn draw(buf: &mut Buffer, area: Rect, state: &SdrMetrics, theme: &crate::Theme) {
    let label = Style::default().fg(theme.label);
    let value = Style::default().fg(theme.value);
    let head = Style::default()
        .fg(theme.value_hi)
        .add_modifier(Modifier::BOLD);
    let w = usize::from(area.width.saturating_sub(1));
    let compact = area.width < COMPACT;
    let x = area.x + 1;
    let mut y = area.y + 1;
    let lines = |buf: &mut Buffer, y: &mut u16, text: &str, style: Style| {
        for line in wrap(text, w.saturating_sub(1)) {
            put(buf, x, *y, &line, style, area);
            *y += 1;
        }
    };
    if silence(state).is_some() {
        put(
            buf,
            x,
            y,
            "NOT MEASURED",
            value.add_modifier(Modifier::BOLD),
            area,
        );
        y += 1;
        lines(
            buf,
            &mut y,
            "no channel could be visited, so there is no busy share to give",
            label,
        );
        return;
    }
    let survey = &state.net.wifi.survey;
    let lock = locked(state);
    if let Some(ch) = lock.or(survey.selected) {
        let title = if lock.is_some() {
            "LOCKED ON CHANNEL"
        } else {
            "CHANNEL"
        };
        put(buf, x, y, title, label.add_modifier(Modifier::BOLD), area);
        y += 1;
        let busy = survey.busy(ch);
        if compact {
            put(buf, x, y, &ch.to_string(), head, area);
            y += 1;
        } else {
            big(
                buf,
                x,
                y,
                &ch.to_string(),
                Style::default().fg(theme.value_hi),
                area,
            );
            y += 4;
        }
        let said = match busy {
            Some(b) => match b.sigma {
                Some(s) => format!("{} ± {} % busy", pct(b.mean), pct(s)),
                None => format!("{} % busy, no spread yet", pct(b.mean)),
            },
            None => "not visited yet".to_string(),
        };
        lines(buf, &mut y, &said, value);
        if compact {
            return;
        }
        y += 1;
        let air = survey.airtime(ch);
        for (i, f) in Family::ALL.iter().enumerate() {
            if air[i] > 0.0 {
                put(buf, x, y, "█", Style::default().fg(theme.wifi(*f)), area);
                put(
                    buf,
                    x + 2,
                    y,
                    &format!("{:<11}{:>6} % of its time", f.label(), pct(air[i])),
                    value,
                    area,
                );
                y += 1;
            }
        }
        let visits = &survey.visits[usize::from(ch - 1)];
        let read: u32 = visits.iter().map(|v| v.read).sum();
        let other: u32 = visits.iter().map(|v| v.other).sum();
        let secs: f64 = visits.iter().map(|v| v.seconds).sum();
        lines(
            buf,
            &mut y,
            &format!("{read} frames read on its centre, {other} heard here from other channels"),
            value,
        );
        lines(
            buf,
            &mut y,
            &format!("{} visits, {secs:.1} s", visits.len()),
            label,
        );
        y += 1;
        if lock.is_some() {
            lines(buf, &mut y, "The survey is paused: this channel is live, the others are held at their last pass.", label);
            let kx = put(buf, x, y, "m", head, area);
            put(buf, kx, y, " surveys again", label, area);
        } else {
            let kx = put(buf, x, y, "l", head, area);
            put(buf, kx, y, " locks the radio here", label, area);
        }
        put(
            buf,
            x,
            area.bottom().saturating_sub(1),
            "Network names: after the MAC header",
            label,
            area,
        );
        return;
    }
    let (headline, figure, reason) = verdict(state);
    if !compact {
        put(
            buf,
            x,
            y,
            "LEAST BUSY",
            label.add_modifier(Modifier::BOLD),
            area,
        );
        y += 2;
    }
    match figure {
        Some(f) if !compact => {
            big(buf, x, y, &f, Style::default().fg(theme.value_hi), area);
            y += 4;
        }
        Some(f) => {
            put(buf, x, y, &format!("{headline}: {f}"), head, area);
            y += 1;
        }
        None => {
            put(buf, x, y, &headline, head, area);
            y += 1;
        }
    }
    lines(buf, &mut y, &reason, value);
    if compact {
        return;
    }
    let busiest = (1..=13u8)
        .filter_map(|c| survey.busy(c).map(|b| (c, b)))
        .max_by(|a, b| a.1.mean.total_cmp(&b.1.mean));
    if let Some((c, b)) = busiest {
        y += 1;
        put(
            buf,
            x,
            y,
            "BUSIEST",
            label.add_modifier(Modifier::BOLD),
            area,
        );
        y += 1;
        let spread = b.sigma.map_or(String::new(), |s| format!(" ± {}", pct(s)));
        lines(
            buf,
            &mut y,
            &format!("channel {c}, {}{spread} % busy", pct(b.mean)),
            value,
        );
    }
    // The notes, while there is room for them and the names line.
    let visits: usize = survey.visits.iter().map(Vec::len).sum();
    let first = survey.visits.iter().find(|v| !v.is_empty());
    let per: f64 = first.map_or(0.0, |v| v.iter().map(|x| x.seconds).sum());
    let notes = [
        format!("{visits} visits so far, {per:.1} s a channel: a sample of the band, not a watch on it. ± is the spread between visits, 1σ."),
        "Busy: the busiest megahertz of a channel's 20, any transmitter. Frames: the airtime stated by frames read on its centre.".to_string(),
    ];
    for note in notes {
        let need = wrap(&note, w.saturating_sub(1)).len() as u16 + 1;
        if y + need + 2 > area.bottom() {
            break;
        }
        y += 1;
        lines(buf, &mut y, &note, label);
    }
    put(
        buf,
        x,
        area.bottom().saturating_sub(1),
        "Network names: after the MAC header",
        label,
        area,
    );
}

impl Panel for NetWifiLeastBusyPanel {
    fn name(&self) -> &'static str {
        "net_wifi_least_busy"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (20, 8)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Least busy")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .counts_from_feed(FeedSpan::Session)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if inner.width < 12 || inner.height < 4 {
            return;
        }
        draw(f.buffer_mut(), inner, state, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw as drawn;
    use crate::state::{NetMode, Visit, WifiSurvey};

    /// Every channel visited `n` times at `busy` (channel 1 first), each
    /// visit half a second, channel 11's frames DSSS at 18 %.
    fn survey(busy: [f64; 13], n: usize) -> WifiSurvey {
        let mut s = WifiSurvey::default();
        for (i, b) in busy.iter().enumerate() {
            s.visits[i] = (0..n)
                .map(|k| {
                    let mut v = Visit {
                        seconds: 0.5,
                        // A little spread, so a sigma exists.
                        busy: b * (1.0 + 0.1 * (k as f64 - 1.0)),
                        read: 3,
                        other: 1,
                        ..Default::default()
                    };
                    if i == 10 {
                        v.airtime[5] = 0.18;
                    }
                    v
                })
                .collect();
        }
        s
    }

    fn state(s: WifiSurvey) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.radio.config_sample_rate = 20e6;
        m.net.wifi.survey = s;
        m
    }

    const STEADY: [f64; 13] = [
        0.3, 0.3, 0.3, 0.3, 0.3, 0.01, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3,
    ];

    /// One channel apart: named in big digits, with what sets it apart.
    #[test]
    fn a_channel_apart_is_named() {
        let text = drawn(NetWifiLeastBusyPanel, 46, 25, &state(survey(STEADY, 6))).join("\n");
        assert!(text.contains("LEAST BUSY"), "{text}");
        assert!(text.contains("Channel 6 stands apart"), "{text}");
        assert!(
            text.contains("BUSIEST") && text.contains("% busy"),
            "{text}"
        );
        assert!(
            text.contains("Network names: after the MAC header"),
            "{text}"
        );
    }

    /// A few tied with the quietest: their runs, in words and digits.
    #[test]
    fn a_few_tied_are_named_as_runs() {
        let mut busy = STEADY;
        busy[0] = 0.01;
        busy[1] = 0.01;
        busy[2] = 0.01;
        let text = drawn(NetWifiLeastBusyPanel, 46, 25, &state(survey(busy, 6))).join("\n");
        assert!(text.contains("Channels 1 to 3, 6"), "{text}");
    }

    /// All thirteen tied: words, and the passes that would tell the first
    /// apart; before three visits, the spread still to come; before every
    /// channel, the first pass.
    #[test]
    fn what_cannot_be_named_yet_is_said_in_words() {
        let mut bursty = WifiSurvey::default();
        for i in 0..13 {
            bursty.visits[i] = [0.0, 0.6, 0.0, 0.0, 0.0, 0.0]
                .iter()
                .map(|b| Visit {
                    seconds: 0.5,
                    busy: *b * (i as f64 / 12.0),
                    ..Default::default()
                })
                .collect();
        }
        // Wide, so the sentences are on one line each.
        let text = drawn(NetWifiLeastBusyPanel, 160, 25, &state(bursty)).join("\n");
        assert!(text.contains("NOT TOLD APART YET"), "{text}");
        assert!(text.contains("more passes ("), "{text}");
        let two = drawn(NetWifiLeastBusyPanel, 160, 25, &state(survey(STEADY, 2))).join("\n");
        assert!(
            two.contains("NO SPREAD YET") && two.contains("1 more pass ("),
            "{two}"
        );
        let mut first = survey(STEADY, 1);
        first.visits[12].clear();
        let text = drawn(NetWifiLeastBusyPanel, 46, 25, &state(first)).join("\n");
        assert!(
            text.contains("FIRST PASS") && text.contains("12 of 13"),
            "{text}"
        );
    }

    /// Selected, the card is that channel's: busy with its spread, frames
    /// by format, counts, and the key that locks there; locked, it is the
    /// locked channel's and says how to survey again.
    #[test]
    fn a_selected_or_locked_channel_has_the_card() {
        let mut s = survey(STEADY, 6);
        s.selected = Some(11);
        let text = drawn(NetWifiLeastBusyPanel, 46, 25, &state(s)).join("\n");
        assert!(text.contains("CHANNEL") && text.contains("± "), "{text}");
        assert!(
            text.contains("DSSS") && text.contains("18.0 % of its time"),
            "{text}"
        );
        assert!(
            text.contains("18 frames read on its centre, 6 heard here"),
            "{text}"
        );
        assert!(text.contains("locks the radio here"), "{text}");
        let mut m = state(survey(STEADY, 6));
        m.net.mode = NetMode::Lock;
        m.radio.frequency = 2_437_000_000;
        let text = drawn(NetWifiLeastBusyPanel, 46, 25, &m).join("\n");
        assert!(
            text.contains("LOCKED ON CHANNEL") && text.contains("surveys again"),
            "{text}"
        );
    }

    /// Nothing measured: said in words, no figure.
    #[test]
    fn nothing_measured_is_said() {
        let mut m = state(survey(STEADY, 6));
        m.radio.hw_streaming = false;
        let text = drawn(NetWifiLeastBusyPanel, 46, 25, &m).join("\n");
        assert!(text.contains("NOT MEASURED"), "{text}");
        assert!(!text.contains("BUSIEST"), "{text}");
    }

    /// Narrow, the card keeps its headline and reason: the channel named
    /// on one line, no notes.
    #[test]
    fn narrow_it_keeps_the_headline_and_its_reason() {
        let text = drawn(NetWifiLeastBusyPanel, 25, 15, &state(survey(STEADY, 6))).join("\n");
        assert!(text.contains("LEAST BUSY: 6"), "{text}");
        assert!(!text.contains("BUSIEST"), "{text}");
        for (w, h) in [(46u16, 25u16), (40, 18), (25, 15), (20, 8)] {
            let lines = drawn(NetWifiLeastBusyPanel, w, h, &state(survey(STEADY, 6)));
            assert!(
                lines.iter().all(|l| l.chars().count() <= usize::from(w)),
                "{w}x{h}"
            );
        }
    }
}
