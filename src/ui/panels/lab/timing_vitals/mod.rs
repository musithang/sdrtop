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
        let feed = out.find("FFT FEED").expect("no feed zone");
        assert!(
            drops < sat && sat < cpu && cpu < usb && usb < feed,
            "zones out of order:\n{out}"
        );
    }

    /// A block the FFT feed refused is a frame the spectrum never drew, and it
    /// is the only record that the block existed. The whole point of counting it
    /// is that it reaches a panel, so the path is pinned end to end here.
    #[test]
    fn a_block_the_spectrum_lost_is_shown_and_not_only_counted() {
        let quiet = draw(TimingVitalsPanel, W, H, &live()).join("\n");
        assert!(quiet.contains("Blocks dropped"), "{quiet}");
        assert!(
            quiet.contains("session 0"),
            "a clean feed says so:\n{quiet}"
        );

        let mut lossy = live();
        lossy.iq.fft_drops = 3;
        lossy.iq.fft_drops_session = 47;
        let out = draw(TimingVitalsPanel, W, H, &lossy).join("\n");
        assert!(out.contains("session 47"), "{out}");
    }

    /// The verdict follows the timing quality, and the three bands are reachable.
    /// The radio's own count sits under the drop trend: absent on a radio
    /// that keeps none, the reason on one that could not be asked, and the
    /// count with its longest drop in time otherwise.
    #[test]
    fn what_the_radio_dropped_is_shown_under_the_drops() {
        let quiet = draw(TimingVitalsPanel, W, H, &live()).join("\n");
        assert!(!quiet.contains("In the radio"), "{quiet}");

        let mut m = live();
        m.signal.radio_drops = crate::state::RadioDropAccount::Unreadable("old firmware".into());
        let out = draw(TimingVitalsPanel, W, H, &m).join("\n");
        assert!(out.contains("not counted: old firmware"), "{out}");

        m.radio.config_sample_rate = 20e6;
        m.signal.radio_drops = crate::state::RadioDropAccount::Counted {
            stream: 12,
            window: 2,
            longest_bytes: 435_000,
        };
        let out = draw(TimingVitalsPanel, W, H, &m).join("\n");
        assert!(out.contains("In the radio 12"), "{out}");
        assert!(out.contains("the longest 10.9 ms"), "{out}");
    }

    /// **Whose loss it was is said**, once there is a loss: the driver's own
    /// (libairspy's queue, a HackRF's short transfer, a SoapySDR overflow)
    /// apart from sdrtop's intake refusing a block. A clean session has no
    /// such line.
    #[test]
    fn a_loss_says_whether_the_driver_or_sdrtop_lost_it() {
        let quiet = draw(TimingVitalsPanel, W, H, &live()).join("\n");
        assert!(!quiet.contains("driver"), "{quiet}");
        let mut m = live();
        m.signal.total_drops_session = 4_063_232;
        m.signal.driver_drops_session = 4_063_232;
        let out = draw(TimingVitalsPanel, W, H, &m).join("\n");
        assert!(out.contains("driver 4063232 \u{00b7} sdrtop 0"), "{out}");
        // At the live bench's 44 columns, seven-digit counts both ways: the
        // line was cut there once, showing 122880 for 1228800.
        m.signal.total_drops_session = 4_866_048;
        m.signal.driver_drops_session = 3_637_248;
        let out = draw(TimingVitalsPanel, 46, H, &m).join("\n");
        assert!(
            out.contains("driver 3637248 \u{00b7} sdrtop 1228800"),
            "{out}"
        );
    }

    /// **The bus line counts what crosses the link**: an R2 packing 12-bit
    /// samples carries 3 bytes on the wire for every 4-byte pair sdrtop gets,
    /// so 38.1 MB/s delivered is 28.6 on the bus, of a 28.6 most; a backend
    /// that cannot say what its wire carries shows what it delivered, under
    /// that name; a HackRF reads as it always did.
    #[test]
    fn the_bus_line_counts_the_wire_not_the_delivery() {
        let mut r2 = live().band_measured_chain();
        r2.caps = std::sync::Arc::new(crate::hardware::native::airspy::caps());
        r2.timing.throughput_mean_mbps = 38.147;
        r2.timing.wire_bytes_per_pair = Some(3.0);
        let out = draw(TimingVitalsPanel, W, H, &r2).join("\n");
        assert!(
            out.contains("Bus throughput 28.6 MB/s of 28.6 max"),
            "{out}"
        );
        r2.timing.wire_bytes_per_pair = None;
        let out = draw(TimingVitalsPanel, W, H, &r2).join("\n");
        assert!(out.contains("Delivered 38.1 MB/s"), "{out}");
        assert!(!out.contains("Bus throughput"), "{out}");
        let mut hackrf = live();
        hackrf.timing.throughput_mean_mbps = 38.147;
        hackrf.timing.wire_bytes_per_pair = Some(2.0);
        let out = draw(TimingVitalsPanel, W, H, &hackrf).join("\n");
        assert!(
            out.contains("Bus throughput 38.1 MB/s of 38.1 max"),
            "{out}"
        );
    }

    #[test]
    fn the_verdict_follows_the_timing_quality() {
        let good = draw(TimingVitalsPanel, W, H, &live()).join("\n");
        assert!(good.contains("all vitals nominal"), "{good}");

        let strained = SdrMetrics::fixture().streaming().with_timing(1.6);
        let mid = draw(TimingVitalsPanel, W, H, &strained).join("\n");
        assert!(mid.contains("under load"), "{mid}");

        let mut dropping = live();
        dropping.signal.drops_per_sec = 8;
        let v = crate::state::TimingQuality::classify(
            9_000,
            4_096,
            0,
            8,
            crate::hardware::DeliveryModel::Push,
            None,
            None,
        );
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
