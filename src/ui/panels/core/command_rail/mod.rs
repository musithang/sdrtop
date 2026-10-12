// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Command Rail: the `[1]` cockpit's left column.
//!
//! One vertical stack of sections, top to bottom: the frequency hero, the mode
//! strip with its adaptive lead card, then four fixed sections - recall, signal,
//! gain, stream - and a one-line log foot pinned to the bottom.
//!
//! Split by section, because that is how it reads on screen and how it is
//! edited:
//!
//! - [`hero`]: the big frequency readout and the band / sample-rate line.
//! - [`smeter`]: the S-unit bar under it, and the clip alert-memory.
//! - [`modes`]: the mode tabs and whichever card the current mode calls for.
//! - [`recall`], [`signal`], [`gain`], [`stream`]: one module per fixed section.
//! - [`row`]: the shared label column every section aligns its values to.
//!
//! This module builds the stack and hands it the density treatment; the sections
//! know nothing about each other.

mod gain;
mod hero;
mod modes;
mod recall;
mod row;
mod signal;
mod smeter;
mod stream;

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::chrome;
use crate::ui::panel::{Panel, PanelChrome};
use crate::ui::panels::core::log;
use crate::ui::widgets::micro_common::fft_stale;

pub struct CommandRailPanel;

impl Panel for CommandRailPanel {
    fn name(&self) -> &'static str {
        "command_rail"
    }
    fn min_size(&self) -> (u16, u16) {
        (22, 12)
    }

    // `c` for Command: focus the rail to drive it directly. In focus, `←/→` tune
    // (which auto-switches the mode to Hunt) and `Tab` cycles the mode manually.
    fn focus_key(&self) -> Option<char> {
        Some('c')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("←→", "Tune"),
            ("Tab", "Mode"),
            (",·.", "Stage"),
            ("↑↓", "Adjust"),
            ("1·2·3", "Recall"),
            ("M", "Save"),
            ("L", "Log"),
        ]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::deck("_Command")
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let iw = inner.width as usize;
        let stale = fft_stale(state);
        let observer = state.observer.active;
        let active = state.radio.hw_streaming && !observer;

        let mut lines = stack(state, stale, active, observer, iw, theme);

        // The bottom inner row belongs to the log foot, split off first so the
        // stack and the foot can never overlap - that overlap used to flicker -
        // and the foot stays anchored however tall the stack is.
        let (stack_area, foot_area) = if inner.height >= 4 {
            (
                Rect {
                    height: inner.height - 1,
                    ..inner
                },
                Some(Rect {
                    x: inner.x,
                    y: inner.y + inner.height - 1,
                    width: inner.width,
                    height: 1,
                }),
            )
        } else {
            (inner, None)
        };

        // Self-adjusting density: on a short rail where the airy layout would
        // overflow - and clip a whole section - drop only as many blank spacers
        // as needed, evenly across the stack, so the panel keeps as much
        // breathing room as fits instead of snapping to fully dense and
        // stranding empty rows above the foot. Tall rails keep every spacer;
        // very short ones drop them all and lean on the section rules instead.
        chrome::collapse_spacers(&mut lines, stack_area.height as usize);
        f.render_widget(Paragraph::new(lines), stack_area);

        if let Some(foot) = foot_area {
            if let Some(entry) = state.ui.log.back() {
                f.render_widget(Paragraph::new(log_foot(entry, theme)), foot);
            }
        }
    }
}

/// The whole rail as one line stack, in screen order.
fn stack(
    state: &SdrMetrics,
    stale: bool,
    active: bool,
    observer: bool,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();

    // ── Frequency hero, band/SR, S-meter ────────────────────────────────────
    lines.extend(hero::freq_hero_lines(
        state.radio.frequency,
        state.spectrum.step_hz,
        observer,
        iw,
        theme,
    ));
    lines.push(hero::band_sr_line(state, iw, theme));
    // Reserve the S-meter's three rows even when there is no current reading.
    // Removing them on a weak/bursty signal made every section below jump.
    let pwr = state.signal.s_meter_dbfs;
    match (!stale, pwr.filter(|value| value.is_finite())) {
        (true, Some(power)) => lines.extend(smeter::s_meter_lines(
            power,
            state.signal.s_meter_peak_dbfs,
            iw,
            theme,
        )),
        _ => lines.extend([Line::raw(""), Line::raw(""), Line::raw("")]),
    }
    lines.push(Line::raw(""));

    // ── Mode strip and its lead card ────────────────────────────────────────
    // The mode auto-follows actions (tune → Hunt, gain → Bench) and decays back
    // to Monitor; the card adapts with it. Everything below here is fixed.
    let mode = state.ui.effective_rail_mode();
    lines.push(modes::mode_tabs_line(mode, iw, theme));
    lines.extend(modes::mode_card_lines(mode, state, stale, theme));
    lines.push(Line::raw(""));

    for (name, section) in [
        ("Recall", recall::lines(state, stale, theme)),
        ("Signal", signal::lines(state, stale, active, iw, theme)),
        ("Gain", gain::lines(state, active, observer, iw, theme)),
        ("Stream", stream::lines(state, active, theme)),
    ] {
        lines.push(chrome::section(name, "", iw, theme));
        lines.extend(section);
    }

    lines
}

/// The newest log line, pinned to the rail's bottom row: the same lamp, clock
/// and text the log panel draws, so the rail can stand in for it on a layout
/// that has no room for both.
fn log_foot(entry: &crate::state::LogEntry, theme: &crate::Theme) -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        log::lamp(entry.level, theme),
        Span::raw(" "),
        Span::styled(
            log::fmt_clock(entry.at_epoch_secs),
            Style::default().fg(theme.border_dim),
        ),
        Span::raw(" "),
        Span::styled(
            entry.text.as_ref().to_string(),
            Style::default().fg(theme.value),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s_meter_presence_does_not_change_the_stack_height() {
        let no_signal = SdrMetrics::fixture().streaming();
        let signal = SdrMetrics::fixture().streaming().with_carrier(0.0, 20.0);
        let theme = crate::Theme::sdr();

        let without_meter = stack(&no_signal, false, true, false, 34, &theme);
        let with_meter = stack(&signal, false, true, false, 34, &theme);

        assert_eq!(without_meter.len(), with_meter.len());
    }
}
