//! `micro_gain` — the field gain-staging view (`[0]` cycle, 3rd step).
//!
//! For setting gain fast on arrival: wide primary/second-stage bars, prominent
//! ADC utilisation, and a central gain-advisor verdict, with estimated NF and MDS
//! for context.
//!
//! Split by the question each block answers — what the chain is set to, what that
//! is doing to the converter, what it costs in sensitivity, and what to do about
//! it:
//!
//! - [`stages`]: the gain chain and its total.
//! - [`adc`]: utilisation and saturation.
//! - [`noise`]: estimated NF and MDS, where the device's topology allows one.
//! - [`advisor`]: the headline verdict.

mod adc;
mod advisor;
mod noise;
mod stages;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome};

use super::field::{self, Field};

/// Label column shared by the four readout rows below the gain bars. Wide enough
/// for `ADC util`, the longest of them.
const READOUT_W: usize = 10;

pub struct MicroGainPanel;

impl Panel for MicroGainPanel {
    fn name(&self) -> &'static str {
        "micro_gain_panel"
    }
    fn min_size(&self) -> (u16, u16) {
        (40, 8)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::untitled()
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let fd = Field::new(state, theme);

        // 12 stacked rows; trailing Min(0) absorbs extra height.
        let rows: Vec<Rect> = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // 0 header
                Constraint::Length(1), // 1 blank
                Constraint::Length(1), // 2 primary gain bar
                Constraint::Length(1), // 3 second stage
                Constraint::Length(1), // 4 boost / total
                Constraint::Length(1), // 5 blank
                Constraint::Length(1), // 6 ADC util
                Constraint::Length(1), // 7 SAT
                Constraint::Length(1), // 8 NF
                Constraint::Length(1), // 9 MDS
                Constraint::Length(1), // 10 blank
                Constraint::Length(1), // 11 advisor
                Constraint::Min(0),
            ])
            .split(inner)
            .to_vec();

        f.render_widget(Paragraph::new(field::header(state, theme)), rows[0]);
        stages::draw(f, rows[2], rows[3], rows[4], state, &fd);
        adc::draw(f, rows[6], rows[7], state, &fd);
        noise::draw(f, rows[8], rows[9], state, &fd);
        advisor::draw(f, rows[11], state, &fd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    const W: u16 = 64;
    const H: u16 = 18;

    fn driven() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        for (i, b) in m.iq.iq_amplitude_hist.iter_mut().enumerate() {
            *b = if (10..24).contains(&i) { 900 } else { 20 };
        }
        m
    }

    /// The three gain rows are read as a column, so they must start in the same
    /// place — on either device family. On an RTL-SDR the primary stage is
    /// `Tuner`, five characters against the HackRF's three, and the rows used to
    /// start at columns 8, 7 and 6.
    #[test]
    fn the_gain_rows_line_up_on_both_device_families() {
        /// Column the content starts at: past the lead space, the label, and the
        /// padding after it.
        fn content_col(row: &str) -> usize {
            let body: Vec<char> = row.trim_matches('\u{2502}').chars().collect();
            let mut i = 0;
            while i < body.len() && body[i] == ' ' {
                i += 1;
            }
            while i < body.len() && body[i] != ' ' {
                i += 1;
            }
            while i < body.len() && body[i] == ' ' {
                i += 1;
            }
            i
        }

        for m in [driven(), driven().single_stage()] {
            let out = draw(MicroGainPanel, W, H, &m);
            // Rows 3, 4, 5 of the buffer are the primary bar, the second stage
            // and the boost line (row 0 is the frame, 1 the header, 2 blank).
            let cols: Vec<usize> = out[3..6].iter().map(|r| content_col(r)).collect();
            assert!(
                cols.windows(2).all(|w| w[0] == w[1]),
                "{}: the three gain rows start at {cols:?}\n{}",
                m.caps.gain.primary_label(),
                out.join("\n")
            );
        }
    }

    /// A single-stage device has no VGA and no Friis cascade, and says so rather
    /// than printing a number it cannot stand behind.
    #[test]
    fn a_single_stage_device_declines_the_rows_it_cannot_fill() {
        let out = draw(MicroGainPanel, W, H, &driven().single_stage()).join("\n");
        assert!(out.contains("Tuner"), "primary label missing:\n{out}");
        assert!(out.contains("AGC"), "boost label missing:\n{out}");
        assert!(
            out.lines().any(|l| l.contains("VGA") && l.contains("---")),
            "the absent second stage should be a dash:\n{out}"
        );
        assert!(
            out.lines()
                .any(|l| l.contains("Est. NF") && l.contains("---")),
            "NF is not estimable without a known cascade:\n{out}"
        );
    }

    /// A HackRF fills all of them.
    #[test]
    fn a_two_stage_device_fills_every_row() {
        let out = draw(MicroGainPanel, W, H, &driven()).join("\n");
        for want in [
            "LNA", "VGA", "AMP", "Total:", "ADC util", "SAT", "Est. NF", "MDS",
        ] {
            assert!(out.contains(want), "{want} missing:\n{out}");
        }
        assert!(out.contains("Total: 54 dB"), "{out}");
    }

    /// Stopped, everything measured reads as a dash — but the gain itself does
    /// not, because it is configured rather than measured.
    #[test]
    fn a_stopped_radio_still_shows_the_gain_it_is_set_to() {
        let out = draw(MicroGainPanel, W, H, &SdrMetrics::fixture()).join("\n");
        assert!(
            out.contains("24 dB"),
            "the LNA setting should survive:\n{out}"
        );
        assert!(out.contains("RX not streaming"), "{out}");
        assert!(
            out.lines()
                .any(|l| l.contains("ADC util") && l.contains("---")),
            "utilisation needs a stream:\n{out}"
        );
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let m = driven();
        let (min_w, min_h) = MicroGainPanel.min_size();
        for (w, h) in [(min_w, min_h), (44, 10), (W, H), (100, 30)] {
            let out = draw(MicroGainPanel, w, h, &m);
            assert_eq!(out.len(), h as usize, "{w}x{h}: wrong row count");
            assert!(
                out.iter().all(|l| l.chars().count() <= w as usize),
                "{w}x{h}: a row overran the panel"
            );
        }
    }
}
