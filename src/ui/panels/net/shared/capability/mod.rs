// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetCapabilityPanel` - what this radio can and cannot do in the 2.4 GHz band.
//!
//! The first panel of the section, and deliberately the one that needs no
//! stream. Everything on it comes from the capability record built when the
//! device was opened, so it is as true before the first sample as after the
//! millionth. That is why its staleness is [`Staleness::Never`]: nothing here
//! goes out of date while the radio is the radio.
//!
//! **It is a list of what is possible, not a promise about what will work.** A
//! mode is shown as available when the radio's declared sample-rate ceiling can
//! carry it. Whether a particular transmitter comes through at that rate is a
//! measurement, and the panels that make it arrive later.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::net::gate::{HIGHEST_CENTRE_HZ, LOWEST_CENTRE_HZ};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct NetCapabilityPanel;

mod modes;
mod span;

/// Label column width, so every `label value` row on the panel lines up.
const LABEL: usize = 9;

/// `LABEL    value   note`, the note drawn only when all of it fits in `iw`.
fn fact<'a>(
    label: &str,
    value: String,
    note: Option<String>,
    iw: usize,
    theme: &crate::Theme,
) -> Line<'a> {
    let used = LABEL + 1 + value.chars().count();
    let note = note.filter(|n| used + 3 + n.chars().count() <= iw);
    let mut spans = vec![
        crate::ui::chrome::field(label, LABEL, theme),
        Span::styled(value, Style::default().fg(theme.value)),
    ];
    if let Some(note) = note {
        spans.push(Span::styled(
            format!("   {note}"),
            Style::default().fg(theme.label),
        ));
    }
    Line::from(spans)
}

impl Panel for NetCapabilityPanel {
    fn name(&self) -> &'static str {
        "net_capability"
    }

    fn min_size(&self) -> (u16, u16) {
        (48, 16)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        // Nothing here is a reading, so nothing here can go stale. The record it
        // draws was written when the device was opened and is true until it is
        // closed.
        // No mode tag, and this is the panel that proves the rule has an edge
        // rather than being applied by habit: nothing here is a reading. It is
        // the capability record built when the device was opened, and it is the
        // same record whether the radio is hopping or parked.
        PanelChrome::new("Band Capability").stale_when(Staleness::Never)
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
        let caps = &state.caps;
        let mut lines = Vec::new();

        let iw = inner.width as usize;
        lines.extend(span::lines(caps, iw, theme));
        // What `signal::net::gate` actually holds the tuner to: every centre
        // the section tunes to, which is narrower than the ISM band drawn above.
        lines.push(fact(
            "NEEDS",
            format!(
                "{:.3} to {:.3} MHz",
                LOWEST_CENTRE_HZ as f64 / 1e6,
                HIGHEST_CENTRE_HZ as f64 / 1e6
            ),
            Some("every centre NET tunes to".to_string()),
            iw,
            theme,
        ));
        lines.push(Line::from(""));
        lines.push(fact(
            "RATE",
            format!("{:.3} Msps ceiling", caps.sample_rate_max_hz / 1e6),
            None,
            iw,
            theme,
        ));
        lines.push(fact(
            "SAMPLES",
            format!("{} bit", caps.sample_geometry.bits()),
            None,
            iw,
            theme,
        ));

        lines.extend(modes::lines(caps, iw, theme));

        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use crate::state::fixture::draw;
    use crate::ui::NetCapabilityPanel;

    #[test]
    fn the_band_and_the_radios_own_range_are_both_named() {
        let m = crate::state::SdrMetrics::fixture();
        let out = draw(NetCapabilityPanel, 72, 22, &m).join("\n");
        assert!(out.contains("2402.000 to 2483.500 MHz"), "{out}");
        // The fixture is a HackRF: 1 MHz to 6 GHz, 20 Msps.
        assert!(out.contains("6000.000 MHz"), "{out}");
        assert!(out.contains("covers the band"), "{out}");
        assert!(out.contains("20.000 Msps ceiling"), "{out}");
        assert!(out.contains("8 bit"), "{out}");
    }

    /// The whole point of the panel: the ceiling sorts the modes, and the ones
    /// that do not fit are shown as not fitting rather than left out. A user who
    /// cannot decode Wi-Fi should be able to see *why* on this screen.
    #[test]
    fn the_modes_are_sorted_by_what_the_rate_can_carry() {
        let m = crate::state::SdrMetrics::fixture();
        let out = draw(NetCapabilityPanel, 72, 24, &m).join("\n");
        let can = out
            .find("THIS RADIO CAN RECEIVE")
            .expect("available heading");
        let cannot = out
            .find("BEYOND ITS SAMPLE RATE")
            .expect("out-of-reach heading");
        assert!(can < cannot);
        let ht20 = out.find("802.11a/g/n HT20").expect("HT20 listed");
        let ht40 = out.find("802.11n HT40").expect("HT40 listed");
        assert!(ht20 < cannot, "HT20 fits in 20 Msps and must be above");
        assert!(ht40 > cannot, "HT40 needs 40 Msps and must be below");
    }

    #[test]
    fn nothing_here_goes_stale_because_nothing_here_is_a_reading() {
        let m = crate::state::SdrMetrics::fixture();
        // The fixture is deliberately not streaming, which is where every other
        // lab panel marks itself.
        assert!(!m.radio.hw_streaming);
        let out = draw(NetCapabilityPanel, 72, 22, &m).join("\n");
        assert!(!out.contains("STALE"), "{out}");
        assert!(out.contains("20.000 Msps ceiling"), "{out}");
    }
}
