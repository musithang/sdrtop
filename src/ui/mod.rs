// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Terminal UI: the layout machinery, the shared chrome and widgets, and the
//! panels themselves.
//!
//! The split is by role rather than by feature:
//!
//! - [`panel`], [`registry`], [`engine`]: the machinery. What a panel *is*, how
//!   panels are looked up by name, and how a preset's list of names becomes
//!   rectangles on screen.
//! - [`chrome`]: the shared frame vocabulary. Deck blocks, nameplates, section
//!   headings, fields, and the density helpers that let a line stack breathe or
//!   compress to any terminal height.
//! - [`widgets`]: drawing primitives with no opinion about layout.
//! - [`panels`]: the panels, grouped `core` / `lab` / `micro`.
//! - [`rf_calc`]: radio maths used by several panels, kept out of every one of
//!   their `render` bodies.
//! - [`overlay`], [`device_selector`]: full-screen UI that is not a panel.
//!
//! Panel structs are re-exported flat below, so `app::builder` registers them by
//! name without caring where they live.

pub mod chrome;
pub mod device_selector;
pub mod engine;
pub mod menu;
pub mod overlay;
pub mod panel;
pub mod panels;
pub mod registry;
pub mod rf_calc;
pub mod widgets;

pub use engine::LayoutEngine;
pub use registry::PanelRegistry;

pub use panels::core::command_rail::CommandRailPanel;
pub use panels::core::footer::FooterPanel;
pub use panels::core::header::{HeaderPanel, SlimHeaderPanel};
pub use panels::core::log::LogPanel;
pub use panels::core::observer::ObserverPanel;
pub use panels::core::signal_strip::SignalStripPanel;
pub use panels::core::spectrum::SpectrumPanel;
pub use panels::core::system_resources::SystemResourcesPanel;
pub use panels::core::waterfall::WaterfallPanel;

pub use panels::lab::adc_loading::AdcLoadingPanel;
pub use panels::lab::bars::{LabBannerPanel, LabMarkerPanel};
pub use panels::lab::fm_demod::FmDemodPanel;
pub use panels::lab::image_scope::ImageScopePanel;
pub use panels::lab::iq_constellation::IqConstellationPanel;
pub use panels::lab::iq_diagnostics::IqDiagnosticsPanel;
pub use panels::lab::iq_histogram::IqHistogramPanel;
pub use panels::lab::level_diagram::LevelDiagramPanel;
pub use panels::lab::rf_chain::RfChainPanel;
pub use panels::lab::signal_characterization::SignalCharacterizationPanel;
pub use panels::lab::signal_metrics::SignalMetricsPanel;
pub use panels::lab::sweep_panel::SweepPanel;
pub use panels::lab::sweep_strip::SweepStripPanel;
pub use panels::lab::timing_diagnostics::TimingDiagnosticsPanel;
pub use panels::lab::timing_stripchart::TimingStripchartPanel;
pub use panels::lab::timing_vitals::TimingVitalsPanel;

pub use panels::micro::entry::MicroPanel;
pub use panels::micro::gain::MicroGainPanel;
pub use panels::micro::health::MicroHealthPanel;
pub use panels::micro::signal::MicroSignalPanel;
pub use panels::micro::sweep::MicroSweepPanel;

#[cfg(test)]
mod gain_rendering {
    use crate::state::fixture::draw;
    use crate::state::SdrMetrics;

    /// Every panel that draws a gain, rendered against each device shape, with
    /// **distinct** values in each stage.
    ///
    /// G5 moved gain from two named fields to one vector indexed by position,
    /// and the failure that change invites is an off-by-one: a panel showing the
    /// VGA where the LNA belongs, or the same number twice. Equal values would
    /// hide exactly that, so the fixture uses 24 and 30 and every assertion
    /// names which one it expects.
    ///
    /// `draw` renders through `PanelRegistry::render_panel`, so the frame and
    /// the nameplate are part of what is compared.
    #[test]
    fn a_hackrf_shows_each_stage_where_it_belongs() {
        let m = SdrMetrics::fixture().streaming();
        assert_eq!(m.radio.primary_gain(), 24, "the fixture's LNA");
        assert_eq!(m.radio.secondary_gain(), 30, "the fixture's VGA");

        for (name, out) in [
            ("header", draw(super::HeaderPanel, 120, 5, &m).join("\n")),
            (
                "command_rail",
                draw(super::CommandRailPanel, 60, 30, &m).join("\n"),
            ),
            (
                "micro_gain",
                draw(super::MicroGainPanel, 44, 14, &m).join("\n"),
            ),
        ] {
            assert!(out.contains("24"), "{name}: no LNA value:\n{out}");
            assert!(out.contains("30"), "{name}: no VGA value:\n{out}");
            assert!(
                out.contains("LNA") && out.contains("VGA"),
                "{name}: a HackRF has both stages named:\n{out}"
            );
        }
    }

    /// One stage, and nothing must invent a second. The old model kept a `vga`
    /// number for this device that meant nothing; the vector simply has one
    /// entry, and the panels have to cope with that rather than read past it.
    #[test]
    fn an_rtl_sdr_has_one_stage_and_no_second_is_drawn() {
        let m = SdrMetrics::fixture().streaming().rtlsdr();
        assert_eq!(m.radio.primary_gain(), 25);
        assert_eq!(m.radio.secondary_gain(), 0, "there is no second stage");
        assert_eq!(m.radio.gains.len(), 1, "and no second value is stored");

        for (name, out) in [
            ("header", draw(super::HeaderPanel, 120, 5, &m).join("\n")),
            (
                "command_rail",
                draw(super::CommandRailPanel, 60, 30, &m).join("\n"),
            ),
            (
                "micro_gain",
                draw(super::MicroGainPanel, 44, 14, &m).join("\n"),
            ),
        ] {
            assert!(out.contains("25"), "{name}: no tuner value:\n{out}");
            // A panel may still name a second stage and dash it, which the micro
            // gain view does deliberately. What it must not do is print a second
            // gain figure, which is exactly what an off-by-one would produce.
            assert!(
                !out.contains(" 30"),
                "{name}: drew a second gain the device does not have:\n{out}"
            );
        }
    }

    /// A stage list with more entries than the two the panels were written for
    /// must not panic or read past the end. This is the shape a SoapySDR device
    /// with four elements arrives in.
    #[test]
    fn more_stages_than_the_panels_know_about_render_without_panicking() {
        let mut m = SdrMetrics::fixture().streaming().soapy();
        m.radio.gains = vec![12.0, 34.0, 56.0, 78.0];
        for (w, h) in [(120u16, 4u16), (60, 30), (44, 14), (40, 10)] {
            let _ = draw(super::HeaderPanel, w, 5, &m);
            let _ = draw(super::CommandRailPanel, w.max(40), h.max(20), &m);
            let _ = draw(super::MicroGainPanel, w.max(40), h.max(10), &m);
        }
    }

    /// And no stages at all, which is what a tuner that named no gains gives.
    #[test]
    fn an_empty_stage_list_renders_rather_than_panicking() {
        let mut m = SdrMetrics::fixture().streaming().rtlsdr();
        m.radio.gains.clear();
        assert_eq!(m.radio.primary_gain(), 0);
        let _ = draw(super::HeaderPanel, 120, 5, &m);
        let _ = draw(super::CommandRailPanel, 60, 30, &m);
        let _ = draw(super::MicroGainPanel, 44, 14, &m);
    }
}
