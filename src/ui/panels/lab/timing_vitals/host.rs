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

use super::calc::link_ceiling_mbps;
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

    let mbps = state.timing.throughput_mean_mbps;
    let ceiling = link_ceiling_mbps(
        state.caps.sample_rate_max_hz,
        state.caps.sample_geometry.bytes_per_pair(),
    );
    let util = if ceiling > 0.0 {
        (mbps / ceiling).clamp(0.0, 1.0)
    } else {
        0.0
    };
    out.push(Line::from(if r.stale {
        vec![
            Span::raw(" "),
            Span::styled("Bus throughput ", r.lbl()),
            r.dash(),
        ]
    } else {
        vec![
            Span::raw(" "),
            Span::styled("Bus throughput ", r.lbl()),
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
