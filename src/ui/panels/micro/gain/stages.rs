// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The gain chain: the primary stage, the second stage where there is one, and
//! the front-end boost with the running total. A radio with more stages than
//! two rows hold (the Airspy R2's three) gets its knob on the first row, the
//! vendor curve or the front stage, and every stage by name on the second.
//!
//! Always drawn, streaming or not - gain is configured, not measured, so a
//! stopped radio still has a real answer here. That is the point of this view:
//! you set the gain before you press play.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::hardware::{GainModel, StageReading};
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
/// into each format string - `" LNA  "`, `" VGA   "`, `" AMP  "` - which lined up
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

    if gm.stages().len() > 2 {
        draw_many(f, primary, second, boost, state, fd);
        return;
    }

    draw_hbar(
        f,
        primary,
        r.primary_gain() as f64 / gm.primary_max_db().max(1) as f64,
        &format!(" {:<w$}  ", gm.primary_label()),
        &format!("{} dB", r.primary_gain()),
        theme.value,
        theme,
    );

    if gm.has_second_stage() {
        draw_hbar(
            f,
            second,
            r.secondary_gain() as f64 / VGA_MAX_DB,
            &format!(" {:<w$}  ", "VGA"),
            &format!("{} dB", r.secondary_gain()),
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

    // A dash where there is no boost, the same shape the VGA row uses when there
    // is no second stage. The row stays so the block keeps its height and the
    // three labels keep their column, and the dash says "not on this radio"
    // rather than "off".
    let boost_cell = if gm.has_boost() {
        Span::styled(
            if r.amp_enabled { "ON " } else { "OFF" },
            Style::default().fg(if r.amp_enabled {
                theme.status_ok
            } else {
                theme.value
            }),
        )
    } else {
        fd.dash()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            fd.padded(gm.boost_label(), w + 2),
            boost_cell,
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

/// A stage's reading as a row shows it: `17 dB`, or `6/14` where its dB was
/// not measured at the tuned frequency.
fn reading_text(reading: Option<StageReading>) -> String {
    match reading {
        Some(StageReading::Db(db)) => format!("{:.0} dB", db.max(0.0)),
        Some(position) => position.compact().trim().to_string(),
        None => "\u{2014}".to_string(),
    }
}

/// The rows for a chain of more than two stages: the knob, every stage, and
/// the total of all of them where all of them are dB.
fn draw_many(
    f: &mut Frame,
    knob: Rect,
    stages_row: Rect,
    total_row: Rect,
    state: &SdrMetrics,
    fd: &Field,
) {
    let gm = &state.caps.gain;
    let theme = fd.theme;
    let w = label_w(gm).max(3);
    let stages = gm.stages();

    // The knob: the vendor curve's step where it walks one, else the front
    // stage, by its reading.
    let (label, fill, value) = match state.curve_reading() {
        Some(c) => (
            c.short.to_string(),
            c.step as f64 / c.last.max(1) as f64,
            c.step_text(),
        ),
        None => {
            let front = stages.first().map_or("", |s| s.name.as_str());
            let fill = match state.stage_reading(0) {
                Some(StageReading::Position { at, of }) => at as f64 / of.max(1) as f64,
                _ => state.radio.stage_gain(0) / stages.first().map_or(1.0, |s| s.max_db.max(1.0)),
            };
            (
                front.to_string(),
                fill,
                reading_text(state.stage_reading(0)),
            )
        }
    };
    draw_hbar(
        f,
        knob,
        fill,
        &format!(" {label:<w$}  "),
        &value,
        theme.value,
        theme,
    );

    let mut line = vec![Span::raw(" ")];
    for (i, spec) in stages.iter().enumerate() {
        if i > 0 {
            line.push(fd.label(" \u{00b7} "));
        }
        line.push(fd.label(format!("{} ", spec.name)));
        line.push(Span::styled(
            reading_text(state.stage_reading(i)),
            Style::default().fg(theme.value),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(line)), stages_row);

    // The whole chain, every stage of it, and only where each is a dB here.
    let total = if state.chain_db_holds() {
        format!("{:.0} dB", state.radio.total_gain().max(0.0))
    } else {
        "\u{2014}".to_string()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            fd.label("Total: "),
            Span::styled(total, Style::default().fg(theme.value_hi)),
        ])),
        total_row,
    );
}

/// Front-end gain the whole chain is contributing, in dB. A single-stage tuner
/// has nothing to add to; a HackRF sums both stages and the amp.
fn total_gain_db(state: &SdrMetrics) -> i32 {
    let r = &state.radio;
    if state.caps.gain.is_single() {
        r.primary_gain() as i32
    } else {
        r.primary_gain() as i32
            + r.secondary_gain() as i32
            + if r.amp_enabled { AMP_GAIN_DB } else { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::native::{hackrf, rtlsdr};

    fn rtl() -> GainModel {
        rtlsdr::gain_model(&[0, 9, 14, 27, 37, 49])
    }

    fn chain() -> GainModel {
        GainModel::new(vec![], "RF", "RF").with_gauge_fallback(116)
    }

    /// A device with no boost gets a dash where the state would be, not `OFF`.
    ///
    /// `OFF` is an invitation to look for the key that turns it on. There is no
    /// such key on that radio, and the reader would be hunting for one.
    #[test]
    fn a_device_with_no_boost_shows_a_dash_rather_than_off() {
        let lines = crate::state::fixture::draw(
            crate::ui::MicroGainPanel,
            48,
            18,
            &crate::state::SdrMetrics::fixture()
                .streaming()
                .named_chain_no_boost(),
        )
        .join("\n");
        assert!(
            lines.contains("AGC"),
            "the row and its label stay:\n{lines}"
        );
        assert!(
            !lines.contains("AGC  OFF") && !lines.contains("AGC  ON"),
            "a boost this radio does not have:\n{lines}"
        );
        // And the device that does have one still says so.
        let hackrf = crate::state::fixture::draw(
            crate::ui::MicroGainPanel,
            48,
            18,
            &crate::state::SdrMetrics::fixture().streaming(),
        )
        .join("\n");
        assert!(hackrf.contains("AMP"), "{hackrf}");
    }

    /// Every label in the group fits the column, whichever device is attached.
    /// This is the alignment that was broken: `Tuner` is five characters and the
    /// literals were padded for three.
    #[test]
    fn the_label_column_fits_every_label_the_device_uses() {
        for gm in [hackrf::gain_model(), rtl(), chain()] {
            let w = label_w(&gm);
            for name in [gm.primary_label(), "VGA", gm.boost_label()] {
                assert!(
                    name.len() <= w,
                    "{name:?} does not fit the {w}-column label field"
                );
            }
        }
    }

    /// HackRF's three labels are all three characters, so its column is unchanged:
    /// the fix costs the common device nothing.
    #[test]
    fn the_hackrf_column_is_the_width_it_always_was() {
        assert_eq!(label_w(&hackrf::gain_model()), 3);
        assert_eq!(label_w(&rtl()), 5, "Tuner needs five");
    }

    /// A single-stage tuner has no second stage or amp to add, so its total is
    /// its one gain - including when the boost flag happens to be set.
    #[test]
    fn a_single_stage_total_is_just_its_own_gain() {
        let mut m = SdrMetrics::fixture().single_stage();
        m.radio.set_stage_gain(0, 27.0);
        m.radio.amp_enabled = true;
        assert_eq!(total_gain_db(&m), 27);
    }

    #[test]
    fn a_two_stage_total_sums_both_stages_and_the_amp() {
        let mut m = SdrMetrics::fixture();
        m.radio.set_stage_gain(0, 24.0);
        m.radio.set_stage_gain(1, 30.0);
        assert_eq!(total_gain_db(&m), 54);
        m.radio.amp_enabled = true;
        assert_eq!(total_gain_db(&m), 54 + AMP_GAIN_DB);
    }
}
