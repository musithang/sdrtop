//! `AdcLoadingPanel` - the ADC Loading column of the Lab RF bench ([6]).
//!
//! Shows how hard the 8-bit ADC is actually driven: the **signed sample histogram**
//! (a centred bell whose tails light up as they approach the rails), the **clip
//! headroom** bar, the **loading** read-out (peak / rms / crest / effective bits /
//! clip events), and a **modeled linearity** card (P1dB / IIP3 / IMD3 / SFDR). The
//! thesis: fill the ADC window without hitting the rails - that is what positions the
//! signal/noise gap the other two panels draw.
//!
//! - [`histogram`]: the bell, its calipers, the axis and the legend.
//! - [`readouts`]: the three fixed blocks under it.
//! - [`rf_bench`](super::rf_bench): the row vocabulary shared with `rf_chain`,
//!   the panel this one is the other half of.

mod histogram;
mod readouts;

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};
use crate::ui::rf_calc::{adc_loading, staging_verdict};

use super::rf_bench::severity_color;

pub struct AdcLoadingPanel;

/// Rows reserved below the bell: axis + caliper legend + the 3 read-out sections
/// (HEADROOM / LOADING / LINEARITY, with their spacers and caption).
const BELOW: usize = 17;

impl Panel for AdcLoadingPanel {
    fn name(&self) -> &'static str {
        "adc_loading"
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 18)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("ADC Loading")
            .stale_when(Staleness::NotStreaming)
            .tag_if(state.lab.rf_freeze.is_some(), Tag::Frozen)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if !state.radio.hw_streaming {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "\u{2014}\u{2014}\u{2014}",
                    Style::default().fg(theme.label),
                )),
                inner,
            );
            return;
        }

        let iw = inner.width as usize;
        let ih = inner.height as usize;

        // --- model (frozen snapshot when held, else live) ----------------------
        let fz = state.lab.rf_freeze.as_ref();
        let hist = fz
            .map(|f| &f.signed_hist)
            .unwrap_or(&state.iq.adc_signed_hist);
        let n: u64 = hist.iter().sum();
        let peak = fz
            .map(|f| f.peak_dbfs)
            .unwrap_or(state.signal.adc_peak_dbfs) as f64;
        let rms = fz.map(|f| f.rms_dbfs).unwrap_or(state.signal.adc_rms_dbfs) as f64;
        let clip = fz
            .map(|f| f.clip_events)
            .unwrap_or(state.signal.adc_clip_events);
        let (lna_g, vga_g) = fz
            .map(|f| (f.lna_gain, f.vga_gain))
            .unwrap_or((state.radio.lna_gain, state.radio.vga_gain));
        let bits = state.caps.sample_geometry.bits();
        let load = adc_loading(peak, rms, clip, n, bits);
        let (verdict, sev) = staging_verdict(peak);
        let sev_col = severity_color(sev, theme);

        let mut lines: Vec<Line> = Vec::new();

        // The bell is the panel's hero visual: it grows to fill all the height the
        // fixed read-out blocks below leave free, instead of capping low and
        // stranding empty rows.
        let chart_w = iw.saturating_sub(1).max(8);
        let chart_h = ih.saturating_sub(BELOW).clamp(3, 28);
        histogram::draw(
            &mut lines,
            hist,
            &histogram::Levels {
                peak_frac: 10f64.powf(peak / 20.0),
                sigma_frac: histogram::per_axis_sigma_frac(rms),
                clipping: load.clip_events > 0 || peak >= -1.0,
            },
            chart_w,
            chart_h,
            theme,
        );
        lines.push(Line::raw(""));

        readouts::headroom(&mut lines, peak, sev_col, iw, theme);
        lines.push(Line::raw(""));
        readouts::loading(&mut lines, &load, verdict, sev_col, iw, theme);
        lines.push(Line::raw(""));
        // The quantisation ceiling on that card follows the device, but its
        // IIP3, IMD3 and SFDR figures are a specific front end's datasheet
        // wearing a generic name. Drawn only where the chain is one sdrtop
        // actually models, which is the same flag the Friis cascade uses and for
        // the same reason.
        if state.caps.friis_applicable {
            readouts::linearity_card(&mut lines, lna_g, vga_g, bits, iw, theme);
        }

        // Self-adjusting density: collapse spacers when short, grow them to fill when
        // tall (chrome::fit_spacers), so the pane breathes the same at every height -
        // consistent with the other lab side panels.
        crate::ui::chrome::fit_spacers(&mut lines, ih);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_name_and_min_size() {
        let p = AdcLoadingPanel;
        assert_eq!(p.name(), "adc_loading");
        let (w, h) = p.min_size();
        assert!(w >= 16 && h >= 8);
    }

    /// The linearity card is a specific front end's datasheet. It is drawn for
    /// the radio it describes and for nothing else.
    ///
    /// Everything above it, the histogram and the loading read-out, is a real
    /// measurement of whatever converter is attached, so it stays.
    #[test]
    fn the_modeled_linearity_card_is_only_drawn_for_a_chain_we_model() {
        let hackrf = crate::state::fixture::draw(
            AdcLoadingPanel,
            60,
            30,
            &crate::state::SdrMetrics::fixture().streaming(),
        )
        .join("\n");
        assert!(hackrf.contains("LINEARITY"), "{hackrf}");
        assert!(hackrf.contains("IIP3"), "{hackrf}");

        let unknown = crate::state::fixture::draw(
            AdcLoadingPanel,
            60,
            30,
            &crate::state::SdrMetrics::fixture().streaming().soapy(),
        )
        .join("\n");
        assert!(
            !unknown.contains("IIP3"),
            "a modeled intercept point for a chain we do not know:\n{unknown}"
        );
        assert!(
            unknown.contains("LOADING"),
            "the measured half must survive:\n{unknown}"
        );
    }

    /// The converter depth reaches the read-out. A 14-bit device must not be
    /// described with an 8-bit ceiling.
    #[test]
    fn the_readout_names_the_devices_own_bit_depth() {
        let unknown = crate::state::fixture::draw(
            AdcLoadingPanel,
            60,
            30,
            &crate::state::SdrMetrics::fixture().streaming().soapy(),
        )
        .join("\n");
        assert!(unknown.contains("/ 14 eff"), "{unknown}");
    }
}
