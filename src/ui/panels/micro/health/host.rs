//! What the computer is spending on the stream: CPU, memory, and the USB link.
//!
//! CPU and RAM come from the system task rather than the radio, so they stay live
//! with RX stopped — which is deliberate: a process still burning CPU after you
//! stopped the stream is worth seeing.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::widgets::micro_common::sparkline;

use super::super::field::Field;
use super::rows::{LABEL_W, SPARK_W, VALUE_W};

/// Bytes per binary megabyte, for the USB throughput readout.
const BYTES_PER_MB: f64 = 1024.0 * 1024.0;

pub(super) fn cpu_line(state: &SdrMetrics, fd: &Field) -> Line<'static> {
    let cpu = state.system.process_cpu_pct;
    let hist: Vec<f64> = state.system.cpu_history.iter().map(|&v| v as f64).collect();
    Line::from(vec![
        Span::raw(" "),
        fd.padded("CPU", LABEL_W),
        fd.value(format!("{:<VALUE_W$}", format!("{cpu:.1}%"))),
        Span::styled(
            sparkline(&hist, SPARK_W),
            Style::default().fg(fd.theme.value),
        ),
    ])
}

pub(super) fn ram_line(state: &SdrMetrics, fd: &Field) -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        fd.padded("RAM", LABEL_W),
        fd.value(format!("{} MB", state.system.process_rss_mb)),
    ])
}

pub(super) fn usb_line(state: &SdrMetrics, fd: &Field) -> Line<'static> {
    let theme = fd.theme;
    let throughput = if fd.stale {
        fd.dash()
    } else {
        fd.value(format!(
            "{:.1} MB/s",
            state.radio.current_throughput_bps as f64 / BYTES_PER_MB
        ))
    };
    let errors = state.signal.usb_errors_session;
    Line::from(vec![
        Span::raw(" "),
        fd.padded("USB", LABEL_W),
        throughput,
        Span::raw("   "),
        fd.label("err: "),
        Span::styled(
            format!("{errors}"),
            Style::default().fg(if errors == 0 {
                theme.value
            } else {
                theme.status_warn
            }),
        ),
    ])
}
