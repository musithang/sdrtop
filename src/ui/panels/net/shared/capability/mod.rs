// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetCapabilityPanel` - what this radio can and cannot do in the 2.4 GHz band.
//!
//! The first panel of the section, and deliberately the one that needs no
//! stream. Almost everything on it comes from the capability record built when
//! the device was opened, so it is as true before the first sample as after the
//! millionth. The one exception is the RETUNE row, a measurement of the radio
//! itself taken when the user presses `K`, and dated on the row rather than
//! aged by the stream. That is why its staleness is [`Staleness::Never`]:
//! nothing here goes out of date because blocks stopped arriving.
//!
//! Zones, top to bottom: the verdict (`verdict`), TUNER (`span`), RADIO, and
//! the MODES limit rows (`modes`). On a short panel the detail gives way in a
//! stated order and the verdict and the modes stay longest.
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
use crate::state::{RetuneRun, SdrMetrics};
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::reading::Reading;

pub struct NetCapabilityPanel;

mod modes;
mod span;
mod verdict;

pub(crate) use verdict::headline;

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

/// The tuning call's measured duration, or that it has not been measured.
///
/// Never a default: before `K` it says "not measured" and how to measure it.
/// The figure is the call only (`signal::retune`'s doc says what that leaves
/// out), with the worst call beside the mean because a follower has to survive
/// the worst one, and dated, because it was taken once.
fn retune<'a>(run: Option<&RetuneRun>, iw: usize, theme: &crate::Theme) -> Line<'a> {
    let (value, note) = match run {
        None => (
            "not measured".to_string(),
            "[K] times the tuning call".to_string(),
        ),
        Some(RetuneRun::Measuring) => (
            "measuring".to_string(),
            "retuning across the band".to_string(),
        ),
        Some(RetuneRun::Done(m, at)) => {
            let reading = Reading::new(m.call_ms, "ms", f64::INFINITY).stated_to(2);
            let worst = m
                .worst_ms
                .map(|w| format!("worst {w:.2} ms, "))
                .unwrap_or_default();
            let ago = crate::ui::widgets::timing_fmt::ago(at.elapsed());
            (
                format!("{} call", reading.text()),
                format!(
                    "{worst}{} of {} calls, {ago}",
                    m.attempts - m.failed,
                    m.attempts
                ),
            )
        }
    };
    fact("RETUNE", value, Some(note), iw, theme)
}

impl Panel for NetCapabilityPanel {
    fn name(&self) -> &'static str {
        "net_capability"
    }

    fn min_size(&self) -> (u16, u16) {
        (48, 16)
    }

    /// `k`, the panel's own action letter: every letter of its name is some
    /// other panel's focus key or a global one, so the engine draws `[K]`.
    fn focus_key(&self) -> Option<char> {
        Some('k')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("K", "time the tuning call")]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        // Nothing here ages with the stream. The record was written when the
        // device was opened and is true until it is closed; the one measurement,
        // the tuning call, is of the radio, taken on request and dated on its row.
        // No mode tag, and this is the panel that proves the rule has an edge
        // rather than being applied by habit: nothing here was gathered from the
        // band, so there is no survey or lock to have gathered it in.
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
        lines.extend(verdict::lines(caps, state.net.retune.as_ref(), iw, theme));
        lines.push(Line::from(""));
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
        let delivery_row = lines.len();
        lines.push(delivery(caps.delivery, iw, theme));
        let observer_row = lines.len();
        lines.push(observer(state.system.observable, iw, theme));
        lines.push(retune(state.net.retune.as_ref(), iw, theme));

        lines.extend(modes::lines(caps, iw, theme));

        // Breathe like the Lab panels (`chrome::fit_spacers`): spacers grow to
        // fill a tall panel and go first on a short one. When that is not
        // enough, detail gives way in this order: the ruler's key, NEEDS, the
        // band ruler's two rows, the headroom row (the verdict says it), and
        // the observer and delivery rows. The verdict and the mode rows, which
        // are the panel's answer, stay on screen longest.
        let avail = inner.height as usize;
        let blank = |l: &Line| l.spans.iter().all(|s| s.content.trim().is_empty());
        let spacers = lines.iter().filter(|l| blank(l)).count();
        let over = lines.len().saturating_sub(spacers).saturating_sub(avail);
        let mut optional = [
            legend,
            needs,
            legend - 1,
            legend - 2,
            legend - 3,
            observer_row,
            delivery_row,
        ];
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

    /// **The verdict and the modes are the panel's answer, so they are the
    /// last to go.** Tall, everything shows with room to breathe; short, the
    /// spacers go, then the detail in its stated order, and the verdict and
    /// every mode row are still there.
    #[test]
    fn a_short_panel_gives_up_the_key_and_needs_before_any_mode() {
        let m = crate::state::SdrMetrics::fixture();
        let tall = draw(NetCapabilityPanel, 90, 40, &m).join("\n");
        assert!(
            tall.contains("BLE advertising") && tall.contains("NEEDS"),
            "{tall}"
        );
        let short = draw(NetCapabilityPanel, 90, 24, &m).join("\n");
        assert!(short.contains("5 OF 8 MODES"), "the verdict stays: {short}");
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

    /// Before `K`, "not measured" and how to measure, never a default; after,
    /// the call's mean with its uncertainty, the worst call and the count,
    /// said to be the call only.
    #[test]
    fn the_retune_row_is_not_measured_until_it_is() {
        use crate::signal::dsp::uncertainty::Uncertain;
        use crate::signal::retune::CallMeasurement;
        let mut m = crate::state::SdrMetrics::fixture();
        let out = draw(NetCapabilityPanel, 90, 32, &m).join("\n");
        assert!(out.contains("RETUNE   not measured"), "{out}");
        assert!(out.contains("[K] times the tuning call"), "{out}");

        m.net.retune = Some(crate::state::RetuneRun::Done(
            CallMeasurement {
                call_ms: Uncertain::from_sigma(1.84, 0.05),
                worst_ms: Some(2.1),
                attempts: 10,
                failed: 0,
                first_error: None,
            },
            std::time::Instant::now(),
        ));
        let out = draw(NetCapabilityPanel, 90, 32, &m).join("\n");
        assert!(out.contains("1.84 ±0.05 ms call"), "{out}");
        assert!(out.contains("worst 2.10 ms, 10 of 10 calls"), "{out}");
        assert!(!out.contains("not measured"), "{out}");
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
