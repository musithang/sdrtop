//! The STREAM section of the rail: drops, buffer fill, USB throughput.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::SdrMetrics;

use super::row::label_cell;

/// Throughput as a compact `5.2 MB/s` string; `—` when not streaming.
fn fmt_mb(bps: u64) -> String {
    if bps == 0 {
        "—".to_string()
    } else {
        format!("{:.1} MB/s", bps as f64 / 1_000_000.0)
    }
}

/// The STREAM section rows: dropped samples, host buffer fill, USB throughput.
pub(super) fn lines(state: &SdrMetrics, active: bool, theme: &crate::Theme) -> Vec<Line<'static>> {
    let value = |s: String| {
        Span::styled(
            s,
            Style::default().fg(if active { theme.value } else { theme.label }),
        )
    };
    let row =
        |name: &str, v: String| Line::from(vec![Span::raw(" "), label_cell(name, theme), value(v)]);
    vec![
        row("DROP", format!("{} /s", state.signal.drops_per_sec)),
        Line::raw(""),
        row("BUF", format!("{:.0} %", state.iq.buf_fill_pct)),
        Line::raw(""),
        row(
            "USB",
            fmt_mb(if active {
                state.radio.current_throughput_bps
            } else {
                0
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_mb_blanks_when_idle() {
        assert_eq!(fmt_mb(0), "—");
        assert_eq!(fmt_mb(5_200_000), "5.2 MB/s");
    }
}
