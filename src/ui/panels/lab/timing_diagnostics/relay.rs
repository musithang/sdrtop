// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! ARRIVAL for a stream a service relays: how long the service held it.
//!
//! This zone stands in for CALLBACK TIMING and DEADLINE BUDGET on a
//! [`crate::hardware::DeliveryModel::Relayed`] device. Its driver calls back,
//! but from a service that holds the stream and passes it on in bursts:
//! SDRplay's service hands over 50 blocks at once every 25 to 35 ms at 2
//! MSPS. Graded as callbacks, a stream that lost nothing read `POOR`, with
//! every interval past its budget.
//!
//! What can be said instead is how long the service held the stream: each
//! delivery against the one that came earliest for its place in the stream
//! over the stretch before it. Whether anything was lost is still told by the
//! positions, in the verdict below.

use ratatui::text::{Line, Span};

use crate::state::SdrMetrics;
use crate::ui::widgets::timing_fmt::fmt_us;

use super::rows::Rows;

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let t = &state.timing;
    let theme = r.theme;

    let mut out = vec![crate::ui::chrome::section(
        "ARRIVAL",
        "relayed by a service",
        r.iw,
        theme,
    )];

    match (r.stale, t.relay_hold_us) {
        (false, Some(us)) => out.push(Line::from(vec![
            r.field("Held"),
            Span::styled(format!("\u{2264} {}", fmt_us(us)), r.val()),
            Span::styled(
                format!(
                    "   longest of the last {} s",
                    (crate::state::RELAY_BASELINE_US + crate::state::RELAY_STRETCH_US) / 1_000_000
                ),
                r.dim(),
            ),
        ])),
        _ => out.push(Line::from(vec![r.field("Held"), r.dash()])),
    }
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("the service holds the stream, then passes it on", r.dim()),
    ]));

    if r.stale || t.cb_period_us == 0 {
        out.push(Line::from(vec![r.field("Delivery"), r.dash()]));
    } else {
        out.push(Line::from(vec![
            r.field("Delivery"),
            Span::styled(fmt_us(t.cb_period_us), r.val()),
            Span::styled(format!("   p99 {}", fmt_us(t.dev_p99_us)), r.dim()),
        ]));
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("not a deadline: this rhythm is the service's", r.dim()),
        ]));
    }

    out
}
