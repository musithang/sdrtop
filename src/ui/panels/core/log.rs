// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::{LogLevel, SdrMetrics};

/// Status lamp for a log line: a single-column glyph whose shape *and* colour
/// escalate with severity (`·` info → `●` ok/error → `▲` warn), so a glance down
/// the gutter reads the event history without parsing text. Ok and Error share
/// the `●` disc and separate purely by colour, like a green/red panel LED.
pub(crate) fn lamp(level: LogLevel, theme: &crate::Theme) -> Span<'static> {
    let (glyph, color) = match level {
        LogLevel::Info => ("\u{00B7}", theme.border_dim), // ·
        LogLevel::Ok => ("\u{25CF}", theme.status_ok),    // ●
        LogLevel::Warn => ("\u{25B2}", theme.status_warn), // ▲
        LogLevel::Error => ("\u{25CF}", theme.status_crit), // ●
    };
    Span::styled(glyph, Style::default().fg(color))
}

/// Zero-padded `HH:MM:SS`. Split from [`fmt_clock`] so it can be unit-tested
/// without depending on the host timezone.
fn fmt_hms(h: u32, m: u32, s: u32) -> String {
    format!("{h:02}:{m:02}:{s:02}")
}

/// Local wall-clock `HH:MM:SS` for a Unix-epoch instant, via libc's reentrant
/// `localtime_r` (already a dependency - no time crate needed). Falls back to
/// `00:00:00` if the conversion fails.
pub(crate) fn fmt_clock(epoch_secs: u64) -> String {
    // SAFETY: `localtime_r` writes into our stack `tm` and returns a pointer to
    // it (or null on failure); we read scalar fields only, no aliasing.
    let (h, m, s) = unsafe {
        let t = epoch_secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            (0, 0, 0)
        } else {
            (
                tm.tm_hour.max(0) as u32,
                tm.tm_min.max(0) as u32,
                tm.tm_sec.max(0) as u32,
            )
        }
    };
    fmt_hms(h, m, s)
}

pub fn render(f: &mut Frame, inner: Rect, m: &SdrMetrics, theme: &crate::Theme) {
    // Each entry → `lamp  HH:MM:SS  message`. The lamp + dim, fixed-width
    // timestamp form an aligned gutter; the message reads in normal value colour.
    let lines: Vec<Line> =
        m.ui.log
            .iter()
            .map(|e| {
                Line::from(vec![
                    lamp(e.level, theme),
                    Span::raw(" "),
                    Span::styled(
                        fmt_clock(e.at_epoch_secs),
                        Style::default().fg(theme.border_dim),
                    ),
                    Span::raw("  "),
                    Span::styled(e.text.as_ref(), Style::default().fg(theme.value)),
                ])
            })
            .collect();

    let scroll = lines.len().saturating_sub(inner.height as usize) as u16;
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

use crate::ui::panel::{Panel, PanelChrome};

pub struct LogPanel;

impl Panel for LogPanel {
    fn name(&self) -> &'static str {
        "log"
    }
    fn min_size(&self) -> (u16, u16) {
        (20, 7)
    }
    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::deck("Log")
    }
    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        render(f, inner, state, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_hms_zero_pads_to_eight_cols() {
        assert_eq!(fmt_hms(0, 0, 0), "00:00:00");
        assert_eq!(fmt_hms(9, 5, 3), "09:05:03");
        assert_eq!(fmt_hms(14, 23, 1), "14:23:01");
        assert_eq!(fmt_hms(23, 59, 59).len(), 8);
    }

    #[test]
    fn fmt_clock_is_eight_columns() {
        // Whatever the host timezone, the field is a fixed 8-col HH:MM:SS so the
        // log gutter stays aligned.
        assert_eq!(fmt_clock(1_700_000_000).len(), 8);
        assert_eq!(fmt_clock(0).len(), 8);
    }
}
