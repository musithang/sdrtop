// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The queue between the RX callback and the FFT worker: how deep it got, and
//! what it had to throw away.
//!
//! **Not the buffer between the radio and the host.** That one is not visible
//! from here at all: libhackrf, librtlsdr and SoapySDR each keep their own and
//! none of them publishes a depth, so what this section used to call a ring
//! buffer with an overrun margin was sdrtop's own four-block queue wearing the
//! driver's name. Samples the link itself loses are counted as drops in the
//! section above; a ceiling reached here costs the spectrum a frame, not the
//! radio a sample.
//!
//! The peak matters more than the depth at any one instant, because the queue
//! only has to reach the ceiling once to lose a block. Both figures come from
//! the hot path, where the hand-off actually happens, rather than from a reading
//! taken here: a queue that fills and drains in microseconds, sampled once every
//! 200 ms, reads a comfortable 0 % through every backlog the poll does not
//! happen to land in.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::widgets::micro_common::buf_color;

use super::rows::Rows;

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let theme = r.theme;
    let mut out = vec![crate::ui::chrome::section(
        "FFT FEED",
        "spectrum queue",
        r.iw,
        theme,
    )];

    let fill = state.iq.buf_fill_pct as f64;
    out.push(r.bar(
        "queue peak",
        fill / 100.0,
        format!("{fill:.0}%"),
        theme.status_ok,
        theme.status_crit,
        buf_color(state.iq.buf_fill_pct, theme),
    ));

    // The event the depth was only ever a proxy for. A block refused here is a
    // frame the spectrum never drew, and it is the only record that the block
    // existed: the channel is lossy by design and carries no sequence number.
    let dropped = state.iq.fft_drops;
    let session = state.iq.fft_drops_session;
    let col = if session == 0 {
        theme.status_ok
    } else if dropped == 0 {
        theme.status_warn
    } else {
        theme.status_crit
    };
    out.push(Line::from(if r.stale {
        vec![
            Span::raw(" "),
            Span::styled("Blocks dropped ", r.lbl()),
            r.dash(),
        ]
    } else {
        vec![
            Span::raw(" "),
            Span::styled("Blocks dropped ", r.lbl()),
            Span::styled(format!("{dropped}"), Style::default().fg(col)),
            Span::styled(format!("   session {session}"), r.dim()),
        ]
    }));
    out
}
