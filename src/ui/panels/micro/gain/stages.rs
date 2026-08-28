//! The gain chain: the primary stage, the second stage where there is one, and
//! the front-end boost with the running total.
//!
//! Always drawn, streaming or not — gain is configured, not measured, so a
//! stopped radio still has a real answer here. That is the point of this view:
//! you set the gain before you press play.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::hardware::GainModel;
use crate::state::SdrMetrics;
use crate::ui::widgets::charts::draw_hbar;

use super::super::field::Field;

/// Full-scale of the second-stage bar, in dB. HackRF's VGA range.
const VGA_MAX_DB: f64 = 62.0;
/// dB the HackRF's RF amp adds when enabled.
const AMP_GAIN_DB: i32 = 14;

/// Width of the label column these three rows share.
///
/// **Sized to the device's own labels**, not to a literal. It used to be baked
/// into each format string — `" LNA  "`, `" VGA   "`, `" AMP  "` — which lined up
/// only because those three names are all three characters. On an RTL-SDR the
/// primary stage is `Tuner`, and the three rows started in three different
/// columns: the bar at 8, the dash at 7, the boost at 6.
fn label_w(gm: &GainModel) -> usize {
    gm.primary_label()
        .len()
        .max("VGA".len())
        .max(gm.boost_label().len())
}

pub(super) fn draw(
    f: &mut Frame,
    primary: Rect,
    second: Rect,
    boost: Rect,
    state: &SdrMetrics,
    fd: &Field,
) {
    let r = &state.radio;
    let gm = &state.caps.gain;
    let theme = fd.theme;
    let w = label_w(gm);

    draw_hbar(
        f,
        primary,
        r.lna_gain as f64 / gm.primary_max_db().max(1) as f64,
        &format!(" {:<w$}  ", gm.primary_label()),
        &format!("{} dB", r.lna_gain),
        theme.value,
        theme,
    );

    if gm.has_second_stage() {
        draw_hbar(
            f,
            second,
            r.vga_gain as f64 / VGA_MAX_DB,
            &format!(" {:<w$}  ", "VGA"),
            &format!("{} dB", r.vga_gain),
            theme.value,
            theme,
        );
    } else {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                fd.padded("VGA", w + 2),
                fd.dash(),
            ])),
            second,
        );
    }

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            fd.padded(gm.boost_label(), w + 2),
            Span::styled(
                if r.amp_enabled { "ON " } else { "OFF" },
                Style::default().fg(if r.amp_enabled {
                    theme.status_ok
                } else {
                    theme.value
                }),
            ),
            Span::raw("    "),
            fd.label("Total: "),
            Span::styled(
                format!("{} dB", total_gain_db(state)),
                Style::default().fg(theme.value_hi),
            ),
        ])),
        boost,
    );
}

/// Front-end gain the whole chain is contributing, in dB. A single-stage tuner
/// has nothing to add to; a HackRF sums both stages and the amp.
fn total_gain_db(state: &SdrMetrics) -> i32 {
    let r = &state.radio;
    if state.caps.gain.is_single() {
        r.lna_gain as i32
    } else {
        r.lna_gain as i32 + r.vga_gain as i32 + if r.amp_enabled { AMP_GAIN_DB } else { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtl() -> GainModel {
        GainModel::RtlSingle {
            gain_steps_db: vec![0, 9, 14, 27, 37, 49],
        }
    }

    /// Every label in the group fits the column, whichever device is attached.
    /// This is the alignment that was broken: `Tuner` is five characters and the
    /// literals were padded for three.
    #[test]
    fn the_label_column_fits_every_label_the_device_uses() {
        for gm in [GainModel::HackRf, rtl()] {
            let w = label_w(&gm);
            for name in [gm.primary_label(), "VGA", gm.boost_label()] {
                assert!(
                    name.len() <= w,
                    "{name:?} does not fit the {w}-column label field"
                );
            }
        }
    }

    /// HackRF's three labels are all three characters, so its column is unchanged
    /// — the fix costs the common device nothing.
    #[test]
    fn the_hackrf_column_is_the_width_it_always_was() {
        assert_eq!(label_w(&GainModel::HackRf), 3);
        assert_eq!(label_w(&rtl()), 5, "Tuner needs five");
    }

    /// A single-stage tuner has no second stage or amp to add, so its total is
    /// its one gain — including when the boost flag happens to be set.
    #[test]
    fn a_single_stage_total_is_just_its_own_gain() {
        let mut m = SdrMetrics::fixture().single_stage();
        m.radio.lna_gain = 27;
        m.radio.amp_enabled = true;
        assert_eq!(total_gain_db(&m), 27);
    }

    #[test]
    fn a_two_stage_total_sums_both_stages_and_the_amp() {
        let mut m = SdrMetrics::fixture();
        m.radio.lna_gain = 24;
        m.radio.vga_gain = 30;
        assert_eq!(total_gain_db(&m), 54);
        m.radio.amp_enabled = true;
        assert_eq!(total_gain_db(&m), 54 + AMP_GAIN_DB);
    }
}
