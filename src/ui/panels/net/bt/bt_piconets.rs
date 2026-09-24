// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBtPiconetsPanel` - the classic preset's roster: one row per piconet
//! heard, and the selected one spelled out below (net-ux-polish-plan 6.1).
//!
//! **A roster of piconets, not of devices.** A LAP is the master's lower
//! address part and every member of its piconet sends it, so the title and
//! every label say piconet (`signal::bt::piconet` has the reasoning).
//!
//! **The UAP column says how far the narrowing got**, in the words
//! `net_bt_hops` already uses: one value once it is down to one, the number
//! of candidates left while a header alone cannot choose
//! (`signal::bt::header::PiconetClock` has the measured floor of two), and a
//! dash before any header of it has been decoded. The detail block says
//! which of those it is in a sentence.
//!
//! Replaces `net_bt_census`, which only ever said that nothing was decoded.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::bt::piconet::{ordered, Inquiry, Piconet, DCI};
use crate::signal::dsp::uncertainty::Uncertain;
use crate::state::SdrMetrics;
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};
use crate::ui::widgets::limit::{Limit, LimitRow, RowWidths};
use crate::ui::widgets::reading::Reading;
use crate::ui::widgets::table::{
    columns_that_fit, header, row, viewport_start, Align, Column, Sort,
};

pub struct NetBtPiconetsPanel;

const COLUMNS: &[Column] = &[
    Column {
        title: "LAP",
        // `● 0x5a3c71`: the hop panel's colour chip, then 24 bits.
        width: 10,
        align: Align::Left,
    },
    Column {
        title: "LAST",
        width: 6,
        align: Align::Right,
    },
    Column {
        title: "HITS",
        width: 6,
        align: Align::Right,
    },
    Column {
        title: "CH",
        width: 3,
        align: Align::Right,
    },
    Column {
        title: "UAP",
        // `32 left`: one header leaves 32 (`header::PiconetClock`).
        width: 7,
        align: Align::Right,
    },
    Column {
        title: "FIRST",
        width: 6,
        align: Align::Right,
    },
];

/// The column the roster is ordered by: the most recently heard first.
const ORDERED_BY: usize = 1;

/// The colour chip a piconet wears here and on the hop panel: solid, so the
/// colour carries in any font (a braille block drew as faint dots).
pub(crate) const CHIP: char = '\u{25cf}';

/// Rows the table keeps before the detail block may take any.
const TABLE_KEEPS: usize = 3;

/// Label width in the detail block.
const LABEL_W: usize = 9;

/// The same shape the census uses: a bench glances, it does not time.
fn ago(secs: u64) -> String {
    if secs < 90 {
        format!("{secs} s")
    } else {
        format!("{} min", secs / 60)
    }
}

/// The UAP cell: one value, candidates left, or a dash before any header.
fn uap_cell(uaps: Option<&Vec<u8>>) -> String {
    match uaps.map(|u| u.as_slice()) {
        Some([one]) => format!("{one:#04x}"),
        Some(many) if !many.is_empty() => format!("{} left", many.len()),
        _ => "-".to_string(),
    }
}

/// The UAP as a sentence, for the detail block.
fn uap_sentence(uaps: Option<&Vec<u8>>) -> String {
    match uaps.map(|u| u.as_slice()) {
        Some([one]) => format!("{one:#04x}"),
        // Listed while a reader can take them in; a first header leaves 32,
        // and 32 values are a wall, not a reading.
        Some(many) if (2..=4).contains(&many.len()) => format!(
            "{} candidates ({}); a header alone does not choose",
            many.len(),
            many.iter()
                .map(|u| format!("{u:#04x}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Some(many) if !many.is_empty() => format!(
            "{} candidates; each further header narrows them",
            many.len()
        ),
        _ => "not narrowed: no header of it decoded yet".to_string(),
    }
}

/// A LAP as the roster and the hop lanes name it: an inquiry code by its
/// abbreviation, since a searching device is not a piconet and its bits
/// are not a master's address; every other LAP in hex.
pub(crate) fn lap_name(lap: u32) -> String {
    match Inquiry::of(lap) {
        Some(i) => format!("{:<8}", i.short()),
        None => format!("{lap:#08x}"),
    }
}

fn cells(p: &Piconet, state: &SdrMetrics, now: std::time::Instant) -> Vec<String> {
    let since = |t: std::time::Instant| ago(now.saturating_duration_since(t).as_secs());
    vec![
        format!("{CHIP} {}", lap_name(p.lap)),
        since(p.last_seen),
        p.hits.to_string(),
        p.channels_hit().to_string(),
        match Inquiry::of(p.lap) {
            // Fixed by the specification, not narrowed from anything.
            Some(_) => "DCI".to_string(),
            None => uap_cell(state.net.bt_uap.get(&p.lap)),
        },
        since(p.first_seen),
    ]
}

/// The channels a piconet was heard on, as runs: `2-5, 17, 40-41`.
fn channel_runs(mask: u128) -> String {
    let mut runs = Vec::new();
    let mut ch = 0u8;
    while ch < 79 {
        if mask & (1 << ch) == 0 {
            ch += 1;
            continue;
        }
        let start = ch;
        while ch + 1 < 79 && mask & (1 << (ch + 1)) != 0 {
            ch += 1;
        }
        runs.push(if start == ch {
            start.to_string()
        } else {
            format!("{start}-{ch}")
        });
        ch += 1;
    }
    runs.join(", ")
}

fn detail(
    p: &Piconet,
    state: &SdrMetrics,
    now: std::time::Instant,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let field = |label: &str, value: String| {
        Line::from(vec![
            crate::ui::chrome::field(label, LABEL_W, theme),
            Span::styled(value, Style::default().fg(theme.value)),
        ])
    };
    let since =
        |t: std::time::Instant| format!("{} ago", ago(now.saturating_duration_since(t).as_secs()));
    let inquiry = Inquiry::of(p.lap);
    let mut out = vec![
        crate::ui::chrome::section(
            if inquiry.is_some() {
                "inquiry"
            } else {
                "piconet"
            },
            "",
            iw,
            theme,
        ),
        field(
            "LAP",
            match inquiry {
                Some(i) => format!("{:#08x}  {}, no one's address", p.lap, i.short()),
                None => format!("{:#08x}  the master's", p.lap),
            },
        ),
        field("hits", p.hits.to_string()),
        field(
            "heard",
            format!("first {}, last {}", since(p.first_seen), since(p.last_seen)),
        ),
    ];
    let room = iw.saturating_sub(LABEL_W + 1);
    for (label, text) in [
        (
            "channels",
            // Of 79, not of the channels watched now: in SURVEY a piconet
            // was heard wherever the survey stood at the time.
            format!(
                "{} of 79: {}",
                p.channels_hit(),
                channel_runs(p.channel_mask())
            ),
        ),
        match inquiry {
            Some(i) => ("meaning", i.meaning().to_string()),
            None => ("UAP", uap_sentence(state.net.bt_uap.get(&p.lap))),
        },
    ] {
        for (i, chunk) in crate::ui::chrome::wrap(&text, room, 3)
            .into_iter()
            .enumerate()
        {
            out.push(field(if i == 0 { label } else { "" }, chunk));
        }
    }
    out
}

/// The selected piconet's detail within `budget` rows: its core block, or
/// nothing where even that does not fit (the table's rows come first, as
/// the census's do), then each section in order while it fits whole. A
/// section left out is named on a last line, so a short panel says what a
/// taller one would show rather than stopping mid-sentence.
fn detail_within(
    p: &Piconet,
    state: &SdrMetrics,
    now: std::time::Instant,
    iw: usize,
    budget: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let mut out = detail(p, state, now, iw, theme);
    if out.len() > budget {
        return Vec::new();
    }
    if Inquiry::of(p.lap).is_some() {
        // Every searching device sends the same code, so the sections below
        // would mix them all into one reading: said once, and not drawn.
        let text = format!(
            "UAP fixed at the DCI, {DCI:#04x}. Every device inquiring sends this code, \
             so there is no one clock, modulation or header stream to read"
        );
        for chunk in crate::ui::chrome::wrap(&text, iw.saturating_sub(1), 3) {
            if out.len() < budget {
                out.push(Line::from(Span::styled(
                    format!(" {chunk}"),
                    Style::default().fg(theme.label),
                )));
            }
        }
        return out;
    }
    let sections = [
        ("MODULATION", modulation_lines(p, iw, theme)),
        ("TIMING", timing_lines(p, state, iw, theme)),
        ("HEADERS", header_lines(p, state, iw, theme)),
    ];
    let last = sections.len() - 1;
    let mut left_out = Vec::new();
    for (k, (name, lines)) in sections.into_iter().enumerate() {
        // Whole, in order, and leaving a row for the line that names what
        // is left out, unless this is the last section.
        let reserve = usize::from(k < last);
        if left_out.is_empty() && out.len() + lines.len() + reserve <= budget {
            out.extend(lines);
        } else {
            left_out.push(name);
        }
    }
    if !left_out.is_empty() && out.len() < budget {
        out.push(Line::from(Span::styled(
            format!(" + {} on a taller panel", left_out.join(", ")),
            Style::default().fg(theme.label),
        )));
    }
    out
}

/// BR's modulation index band, **read from the Core Specification 5.4,
/// Vol 2, Part A, 3.1.1** on the SIG's own site this session: "The
/// Modulation index shall be between 0.28 and 0.35" (GFSK, BT = 0.5,
/// 1 Msym/s).
const BR_INDEX: Limit = Limit::Band {
    low: 0.28,
    high: 0.35,
};

/// The same band as a deviation: `h = 2 * delta_f / 1 Msym/s`, so 140 to
/// 175 kHz. Derived from [`BR_INDEX`], not a second figure from the text.
const BR_DELTA_F1_KHZ: Limit = Limit::Band {
    low: 140.0,
    high: 175.0,
};

/// The same section: "the minimum frequency deviation, Fmin ... which
/// corresponds to 1010 sequence shall be no smaller than ±80% of the
/// frequency deviation (fd) ... which corresponds to a 00001111 sequence".
/// The text states it for the minimum; what is shown against it here is the
/// ratio of the means, which is what a header's symbols support, and the
/// row is labelled so.
const BR_RATIO: Limit = Limit::Min(0.8);

/// Resolutions a reading must beat before it prints, as the BLE rows'
/// (`ble_detail`): a fraction of the band each limit states.
const INDEX_RESOLUTION: f64 = 0.02;
const DELTA_F1_RESOLUTION_KHZ: f64 = 10.0;
const RATIO_RESOLUTION: f64 = 0.1;

/// The MODULATION section (net-ux-polish-plan 6.4): the piconet's BR
/// modulation index, read from the trailer and header symbols of every
/// header captured on its LAP (`piconet::Deviation`), against the BR band,
/// in the same `widgets::limit` rows the BLE packet detail uses, so the two
/// protocols' transmitter quality reads alike. The header's FEC repeats
/// each bit three times, so settled runs are plentiful and alternating
/// ones are only the trailer's: the ratio row waits longer for its second
/// reading, and says so.
fn modulation_lines(p: &Piconet, iw: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    let d = p.headers.deviation;
    let mut out = vec![crate::ui::chrome::section(
        "modulation",
        "BR limits: Core 5.4 Vol 2 A 3.1.1",
        iw,
        theme,
    )];
    let quiet = |text: &str| {
        Line::from(Span::styled(
            format!(" {text}"),
            Style::default().fg(theme.stale),
        ))
    };
    let Some(df1) = d.settled.mean() else {
        out.push(quiet(&format!(
            "not measured: {} settled runs in {} headers, two needed",
            d.settled.n, p.headers.captured
        )));
        return out;
    };
    let mut rows = vec![
        LimitRow::new(
            "Mod index",
            Reading::new(df1.scale(2.0 / 1e6), "", INDEX_RESOLUTION),
            BR_INDEX,
        ),
        LimitRow::new(
            "df1 avg",
            Reading::new(df1.scale(0.001), "kHz", DELTA_F1_RESOLUTION_KHZ),
            BR_DELTA_F1_KHZ,
        ),
    ];
    let ratio = d.alternating.mean().map(|df2| df2.ratio(&df1));
    if let Some(r) = ratio {
        rows.push(LimitRow::new(
            "df2/df1",
            Reading::new(r, "", RATIO_RESOLUTION),
            BR_RATIO,
        ));
    }
    let w = RowWidths::fit_within(&rows, iw);
    out.extend(rows.iter().map(|r| Line::from(r.spans(theme, w))));
    if ratio.is_none() {
        out.push(quiet("df2/df1: fewer than two alternating runs yet"));
    }
    for chunk in crate::ui::chrome::wrap(
        &format!(
            "{} settled and {} alternating runs from {} headers, every member's",
            d.settled.n, d.alternating.n, p.headers.captured
        ),
        iw.saturating_sub(1),
        2,
    ) {
        out.push(Line::from(Span::styled(
            format!(" {chunk}"),
            Style::default().fg(theme.label),
        )));
    }
    out
}

/// Slot jitter's limit, **read from the Core Specification 5.4, Vol 2,
/// Part B, 2.2.5**: "The instantaneous timing shall not deviate more than
/// 1 μs from the average timing."
const JITTER_LIMIT_US: Limit = Limit::Max(1.0);

/// A jitter reading must beat this before it prints: a quarter of the
/// limit, which a few dozen hits reach.
const JITTER_RESOLUTION_US: f64 = 0.25;

/// The TIMING section (net-ux-polish-plan 6.5): each access code's offset
/// from the piconet's own fitted 625 µs grid (`signal::bt::slots`). The
/// largest against the specification's 1 µs, the root mean square as a
/// reading, and what they rest on: how many hits over how long. Refused
/// below `signal::bt::slots::MIN_HITS` hits and when the hits line up on no grid
/// beyond chance; never a figure from three points.
///
/// **Every member's packets, and our own resolution in it.** A LAP is the
/// piconet, so slaves' transmissions are on the grid too, timed from what
/// they received; and a hit is dated to a quarter symbol, 0.25 µs, which a
/// residual includes.
fn timing_lines(
    p: &Piconet,
    state: &SdrMetrics,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::signal::bt::slots::SlotRefusal;
    let mut out = vec![crate::ui::chrome::section(
        "timing",
        "625 us slots: Core 5.4 Vol 2 B 2.2.5",
        iw,
        theme,
    )];
    let quiet = |text: String| {
        Line::from(Span::styled(
            format!(" {text}"),
            Style::default().fg(theme.stale),
        ))
    };
    let f = match &p.slots {
        None => {
            out.push(quiet("no hit timed yet".to_string()));
            return out;
        }
        Some(Err(SlotRefusal::Collecting { have, need })) => {
            out.push(quiet(format!(
                "collecting: {have} of {need} hits to fit a slot grid"
            )));
            return out;
        }
        Some(Err(SlotRefusal::NoGrid { hits })) => {
            out.push(quiet(format!(
                "no slot grid: {hits} hits do not line up at 625 us beyond chance"
            )));
            return out;
        }
        Some(Ok(f)) => f,
    };
    let rows = vec![LimitRow::new(
        "Jitter max",
        Reading::new(Uncertain::exact(f.max_us), "us", f64::INFINITY),
        JITTER_LIMIT_US,
    )];
    let w = RowWidths::fit_within(&rows, iw);
    out.extend(rows.iter().map(|r| Line::from(r.spans(theme, w))));
    out.push(Line::from(vec![
        crate::ui::chrome::field("rms", LABEL_W, theme),
        Span::styled(
            Reading::new(f.rms_us, "us", JITTER_RESOLUTION_US).text(),
            Style::default().fg(theme.value),
        ),
    ]));
    // The piconet's own colour, as on the hop panel and its roster chip.
    let colour = state
        .net
        .bt_piconets
        .iter()
        .position(|q| q.lap == p.lap)
        .map_or(theme.value_hi, |k| theme.series_color(k));
    let (bars, beyond) = residual_histogram(&f.residuals_us, iw, colour, theme);
    out.extend(bars);
    let span = crate::ui::widgets::timing_fmt::seconds_ms((f.span_us / 1e3) as u64);
    let beyond = if beyond > 0 {
        format!(", {beyond} beyond the plot's 1.5")
    } else {
        String::new()
    };
    for chunk in crate::ui::chrome::wrap(
        &format!(
            "residual from the grid, us{beyond}: {} hits over {span}, every member's; hits dated to 0.25 us",
            f.hits
        ),
        iw.saturating_sub(1),
        2,
    ) {
        out.push(Line::from(Span::styled(
            format!(" {chunk}"),
            Style::default().fg(theme.label),
        )));
    }
    out
}

/// The residual plot's reach either side of the grid, µs: past the 1 µs
/// limit, so a residual beyond it shows as one.
const PLOT_US: f64 = 1.5;
/// Its height in rows of eighth blocks.
const PLOT_ROWS: usize = 3;

/// Where each hit sat against the grid (net-ux-polish-plan 6.5.b):
/// residuals from −[`PLOT_US`] to +[`PLOT_US`] in bars of the piconet's
/// own colour, the specification's ±1 µs (2.2.5) as `┊` rules in the
/// warning ink, zero as a dim one, and a tick and label row under them. The
/// numbers say how wide the spread is; the shape says whether it is one
/// spread or two, as when a peripheral answers a little late on every slot
/// and stands as its own hump. Returns the lines, empty where the width
/// cannot hold a readable plot, and how many residuals fell beyond it.
fn residual_histogram(
    residuals: &[f32],
    iw: usize,
    colour: ratatui::style::Color,
    theme: &crate::Theme,
) -> (Vec<Line<'static>>, usize) {
    const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let cols = iw.saturating_sub(4);
    let beyond = residuals
        .iter()
        .filter(|r| (r.abs() as f64) >= PLOT_US)
        .count();
    if cols < 15 {
        return (Vec::new(), beyond);
    }
    let col_of = |x: f64| {
        (((x + PLOT_US) / (2.0 * PLOT_US)) * cols as f64)
            .floor()
            .clamp(0.0, cols as f64 - 1.0) as usize
    };
    let mut bins = vec![0u32; cols];
    for &r in residuals.iter().filter(|r| (r.abs() as f64) < PLOT_US) {
        bins[col_of(r as f64)] += 1;
    }
    let most = bins.iter().copied().max().unwrap_or(0).max(1);
    let limits = [col_of(-1.0), col_of(1.0)];
    let zero = col_of(0.0);
    let mut out = Vec::with_capacity(PLOT_ROWS + 2);
    for row in 0..PLOT_ROWS {
        let base = (PLOT_ROWS - 1 - row) * 8;
        let mut spans = vec![Span::raw("  ")];
        for (c, &n) in bins.iter().enumerate() {
            let fill = ((n as f64 / most as f64 * (PLOT_ROWS * 8) as f64).round() as usize)
                .saturating_sub(base)
                .min(8);
            spans.push(if fill > 0 {
                Span::styled(EIGHTHS[fill].to_string(), Style::default().fg(colour))
            } else if limits.contains(&c) {
                Span::styled("\u{250a}", Style::default().fg(theme.status_warn))
            } else if c == zero {
                Span::styled("\u{250a}", Style::default().fg(theme.border_dim))
            } else {
                Span::raw(" ")
            });
        }
        out.push(Line::from(spans));
    }
    let ticks: String = (0..cols)
        .map(|c| {
            if limits.contains(&c) || c == zero {
                '\u{2534}'
            } else {
                '\u{2500}'
            }
        })
        .collect();
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(ticks, Style::default().fg(theme.border_dim)),
    ]));
    let mut labels = vec![' '; cols];
    for (c, text) in [(limits[0], "-1"), (zero, "0"), (limits[1], "+1")] {
        let at = c.saturating_sub(text.len() / 2).min(cols - text.len());
        for (i, ch) in text.chars().enumerate() {
            labels[at + i] = ch;
        }
    }
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            labels.into_iter().collect::<String>(),
            Style::default().fg(theme.label),
        ),
    ]));
    (out, beyond)
}

/// What the piconet's headers say (net-ux-polish-plan 6.3), under its own
/// heading, marked as the port it is: the header decode is `libbtbb`'s,
/// read from its source and never yet checked against a classic
/// transmitter on the air (`signal::bt::header`, rule 1).
///
/// **Read only under one UAP.** Before the UAP is one value the heading
/// says so and how many headers are waiting; no type is guessed from a
/// candidate (rule 2). A header that did not decode under the resolved UAP
/// is counted beside the ones that did, because a rising count is how a
/// wrong resolution would show.
fn header_lines(
    p: &Piconet,
    state: &SdrMetrics,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::signal::bt::header::PacketType;
    let h = &p.headers;
    let field = |label: &str, value: String| {
        Line::from(vec![
            crate::ui::chrome::field(label, LABEL_W, theme),
            Span::styled(value, Style::default().fg(theme.value)),
        ])
    };
    let mut out = vec![crate::ui::chrome::section(
        "headers",
        "libbtbb port, unchecked on air",
        iw,
        theme,
    )];
    if h.captured == 0 {
        out.push(field(
            "captured",
            "none yet: no header followed a hit".to_string(),
        ));
        return out;
    }
    let uap = match state.net.bt_uap.get(&p.lap).map(|u| u.as_slice()) {
        Some([one]) => *one,
        other => {
            let n = other.map_or(0, |u| u.len());
            out.push(field(
                "captured",
                format!(
                    "{}, not read: UAP not resolved ({n} candidates)",
                    h.captured
                ),
            ));
            out.push(field("clock", clock_text(h.clock_hypotheses)));
            return out;
        }
    };
    let mut read = format!("{} of {} captured", h.decoded, h.captured);
    if h.undecoded > 0 {
        read.push_str(&format!(
            ", {} did not decode under {uap:#04x}",
            h.undecoded
        ));
    }
    out.push(field("read", read));
    let mut mix: Vec<(u32, u8)> = (0..16u8)
        .map(|c| (h.types[c as usize], c))
        .filter(|(n, _)| *n > 0)
        .collect();
    mix.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let room = iw.saturating_sub(LABEL_W + 1);
    let types = if mix.is_empty() {
        "-".to_string()
    } else {
        mix.iter()
            .map(|(n, c)| format!("{} {n}", PacketType::from_code(*c).label()))
            .collect::<Vec<_>>()
            .join(" \u{00b7} ")
    };
    for (i, chunk) in crate::ui::chrome::wrap(&types, room, 2)
        .into_iter()
        .enumerate()
    {
        out.push(field(if i == 0 { "types" } else { "" }, chunk));
    }
    let addrs: Vec<String> = (0..8u8)
        .filter(|a| h.lt_addrs & (1 << a) != 0)
        .map(|a| {
            if a == 0 {
                "0 (broadcast)".to_string()
            } else {
                a.to_string()
            }
        })
        .collect();
    out.push(field(
        "LT_ADDR",
        if addrs.is_empty() {
            "-".to_string()
        } else {
            addrs.join(", ")
        },
    ));
    out.push(field("clock", clock_text(h.clock_hypotheses)));
    out
}

/// The CLK1-6 hunt, in words: the whitening every header is read through
/// depends on it.
fn clock_text(hypotheses: u8) -> String {
    match hypotheses {
        0 => "CLK1-6 not tracked yet".to_string(),
        1 => "CLK1-6 found (1 of 64 hypotheses left)".to_string(),
        n => format!("CLK1-6: {n} of 64 hypotheses left"),
    }
}

impl Panel for NetBtPiconetsPanel {
    fn name(&self) -> &'static str {
        "net_bt_piconets"
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 6)
    }

    fn focus_key(&self) -> Option<char> {
        // The command rail's letter too: no NET layout shows the rail
        // (`app::FocusKeys`).
        Some('c')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("↑↓", "select a piconet")]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Pi_conets")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            // Hits and first sightings accumulate for the session, so a drop
            // at any point in it undercounts them.
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
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let width = inner.width as usize;
        let note = |text: &str, ink| {
            crate::ui::chrome::wrap(text, width, 4)
                .into_iter()
                .map(move |chunk| Line::from(Span::styled(chunk, Style::default().fg(ink))))
        };

        // The three silences, each named (bar item 3).
        let roster = ordered(&state.net.bt_piconets);
        if roster.is_empty() {
            let mut lines: Vec<Line> = Vec::new();
            match &state.net.bt_refused {
                Some(reason) => {
                    lines.extend(note("not watching", theme.stale));
                    lines.extend(note(reason, theme.label));
                }
                None if state.net.bt_channels_watched.is_empty() => {
                    lines.extend(note("no classic receiver running", theme.stale));
                }
                None => lines.extend(note(
                    &format!(
                        "watching {} channels - no piconet heard yet",
                        state.net.bt_channels_watched.len()
                    ),
                    theme.stale,
                )),
            }
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let now = std::time::Instant::now();
        let fit = columns_that_fit(COLUMNS, width);
        let laps: Vec<u32> = roster.iter().map(|p| p.lap).collect();
        let cursor = state.net.bt_view.cursor(&laps);
        let height = inner.height as usize;

        // The detail block gives way to the table, as the census's does.
        // What the table keeps (header, its first rows, a gap) comes
        // first; the detail takes what is left.
        let budget = height.saturating_sub(2 + roster.len().min(TABLE_KEEPS));
        let extra = cursor
            .map(|i| detail_within(roster[i], state, now, width, budget, theme))
            .unwrap_or_default();

        let mut lines = vec![header(
            COLUMNS,
            fit,
            Sort {
                column: ORDERED_BY,
                descending: false,
            },
            theme,
        )];
        // The rows the roster has, up to what the block leaves: the block
        // follows the last row rather than the foot of the panel, so a short
        // roster does not hold its detail a screen away from it.
        let body = height.saturating_sub(1 + extra.len()).min(roster.len());
        let start = viewport_start(
            state.net.bt_view.first_visible,
            cursor.unwrap_or(0),
            roster.len(),
            body,
        );
        for (i, p) in roster.iter().enumerate().skip(start).take(body) {
            let mut line = row(
                COLUMNS,
                fit,
                &cells(p, state, now),
                Some(i) == cursor,
                theme,
            );
            // The chip wears the colour the scatter draws this piconet in:
            // its place in `bt_piconets`, the order first heard.
            let colour = state
                .net
                .bt_piconets
                .iter()
                .position(|q| q.lap == p.lap)
                .map(|k| theme.series_color(k));
            if let (Some(colour), Some(cell)) = (colour, line.spans.get(1).cloned()) {
                let text = cell.content.to_string();
                if let Some(rest) = text.strip_prefix(CHIP) {
                    line.spans.splice(
                        1..2,
                        [
                            Span::styled(CHIP.to_string(), cell.style.fg(colour)),
                            Span::styled(rest.to_string(), cell.style),
                        ],
                    );
                }
            }
            lines.push(line);
        }
        if !extra.is_empty() && lines.len() + extra.len() < height {
            lines.push(Line::from(""));
        }
        lines.extend(extra);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::bt::piconet::observe;
    use crate::state::fixture::draw;
    use std::time::{Duration, Instant};

    fn heard() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = (0..20).collect();
        let t = Instant::now() - Duration::from_secs(30);
        for (i, ch) in [2u8, 3, 4, 5, 17].into_iter().enumerate() {
            observe(
                &mut m.net.bt_piconets,
                0x5a3c71,
                ch,
                t + Duration::from_secs(i as u64),
            );
        }
        observe(&mut m.net.bt_piconets, 0x123456, 9, Instant::now());
        m.net.bt_uap.insert(0x5a3c71, vec![0x4c, 0x9a]);
        m.net.bt_uap.insert(0x123456, vec![0x21]);
        m
    }

    /// The three silences are three sentences (bar item 3): refused, not
    /// running, and listening with nothing heard.
    #[test]
    fn an_empty_roster_says_which_silence_it_is() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_refused = Some("no classic channel fits the view".to_string());
        let out = draw(NetBtPiconetsPanel, 50, 8, &m).join("\n");
        assert!(out.contains("not watching"), "{out}");
        assert!(out.contains("no classic channel fits"), "{out}");

        m.net.bt_refused = None;
        let out = draw(NetBtPiconetsPanel, 50, 8, &m).join("\n");
        assert!(out.contains("no classic receiver running"), "{out}");

        m.net.bt_channels_watched = vec![10, 20];
        let out = draw(NetBtPiconetsPanel, 50, 8, &m).join("\n");
        assert!(out.contains("watching 2 channels"), "{out}");
    }

    /// An inquiry code wears its own name and a UAP fixed by the
    /// specification, and its detail stops before the sections that would
    /// average every searching device into one.
    #[test]
    fn an_inquiry_code_is_named_and_not_read_as_a_piconet() {
        let mut m = heard();
        observe(&mut m.net.bt_piconets, 0x9E_8B33, 40, Instant::now());
        m.net.bt_view.selected = Some(0x9E_8B33);
        let out = draw(NetBtPiconetsPanel, 60, 24, &m);
        let text = out.join("\n");
        let row = out.iter().find(|l| l.contains("GIAC")).expect(&text);
        assert!(row.contains("DCI"), "{row}");
        assert!(!text.contains("0x9e8b33  the master's"), "{text}");
        assert!(text.contains("INQUIRY"), "{text}");
        assert!(text.contains("no one's address"), "{text}");
        assert!(text.contains("general inquiry"), "{text}");
        assert!(!text.contains("MODULATION"), "{text}");
        assert!(!text.contains("TIMING"), "{text}");
        assert!(!text.contains("HEADERS"), "{text}");
    }

    /// **One row per piconet, the most recently heard first**, with its
    /// hits, how many channels, and how far its UAP has narrowed.
    #[test]
    fn each_piconet_gets_a_row_the_newest_first() {
        let out = draw(NetBtPiconetsPanel, 50, 8, &heard());
        let text = out.join("\n");
        let newest = text.find("0x123456").expect(&text);
        let older = text.find("0x5a3c71").expect(&text);
        assert!(newest < older, "{text}");
        let row = out.iter().find(|l| l.contains("0x5a3c71")).unwrap();
        assert!(row.contains("2 left"), "{row}");
        assert!(row.contains(" 5 "), "five hits: {row}");
        let row = out.iter().find(|l| l.contains("0x123456")).unwrap();
        assert!(row.contains("0x21"), "{row}");
    }

    /// The selected piconet spelled out: the channels as runs, and the
    /// UAP's state in words.
    #[test]
    fn the_selected_piconet_is_spelled_out() {
        let mut m = heard();
        m.net.bt_view.selected = Some(0x5a3c71);
        let out = draw(NetBtPiconetsPanel, 60, 16, &m).join("\n");
        assert!(out.contains("PICONET"), "{out}");
        assert!(out.contains("5 of 79: 2-5, 17"), "{out}");
        assert!(out.contains("2 candidates (0x4c, 0x9a)"), "{out}");

        let none = draw(NetBtPiconetsPanel, 60, 16, &heard()).join("\n");
        assert!(!none.contains("PICONET"), "{none}");
    }

    /// **A first header leaves 32 candidates**, seen on the air: the cell
    /// holds `32 left` whole, and the detail says how many rather than
    /// listing a wall of values.
    #[test]
    fn many_candidates_are_counted_not_listed() {
        let mut m = heard();
        m.net
            .bt_uap
            .insert(0x5a3c71, (0..32).map(|i| i * 8 + 1).collect());
        m.net.bt_view.selected = Some(0x5a3c71);
        let out = draw(NetBtPiconetsPanel, 60, 16, &m).join("\n");
        assert!(out.contains("32 left"), "{out}");
        assert!(
            out.contains("32 candidates; each further header narrows them"),
            "{out}"
        );
        assert!(!out.contains("0x09"), "no list: {out}");
    }

    /// The detail follows the last row, not the foot of the panel: a short
    /// roster keeps its detail beside it.
    #[test]
    fn the_detail_follows_the_roster_rather_than_the_foot() {
        let mut m = heard();
        m.net.bt_view.selected = Some(0x5a3c71);
        let out = draw(NetBtPiconetsPanel, 60, 30, &m);
        let block = out.iter().position(|l| l.contains("PICONET")).unwrap();
        // The frame, the header, two rows, a gap.
        assert_eq!(block, 5, "{}", out.join("\n"));
    }

    /// **Headers are read only under one UAP** (6.3): before, the block
    /// says how many wait and why; after, the type mix (most first), the
    /// LT_ADDRs, and the clock, under a heading that says it is a port.
    #[test]
    fn headers_are_read_only_once_the_uap_is_one_value() {
        use crate::signal::bt::header::{Header, PacketType};
        use crate::signal::bt::piconet::{observe_header, HeaderRead};
        let mut m = heard();
        m.net.bt_view.selected = Some(0x5a3c71);
        let out = draw(NetBtPiconetsPanel, 70, 30, &m).join("\n");
        assert!(out.contains("none yet"), "{out}");

        observe_header(
            &mut m.net.bt_piconets,
            0x5a3c71,
            HeaderRead::Unresolved,
            2,
            Default::default(),
        );
        let out = draw(NetBtPiconetsPanel, 70, 30, &m).join("\n");
        assert!(
            out.contains("1, not read: UAP not resolved (2 candidates)"),
            "{out}"
        );
        assert!(!out.contains("types"), "{out}");

        m.net.bt_uap.insert(0x5a3c71, vec![0x4c]);
        let header = |t, a| Header {
            lt_addr: a,
            packet_type: t,
            flags: 0,
            hec: 0,
            clk6: 0,
        };
        for (t, a) in [
            (PacketType::Poll, 1),
            (PacketType::Poll, 1),
            (PacketType::Null, 0),
            (PacketType::Dh1, 2),
            (PacketType::Poll, 2),
        ] {
            observe_header(
                &mut m.net.bt_piconets,
                0x5a3c71,
                HeaderRead::Decoded(header(t, a)),
                1,
                Default::default(),
            );
        }
        observe_header(
            &mut m.net.bt_piconets,
            0x5a3c71,
            HeaderRead::Undecoded,
            1,
            Default::default(),
        );
        let out = draw(NetBtPiconetsPanel, 70, 30, &m).join("\n");
        assert!(out.contains("HEADERS"), "{out}");
        assert!(out.contains("unchecked on air"), "{out}");
        assert!(
            out.contains("5 of 7 captured, 1 did not decode under 0x4c"),
            "{out}"
        );
        assert!(
            out.contains("POLL 3 \u{00b7} NULL 1 \u{00b7} DH1 1"),
            "{out}"
        );
        assert!(out.contains("0 (broadcast), 1, 2"), "{out}");
        assert!(out.contains("CLK1-6 found"), "{out}");
    }

    /// **The BR modulation index against the BR band** (6.4): refused
    /// until two settled runs, then the index, delta-f1 and, once two
    /// alternating runs are in, the ratio, in the limit rows BLE uses.
    #[test]
    fn the_modulation_index_is_read_against_the_br_band() {
        use crate::signal::bt::piconet::{observe_header, Deviation, HeaderRead};
        use crate::signal::dsp::deviation::Sums;
        let mut m = heard();
        m.net.bt_view.selected = Some(0x5a3c71);
        let out = draw(NetBtPiconetsPanel, 80, 40, &m).join("\n");
        assert!(out.contains("MODULATION"), "{out}");
        assert!(
            out.contains("not measured: 0 settled runs in 0 headers"),
            "{out}"
        );

        // 160 kHz settled, h = 0.32; alternating at 150 kHz, ratio ~0.94.
        let dev = Deviation {
            settled: Sums::of(&[158_000.0, 160_000.0, 162_000.0, 160_000.0]),
            alternating: Sums::of(&[149_000.0, 151_000.0]),
        };
        observe_header(
            &mut m.net.bt_piconets,
            0x5a3c71,
            HeaderRead::Unresolved,
            32,
            dev,
        );
        let out = draw(NetBtPiconetsPanel, 80, 40, &m).join("\n");
        assert!(out.contains("Mod index"), "{out}");
        assert!(out.contains("0.32"), "{out}");
        assert!(out.contains("0.28"), "the band is drawn: {out}");
        assert!(out.contains("df2/df1"), "{out}");
        assert!(
            out.contains("4 settled and 2 alternating runs from 1 headers"),
            "{out}"
        );
        assert!(out.contains("Core 5.4 Vol 2 A 3.1.1"), "{out}");
    }

    /// **Slot jitter against the specification's 1 µs** (6.5): refused
    /// while collecting and when no grid is found, then the largest
    /// residual against the limit, the rms beside it, and what they rest on.
    #[test]
    fn slot_jitter_is_shown_against_the_one_microsecond_limit() {
        use crate::signal::bt::slots::{SlotFit, SlotRefusal};
        let mut m = heard();
        m.net.bt_view.selected = Some(0x5a3c71);
        let at = |m: &SdrMetrics| draw(NetBtPiconetsPanel, 80, 50, m).join("\n");
        assert!(at(&m).contains("no hit timed yet"), "{}", at(&m));

        let set = |m: &mut SdrMetrics, s| m.net.bt_piconets[0].slots = Some(s);
        set(&mut m, Err(SlotRefusal::Collecting { have: 5, need: 8 }));
        assert!(at(&m).contains("collecting: 5 of 8 hits"), "{}", at(&m));
        set(&mut m, Err(SlotRefusal::NoGrid { hits: 30 }));
        assert!(at(&m).contains("30 hits do not line up"), "{}", at(&m));

        set(
            &mut m,
            Ok(SlotFit {
                hits: 60,
                span_us: 60e6,
                rate_ppm: 12.0,
                rms_us: Uncertain::from_sigma(0.31, 0.03),
                max_us: 0.82,
                residuals_us: vec![0.0; 60],
                model: crate::signal::bt::slots::Model {
                    t0_us: 0.0,
                    period_us: 625.0,
                    offset_us: 0.0,
                    mean_x: 0.0,
                    mean_y: 0.0,
                    slope: 0.0,
                },
            }),
        );
        let out = at(&m);
        assert!(out.contains("TIMING"), "{out}");
        assert!(out.contains("Jitter max"), "{out}");
        assert!(out.contains("0.82"), "{out}");
        assert!(out.contains("0.31"), "{out}");
        assert!(out.contains("60 hits over 60 s"), "{out}");
        assert!(out.contains("residual from the grid"), "{out}");
        assert!(out.contains("Core 5.4 Vol 2 B 2.2.5"), "{out}");
    }

    /// **A short panel keeps what fits and names the rest**: the core
    /// block and the sections that fit whole, then one line saying which
    /// a taller panel would show.
    #[test]
    fn a_short_panel_keeps_whole_sections_and_names_the_rest() {
        let mut m = heard();
        m.net.bt_view.selected = Some(0x5a3c71);
        let out = draw(NetBtPiconetsPanel, 60, 16, &m).join("\n");
        assert!(out.contains("PICONET"), "{out}");
        assert!(out.contains("on a taller panel"), "{out}");
        assert!(out.contains("HEADERS on a taller panel"), "{out}");
        let tall = draw(NetBtPiconetsPanel, 60, 60, &m).join("\n");
        assert!(!tall.contains("taller panel"), "{tall}");
        assert!(tall.contains("HEADERS"), "{tall}");
    }

    /// **The residuals as a shape** (6.5.b): each lands in its column, the
    /// ±1 µs limits and zero are ruled and labelled, and a residual past
    /// the plot is counted rather than dropped.
    #[test]
    fn the_residual_histogram_places_each_hit_and_the_limits() {
        let theme = crate::Theme::sdr();
        let colour = theme.series_color(1);
        // 40 columns across 3 us: 0.075 us each.
        let (lines, beyond) =
            residual_histogram(&[0.0, 0.01, 0.02, 0.9, -0.5, 2.0], 44, colour, &theme);
        assert_eq!(beyond, 1);
        assert_eq!(lines.len(), PLOT_ROWS + 2);
        let text = |l: &Line| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        // The tallest bar (three residuals near zero) reaches the top row.
        let top = text(&lines[0]);
        assert_eq!(top.chars().nth(2 + 20), Some('\u{2588}'), "{top:?}");
        // Limits ruled at -1 and +1 (columns 6 and 33), labelled below.
        assert_eq!(top.chars().nth(2 + 6), Some('\u{250a}'), "{top:?}");
        assert_eq!(top.chars().nth(2 + 33), Some('\u{250a}'), "{top:?}");
        let labels = text(&lines[PLOT_ROWS + 1]);
        assert!(labels.contains("-1") && labels.contains("+1"), "{labels:?}");
        // The limit rules are in the warning ink, the bars in the colour.
        let limit_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "\u{250a}")
            .unwrap();
        assert_eq!(limit_span.style.fg, Some(theme.status_warn));
        let bar = lines[2]
            .spans
            .iter()
            .find(|s| s.content != " " && s.content != "\u{250a}" && s.content != "  ")
            .unwrap();
        assert_eq!(bar.style.fg, Some(colour));
        // Too narrow for a readable plot: none, and still the count.
        assert_eq!(
            residual_histogram(&[3.0], 16, colour, &theme),
            (Vec::new(), 1)
        );
    }

    #[test]
    fn channel_runs_join_neighbours() {
        assert_eq!(channel_runs(0), "");
        assert_eq!(channel_runs(0b1111 << 2 | 1 << 17 | 1 << 78), "2-5, 17, 78");
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut m = heard();
        m.net.bt_view.selected = Some(0x5a3c71);
        for w in 30..70u16 {
            for h in 6..24u16 {
                for s in [&m, &SdrMetrics::fixture()] {
                    for line in draw(NetBtPiconetsPanel, w, h, s) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }
}
