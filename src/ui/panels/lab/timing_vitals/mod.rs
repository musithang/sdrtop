// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `timing_vitals` - the host-pipeline health column of the `lab_timing` preset.
//!
//! Sample drops, ADC saturation and CPU as 60 s trends, then the USB link and the
//! ring buffer as captioned bars, closed by a one-line verdict and the uptime.
//! Link utilisation is referenced to the device's own USB ceiling
//! (`caps.sample_rate_max_hz`), not a magic constant.
//!
//! Split by **what each zone measures**, which is also the order a reader works
//! through it - what the radio delivered, what the host could take, what the
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
        let r = Rows::new(inner.width as usize, !state.radio.hw_streaming, theme);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    const W: u16 = 56;
    const H: u16 = 26;

    fn live() -> SdrMetrics {
        SdrMetrics::fixture().streaming().with_timing(0.3)
    }

    /// A stopped radio dashes every measured value rather than showing the last
    /// one it had - and says plainly that it is idle.
    #[test]
    fn a_stopped_radio_dashes_its_readings() {
        let out = draw(TimingVitalsPanel, W, H, &SdrMetrics::fixture()).join("\n");
        assert!(out.contains("---"), "{out}");
        assert!(out.contains("idle"), "no idle verdict:\n{out}");
        assert!(out.contains("STALE"), "the frame should be tagged:\n{out}");
    }

    /// All five zones are present, in the order the panel promises.
    #[test]
    fn the_zones_appear_in_order() {
        let out = draw(TimingVitalsPanel, W, H, &live()).join("\n");
        let drops = out.find("Sample drops").expect("no drops zone");
        let sat = out.find("ADC saturation").expect("no saturation zone");
        let cpu = out.find("CPU load").expect("no cpu zone");
        let usb = out.find("USB LINK").expect("no usb zone");
        let ring = out.find("RING BUFFER").expect("no ring zone");
        assert!(
            drops < sat && sat < cpu && cpu < usb && usb < ring,
            "zones out of order:\n{out}"
        );
    }

    /// The verdict follows the timing quality, and the three bands are reachable.
    #[test]
    fn the_verdict_follows_the_timing_quality() {
        let good = draw(TimingVitalsPanel, W, H, &live()).join("\n");
        assert!(good.contains("all vitals nominal"), "{good}");

        let strained = SdrMetrics::fixture().streaming().with_timing(1.6);
        let mid = draw(TimingVitalsPanel, W, H, &strained).join("\n");
        assert!(mid.contains("under load"), "{mid}");

        let mut dropping = live();
        dropping.signal.drops_per_sec = 8;
        let v = crate::state::TimingQuality::classify(9_000, 4_096, 0, 8);
        dropping.timing.timing_quality = v.quality;
        dropping.timing.timing_cause = v.cause;
        let bad = draw(TimingVitalsPanel, W, H, &dropping).join("\n");
        assert!(bad.contains("overrun logged"), "{bad}");
    }

    /// Link utilisation is referenced to the device's own ceiling, so the same
    /// throughput reads differently on a radio with a different maximum rate.
    #[test]
    fn link_utilisation_is_relative_to_the_device() {
        let mut m = live();
        m.timing.throughput_mean_mbps = 19.0; // half of a 20 Msps HackRF's ceiling
        let out = draw(TimingVitalsPanel, W, H, &m).join("\n");
        assert!(out.contains("38.1 max"), "ceiling missing:\n{out}");
        assert!(
            out.contains("50%") || out.contains("49%"),
            "utilisation should read about half:\n{out}"
        );
    }

    /// The panel fills whatever height it is given without spilling out of it.
    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let m = live();
        let (min_w, min_h) = TimingVitalsPanel.min_size();
        for (w, h) in [(min_w, min_h), (44, 20), (W, H), (100, 40)] {
            let out = draw(TimingVitalsPanel, w, h, &m);
            assert_eq!(out.len(), h as usize, "{w}x{h}: wrong row count");
            assert!(
                out.iter().all(|l| l.chars().count() <= w as usize),
                "{w}x{h}: a row overran the panel"
            );
        }
    }
}
