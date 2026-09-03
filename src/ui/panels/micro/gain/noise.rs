// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Estimated noise figure and minimum discernible signal - what the current
//! staging costs in sensitivity.
//!
//! Both come from the Friis cascade, which needs a known front-end topology.
//! HackRF's three stages are documented; a single-tuner device's are not, so this
//! block reports nothing rather than a number it cannot stand behind.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::rf_calc::{estimate_mds_dbm, estimate_nf_db};

use super::super::field::Field;
use super::READOUT_W;

/// Noise figure (dB) at which the front end stops being quiet, then good.
const NF_GOOD_DB: f64 = 4.0;
const NF_FAIR_DB: f64 = 8.0;
/// MDS (dBm) below which the receiver is sensitive, then adequate. More negative
/// is better, so these read the other way round.
const MDS_GOOD_DBM: f64 = -95.0;
const MDS_FAIR_DBM: f64 = -85.0;

pub(super) fn draw(f: &mut Frame, nf_row: Rect, mds_row: Rect, state: &SdrMetrics, fd: &Field) {
    let theme = fd.theme;
    if !state.caps.friis_applicable {
        for (row, name) in [(nf_row, "Est. NF"), (mds_row, "MDS")] {
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw(" "),
                    fd.padded(name, READOUT_W),
                    fd.dash(),
                ])),
                row,
            );
        }
        return;
    }

    let r = &state.radio;
    let nf = estimate_nf_db(r.amp_enabled, r.primary_gain());
    let nf_color = if nf < NF_GOOD_DB {
        theme.status_ok
    } else if nf < NF_FAIR_DB {
        theme.status_warn
    } else {
        theme.status_crit
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            fd.padded("Est. NF", READOUT_W),
            Span::styled(format!("~{nf:.1} dB"), Style::default().fg(nf_color)),
        ])),
        nf_row,
    );

    let (text, color) = match estimate_mds_dbm(r.bb_filter_hz, nf) {
        Some(mds) => {
            let c = if mds < MDS_GOOD_DBM {
                theme.status_ok
            } else if mds < MDS_FAIR_DBM {
                theme.status_warn
            } else {
                theme.status_crit
            };
            (format!("~{mds:.0} dBm"), c)
        }
        // No baseband width reported means no bandwidth to integrate the noise
        // over, so there is no MDS to state.
        None => ("---".to_string(), theme.stale),
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            fd.padded("MDS", READOUT_W),
            Span::styled(text, Style::default().fg(color)),
        ])),
        mds_row,
    );
}
