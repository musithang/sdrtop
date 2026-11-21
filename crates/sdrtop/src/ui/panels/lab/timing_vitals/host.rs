// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the computer can take: process CPU/RAM, and the USB link it arrives over.
//!
//! CPU and RAM come from the system task rather than the radio, so unlike
//! everything else in this panel they stay live with RX stopped.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::SdrMetrics;

use super::rows::{load_color, Rows};

pub(super) fn cpu_lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let cpu = state.system.process_cpu_pct as f64;
    // Not `r.heading`: this pair is measured by the system task, so it has no
    // stale form - a stopped radio does not stop the process using CPU.
    let head = vec![
        Span::raw(" "),
        Span::styled("CPU load ", r.lbl()),
        Span::styled(
            format!("{cpu:.1} %"),
            Style::default().fg(load_color(cpu, r.theme)),
        ),
        Span::styled(
            format!("   RAM {} MB", state.system.process_rss_mb),
            r.lbl(),
        ),
    ];
    r.trend(
        head,
        state.system.cpu_history.iter().map(|&v| v as f64).collect(),
    )
    .to_vec()
}

pub(super) fn usb_lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let theme = r.theme;
    let mut out = vec![crate::ui::chrome::section(
        "USB LINK",
        "bulk transfer",
        r.iw,
        theme,
    )];

    // Recent errors outrank session errors: a count that is still climbing is a
    // different problem from one that happened once at start-up.
    let recent: u64 = state.signal.usb_error_history.iter().sum();
    let err_color = if recent > 0 {
        theme.status_crit
    } else if state.signal.usb_errors_session > 0 {
        theme.status_warn
    } else {
        theme.status_ok
    };
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("USB errors ", r.lbl()),
        Span::styled(
            format!("{}", state.signal.usb_errors_session),
            Style::default().fg(err_color),
        ),
        Span::styled(" (session)", r.lbl()),
    ]));

    // What crossed the link, where the backend knows its wire format: the
    // delivered pairs at the wire's bytes per pair (an R2 packing 12-bit
    // samples carries 3 for every 4 delivered). Where it does not, the
    // delivered figure under its own name.
    // The figures are the engine's, the ones the API serves too.
    let t = crate::engine::view::vitals::throughput(state);
    let (name, mbps, ceiling, util) = (
        if t.what == "bus" {
            "Bus throughput "
        } else {
            "Delivered "
        },
        t.mb_s,
        t.ceiling_mb_s,
        t.utilisation,
    );
    out.push(Line::from(if r.stale {
        vec![Span::raw(" "), Span::styled(name, r.lbl()), r.dash()]
    } else {
        vec![
            Span::raw(" "),
            Span::styled(name, r.lbl()),
            Span::styled(format!("{mbps:.1} MB/s"), Style::default().fg(theme.value)),
            Span::styled(format!(" of {ceiling:.1} max"), r.lbl()),
        ]
    }));
    out.push(r.bar(
        "link util",
        util,
        format!("{:.0}%", util * 100.0),
        theme.status_ok,
        theme.status_warn,
        theme.value,
    ));
    out
}
