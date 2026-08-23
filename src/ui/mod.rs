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
