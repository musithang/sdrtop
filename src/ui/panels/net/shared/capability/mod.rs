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

use crate::hardware::DeliveryModel;
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

/// How blocks reach sdrtop, and so what every timing figure measures.
///
/// `hardware::DeliveryModel`'s own finding: a pull loop's gaps between reads
/// are our rhythm, not the link's, and a timing number that does not say which
/// it is will be read as the link's.
fn delivery<'a>(model: DeliveryModel, iw: usize, theme: &crate::Theme) -> Line<'a> {
    let (word, note) = match model {
        DeliveryModel::Push => ("push", "the driver paces blocks: timing is the link's"),
        DeliveryModel::Pull => ("pull", "sdrtop paces reads: timing is its own loop"),
    };
    fact(
        "DELIVERY",
        word.to_string(),
        Some(note.to_string()),
        iw,
        theme,
    )
}

/// Whether sdrtop could still watch this radio, read-only, while another
/// program holds it (`state::SystemState::observable`).
fn observer<'a>(observable: bool, iw: usize, theme: &crate::Theme) -> Line<'a> {
    let (word, note) = if observable {
        (
            "available",
            "watches it read-only when another program holds it",
        )
    } else {
        (
            "not available",
            "no read-only view when another program holds it",
        )
    };
    fact(
        "OBSERVER",
        word.to_string(),
        Some(note.to_string()),
        iw,
        theme,
    )
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
        let legend = lines.len() - 1;
        // What `signal::net::gate` actually holds the tuner to: every centre
        // the section tunes to, which is narrower than the ISM band drawn above.
        let needs = lines.len();
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
        lines.push(crate::ui::chrome::section("radio", "", iw, theme));
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
        lines.push(delivery(caps.delivery, iw, theme));
        lines.push(observer(state.system.observable, iw, theme));

        lines.extend(modes::lines(caps, iw, theme));

        // Breathe like the Lab panels (`chrome::fit_spacers`): spacers grow to
        // fill a tall panel and go first on a short one. When that is not
        // enough, the ruler's key, the NEEDS row, and then the band ruler's
        // two rows give way, in that order, so the modes, which are the
        // panel's answer, stay on screen longest.
        let avail = inner.height as usize;
        let blank = |l: &Line| l.spans.iter().all(|s| s.content.trim().is_empty());
        let spacers = lines.iter().filter(|l| blank(l)).count();
        let over = lines.len().saturating_sub(spacers).saturating_sub(avail);
        let mut optional = [legend, needs, legend - 1, legend - 2];
        let n = over.min(optional.len());
        optional[..n].sort_unstable_by(|a, b| b.cmp(a));
        for &i in &optional[..n] {
            lines.remove(i);
        }
        crate::ui::chrome::fit_spacers(&mut lines, avail);

        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use crate::state::fixture::draw;
    use crate::ui::NetCapabilityPanel;

    /// At full height. What a short panel gives up first is
    /// `a_short_panel_gives_up_the_key_and_needs_before_any_mode`.
    #[test]
    fn the_band_and_the_radios_own_range_are_both_named() {
        let m = crate::state::SdrMetrics::fixture();
        let out = draw(NetCapabilityPanel, 72, 32, &m).join("\n");
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

    /// **The modes are the panel's answer, so they are the last to go.** Tall,
    /// everything shows with room to breathe; short, the spacers go, then the
    /// ruler's key, then the NEEDS row, then the ruler, and every mode row is
    /// still there.
    #[test]
    fn a_short_panel_gives_up_the_key_and_needs_before_any_mode() {
        let m = crate::state::SdrMetrics::fixture();
        let tall = draw(NetCapabilityPanel, 90, 40, &m).join("\n");
        assert!(
            tall.contains("BLE advertising") && tall.contains("NEEDS"),
            "{tall}"
        );
        let short = draw(NetCapabilityPanel, 90, 21, &m).join("\n");
        assert!(!short.contains("BLE advertising"), "{short}");
        assert!(!short.contains("NEEDS"), "{short}");
        assert!(
            short.contains("RANGE"),
            "the tuner's range outlasts its ruler: {short}"
        );
        for phy in crate::signal::net::gate::PHYS {
            assert!(short.contains(phy.name), "{} lost:\n{short}", phy.name);
        }
    }

    /// The transport and observer facts, both ways round. The fixture is a
    /// HackRF: a push driver, and a radio observer mode can watch.
    #[test]
    fn the_radio_says_how_its_blocks_arrive_and_whether_it_can_be_watched() {
        let mut m = crate::state::SdrMetrics::fixture();
        let out = draw(NetCapabilityPanel, 90, 30, &m).join("\n");
        assert!(out.contains("DELIVERY push"), "{out}");
        assert!(out.contains("timing is the link's"), "{out}");
        assert!(out.contains("OBSERVER available"), "{out}");

        let mut caps = (*m.caps).clone();
        caps.delivery = crate::hardware::DeliveryModel::Pull;
        m.caps = std::sync::Arc::new(caps);
        m.system.observable = false;
        let out = draw(NetCapabilityPanel, 90, 30, &m).join("\n");
        assert!(out.contains("DELIVERY pull"), "{out}");
        assert!(out.contains("timing is its own loop"), "{out}");
        assert!(out.contains("OBSERVER not available"), "{out}");
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
