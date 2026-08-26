//! The three read-out blocks under the bell: clip headroom, loading, and the
//! modeled linearity card.
//!
//! One module because they are one stack of fixed-height sections, all reading
//! the same `adc_loading` / `linearity` model, and all written in the shared
//! `rf_bench` row vocabulary.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::ui::chrome::section;
use crate::ui::rf_calc::{linearity, AdcLoading, OPT_PEAK_DBFS};

use super::super::rf_bench::{bar_row, bar_width, row, Bar, Row};

/// Label column for this panel's rows: clears `HDRM`, `peak`, `bits`, `clip`.
const LABEL_W: usize = 4;

/// Width reserved for the headroom bar's right-hand value.
const VALW: usize = 11;

/// Top of the headroom gauge, in dB below full scale.
const HDRM_MAX: f64 = 24.0;

/// `HEADROOM` — how far the peak sits below 0 dBFS, with a tick at the optimal
/// landing (`−OPT_PEAK_DBFS`, i.e. 8 dB of headroom).
pub(super) fn headroom(
    out: &mut Vec<Line<'static>>,
    peak: f64,
    sev_col: Color,
    iw: usize,
    theme: &crate::Theme,
) {
    let headroom = -peak;
    out.push(section("Headroom", "\u{2502} = optimal", iw, theme));
    out.push(bar_row(
        Bar {
            label: "HDRM",
            label_w: LABEL_W,
            value: headroom.clamp(0.0, HDRM_MAX) as u32,
            max: HDRM_MAX as u32,
            lo: theme.status_warn,
            hi: theme.status_ok,
            tick: Some(-OPT_PEAK_DBFS / HDRM_MAX),
            val_str: format!("{headroom:+.0} dB"),
            val_col: sev_col,
        },
        bar_width(iw, LABEL_W, VALW),
        theme,
    ));
}

/// `LOADING` — peak, rms, effective bits and clip events, with the staging
/// verdict on the last row.
pub(super) fn loading(
    out: &mut Vec<Line<'static>>,
    load: &AdcLoading,
    verdict: &str,
    sev_col: Color,
    iw: usize,
    theme: &crate::Theme,
) {
    let dim = theme.border_dim;
    let r = |label: &'static str, mid: String, mid_col: Color, right: String, right_col: Color| {
        row(
            Row {
                label,
                label_w: LABEL_W,
                mid,
                mid_col,
                right,
                right_col,
            },
            iw,
            theme,
        )
    };

    out.push(section("Loading", "peak / rms", iw, theme));
    out.push(r(
        "peak",
        format!("{:.0} dBFS", load.peak_dbfs),
        sev_col,
        format!("{}/127 cts", load.peak_counts),
        theme.value,
    ));
    out.push(r(
        "rms",
        format!("{:.0} dBFS", load.rms_dbfs),
        theme.value,
        format!("crest {:.1} dB", load.crest_db),
        theme.value,
    ));
    out.push(r(
        "bits",
        format!("{:.1} / 8 eff", load.enob),
        theme.value_hi,
        "ENOB".to_string(),
        dim,
    ));
    let (clip_txt, clip_col) = if load.clip_events == 0 {
        ("none".to_string(), theme.status_ok)
    } else {
        (format!("{} hits", load.clip_events), theme.status_crit)
    };
    let n_txt = if load.n >= 1000 {
        format!("{}k", load.n / 1000)
    } else {
        format!("{}", load.n)
    };
    out.push(r(
        "clip",
        format!("{clip_txt} / {n_txt}"),
        clip_col,
        verdict.to_string(),
        sev_col,
    ));
}

/// `LINEARITY` — modeled, not measured: P1dB headroom, IIP3 / IMD3 and SFDR
/// against the 8-bit limit. Followed by the panel's teaching caption.
pub(super) fn linearity_card(
    out: &mut Vec<Line<'static>>,
    lna_g: u32,
    vga_g: u32,
    iw: usize,
    theme: &crate::Theme,
) {
    let dim = theme.border_dim;
    let r = |label: &'static str, mid: String, mid_col: Color, right: String, right_col: Color| {
        row(
            Row {
                label,
                label_w: LABEL_W,
                mid,
                mid_col,
                right,
                right_col,
            },
            iw,
            theme,
        )
    };

    let lin = linearity(lna_g, vga_g);
    out.push(section("Linearity", "modeled", iw, theme));
    out.push(r(
        "P1dB",
        format!("{:.0} dB hdrm", lin.p1db_headroom_db),
        theme.value,
        "compression".to_string(),
        dim,
    ));
    out.push(r(
        "IIP3",
        format!("{:+.0} dBm", lin.iip3_dbm),
        theme.value,
        format!("IMD3 {:.0} dBc", lin.imd3_dbc),
        theme.value,
    ));
    out.push(r(
        "SFDR",
        format!("{:.0} dB", lin.sfdr_db),
        theme.value_hi,
        format!("8-bit \u{2264}{:.0}", lin.sfdr_limit_db),
        dim,
    ));

    // Teaching caption.
    out.push(Line::from(Span::styled(
        " fill the range without hitting the rails",
        Style::default().fg(dim),
    )));
}
