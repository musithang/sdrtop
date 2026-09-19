// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The verdict at the top of the panel, in `rf_chain`'s idiom: a marked word,
//! then a few plain sentences.
//!
//! **Composed only from the facts drawn below it**, so the verdict and the
//! panel cannot disagree: whether the tuner covers the band and with what
//! headroom (the TUNER zone), how many modes the sample-rate ceiling carries
//! and how far the nearest one out of reach is (the MODES zones), and, once
//! `K` has run, what the tuning call says about following a connection (the
//! RETUNE row). Nothing here is known that is not shown there.
//!
//! **The retune sentence is one-sided on purpose.** A tuning call slower than
//! the shortest BLE connection interval rules following out; a faster one is
//! necessary and not proof, because the call does not include the synthesiser
//! settling or the samples in flight (`signal::retune`). The sentence says
//! which of the two it is, and never "fast enough".

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::hardware::DeviceCapabilities;
use crate::signal::net::gate::{admits, reaches_band, PHYS};
use crate::signal::retune::MIN_CONNECTION_INTERVAL_MS;
use crate::state::RetuneRun;

/// How the verdict ranks the radio: every mode, some, or none, which is also
/// the colour and the mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grade {
    Every,
    Some,
    None,
}

/// The word and the sentences, as plain text: the table the tests read.
fn compose(caps: &DeviceCapabilities, retune: Option<&RetuneRun>) -> (Grade, String, Vec<String>) {
    let fit = PHYS.iter().filter(|p| admits(caps, p)).count();
    let covered = reaches_band(caps);
    let grade = match (covered, fit) {
        (false, _) | (_, 0) => Grade::None,
        (true, n) if n == PHYS.len() => Grade::Every,
        _ => Grade::Some,
    };
    let word = if !covered {
        "OUT OF BAND".to_string()
    } else {
        format!("{fit} OF {} MODES", PHYS.len())
    };

    let mut body = Vec::new();
    let (lo, hi) = (
        crate::signal::net::band::LOW_HZ as f64,
        crate::signal::net::band::HIGH_HZ as f64,
    );
    let (tmin, tmax) = (caps.freq_min_hz as f64, caps.freq_max_hz as f64);
    body.push(if covered {
        format!(
            "Tunes the whole band, {:.0} MHz of tuner below it and {:.0} MHz above.",
            (lo - tmin) / 1e6,
            (tmax - hi) / 1e6
        )
    } else {
        format!(
            "Tunes {:.0} to {:.0} MHz, which does not reach the 2.4 GHz band.",
            tmin / 1e6,
            tmax / 1e6
        )
    });

    let ceiling = caps.sample_rate_max_hz / 1e6;
    let nearest_out = PHYS
        .iter()
        .filter(|p| !admits(caps, p))
        .min_by(|a, b| a.rate_hz.total_cmp(&b.rate_hz));
    body.push(match nearest_out {
        None => format!("{ceiling:.1} Msps carries every mode listed."),
        Some(p) => format!(
            "{ceiling:.1} Msps carries {fit} of {}; the nearest out of reach, {}, is {:.1} Msps short.",
            PHYS.len(),
            p.name,
            p.rate_hz / 1e6 - ceiling
        ),
    });

    body.push(match retune {
        None => "Tuning call not timed yet: [K].".to_string(),
        Some(RetuneRun::Measuring) => "Timing the tuning call across the band.".to_string(),
        Some(RetuneRun::Done(m, _)) => match m.worst_ms {
            None => "Every tuning call was refused.".to_string(),
            Some(w) if w > MIN_CONNECTION_INTERVAL_MS => format!(
                "Tuning call up to {w:.2} ms, longer than a {MIN_CONNECTION_INTERVAL_MS} ms \
                 connection interval: it cannot follow the fastest connections."
            ),
            Some(w) => format!(
                "Tuning call up to {w:.2} ms, inside a {MIN_CONNECTION_INTERVAL_MS} ms \
                 connection interval: necessary for following, not proof of it."
            ),
        },
    });
    (grade, word, body)
}

/// The verdict block, wrapped to `iw` columns.
pub(super) fn lines(
    caps: &DeviceCapabilities,
    retune: Option<&RetuneRun>,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let (grade, word, body) = compose(caps, retune);
    let (mark, ink) = match grade {
        Grade::Every => ("\u{2713}", theme.status_ok),
        Grade::Some => ("\u{00b7}", theme.value_hi),
        Grade::None => ("\u{26a0}", theme.status_crit),
    };
    let mut out = vec![Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{mark} {word}"),
            Style::default().fg(ink).add_modifier(Modifier::BOLD),
        ),
    ])];
    let label = Style::default().fg(theme.label);
    for sentence in body {
        for row in crate::ui::chrome::wrap(&sentence, iw.saturating_sub(1), 3) {
            out.push(Line::from(vec![Span::raw(" "), Span::styled(row, label)]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::signal::retune::CallMeasurement;
    use crate::state::SdrMetrics;

    fn done(worst: Option<f64>) -> RetuneRun {
        RetuneRun::Done(
            CallMeasurement {
                call_ms: Uncertain::from_sigma(worst.unwrap_or(0.0), 0.1),
                worst_ms: worst,
                attempts: 10,
                failed: if worst.is_some() { 0 } else { 10 },
                first_error: None,
            },
            std::time::Instant::now(),
        )
    }

    /// The fixture, a HackRF: the whole band with the headroom the TUNER zone
    /// shows, five of eight modes, and 802.11b the nearest out of reach at
    /// 2 Msps short, the figure its limit row shows.
    #[test]
    fn the_verdict_says_what_the_zones_below_it_show() {
        let m = SdrMetrics::fixture();
        let (grade, word, body) = compose(&m.caps, None);
        assert_eq!(grade, Grade::Some);
        assert_eq!(word, "5 OF 8 MODES");
        assert!(
            body[0].contains("2399 MHz of tuner below it and 3516 MHz above"),
            "{body:?}"
        );
        assert!(
            body[1].contains("802.11b DSSS, is 2.0 Msps short"),
            "{body:?}"
        );
        assert!(body[2].contains("not timed yet"), "{body:?}");
    }

    /// The retune sentence is one-sided: slower than the interval rules
    /// following out, faster is necessary and never called enough.
    #[test]
    fn the_retune_sentence_never_promises_more_than_the_call_shows() {
        let m = SdrMetrics::fixture();
        let slow = compose(&m.caps, Some(&done(Some(9.3)))).2;
        assert!(slow[2].contains("cannot follow"), "{slow:?}");
        let fast = compose(&m.caps, Some(&done(Some(1.9)))).2;
        assert!(fast[2].contains("not proof"), "{fast:?}");
        assert!(!fast[2].to_lowercase().contains("fast enough"), "{fast:?}");
        let refused = compose(&m.caps, Some(&done(None))).2;
        assert!(refused[2].contains("refused"), "{refused:?}");
    }

    /// A radio that carries every mode reads as such, and one that does not
    /// reach the band reads as out of band whatever its rate.
    #[test]
    fn every_mode_and_out_of_band_are_their_own_words() {
        let mut caps = (*SdrMetrics::fixture().caps).clone();
        caps.sample_rate_max_hz = 100e6;
        let (grade, word, body) = compose(&caps, None);
        assert_eq!((grade, word.as_str()), (Grade::Every, "8 OF 8 MODES"));
        assert!(body[1].contains("every mode"), "{body:?}");
        caps.freq_max_hz = 1_766_000_000;
        let (grade, word, _) = compose(&caps, None);
        assert_eq!((grade, word.as_str()), (Grade::None, "OUT OF BAND"));
    }
}
