//! `timing_vitals` — the host-pipeline health column of the `lab_timing` preset.
//!
//! Sample drops, ADC saturation and CPU as 60 s trends, then the USB link and the
//! ring buffer as captioned bars, closed by a one-line verdict and the uptime.
//! Link utilisation is referenced to the device's own USB ceiling
//! (`caps.sample_rate_max_hz`), not a magic constant.
//!
//! Split by **what each zone measures**, which is also the order a reader works
//! through it — what the radio delivered, what the host could take, what the
//! queue between them did, and the verdict:
//!
//! - [`calc`]: the arithmetic. No theme, no width.
//! - [`rows`]: the trend-block and bar-row vocabulary, and the label column
//!   shared with `timing_diagnostics` on the other half of the bench.
//! - [`stream`]: sample drops + ADC saturation.
//! - [`host`]: CPU/RAM + the USB link.
//! - [`buffer`]: the ring buffer.
//! - [`verdict`]: the closing line.

mod buffer;
mod calc;
mod host;
mod rows;
mod stream;
mod verdict;

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

use rows::Rows;

pub struct TimingVitalsPanel;

impl Panel for TimingVitalsPanel {
    fn name(&self) -> &'static str {
        "timing_vitals"
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 18)
    }
    fn focus_key(&self) -> Option<char> {
        Some('v')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("R", "Reset drop counter"), ("C", "Clear history")]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Hardware _Vitals").stale_when(Staleness::NotStreaming)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let r = Rows::new(
            inner.width as usize,
            !state.radio.hw_streaming,
            theme,
        );

        let mut lines: Vec<Line> = vec![Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "host pipeline health \u{00b7} 60 s rolling",
                Style::default().fg(theme.label),
            ),
        ])];
        lines.extend(stream::lines(state, &r));
        lines.push(Line::raw(""));
        lines.extend(host::cpu_lines(state, &r));
        lines.push(Line::raw(""));
        lines.extend(host::usb_lines(state, &r));
        lines.push(Line::raw(""));
        lines.extend(buffer::lines(state, &r));
        lines.push(Line::raw(""));
        lines.extend(verdict::lines(state, &r));

        crate::ui::chrome::fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}
