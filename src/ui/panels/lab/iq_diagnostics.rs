use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::image_rejection_db;
use crate::state::SdrMetrics;
use crate::ui::chrome::section;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::charts::{gain_bar_colored, null_meter};

pub struct IqDiagnosticsPanel;

fn offset_color(abs_val: f32, theme: &crate::Theme) -> Color {
    if abs_val > 0.02 {
        theme.status_crit
    } else if abs_val > 0.005 {
        theme.status_warn
    } else {
        theme.status_ok
    }
}

fn imbalance_color(abs_db: f32, theme: &crate::Theme) -> Color {
    if abs_db > 3.0 {
        theme.status_crit
    } else if abs_db > 1.0 {
        theme.status_warn
    } else {
        theme.status_ok
    }
}

fn phase_color(abs_deg: f32, theme: &crate::Theme) -> Color {
    if abs_deg > 5.0 {
        theme.status_crit
    } else if abs_deg > 2.0 {
        theme.status_warn
    } else {
        theme.status_ok
    }
}

fn irr_color(irr_db: f64, theme: &crate::Theme) -> Color {
    if irr_db >= 30.0 {
        theme.status_ok
    } else if irr_db >= 20.0 {
        theme.status_warn
    } else {
        theme.status_crit
    }
}

fn spike_color(spike_dbfs: f64, theme: &crate::Theme) -> Color {
    if spike_dbfs < -40.0 {
        theme.status_ok
    } else if spike_dbfs < -20.0 {
        theme.status_warn
    } else {
        theme.status_crit
    }
}

use crate::ui::widgets::micro_common::spark_minmax;

/// DC spike level in dBFS: how tall the centre-frequency spike is in the spectrum.
///   DC spike = 20·log₁₀(dc_magnitude)
/// Returns None when dc_mag is zero (no spike).
fn dc_spike_dbfs(dc_mag: f64) -> Option<f64> {
    if dc_mag <= 0.0 {
        return None;
    }
    Some(20.0 * dc_mag.log10())
}

impl Panel for IqDiagnosticsPanel {
    fn name(&self) -> &'static str {
        "iq_diagnostics"
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 12)
    }
    fn focus_key(&self) -> Option<char> {
        Some('i')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("D", "DC-block"),
            ("C", "auto-cal"),
            ("F", "freeze"),
            ("M", "pin"),
        ]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("_IQ Diagnostics").stale_when(Staleness::NotStreaming)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let stale = !state.radio.hw_streaming;

        if stale {
            f.render_widget(
                Paragraph::new(Span::styled("---", Style::default().fg(theme.label))),
                inner,
            );
            return;
        }

        let iw = inner.width as usize;
        let dim = theme.border_dim;
        let lbl_st = Style::default().fg(theme.label);

        // Fixed label field (3-wide) + a right value budget, so every meter/bar
        // starts and ends at the same column and the readings line up. The meter
        // (arrows + track) and the gradient bar share one visual width (`field_w`).
        const LEAD: usize = 5; // " LBL " = space + 3 + space
        const VALUE_W: usize = 10;
        let field_w = iw.saturating_sub(LEAD + 1 + VALUE_W).max(8);
        let track_w = field_w.saturating_sub(2); // null_meter adds 2 arrow columns

        // ├╴ SECTION ╶──── hint — nameplate with a dim right-aligned annotation,
        // the same instrument language as the command rail.
        // A plain " text ………… value" readout line, value right-aligned to the panel.
        let readout = |text: &str, val: String, color: Color| -> Line<'static> {
            let pad = iw.saturating_sub(1 + text.chars().count() + val.chars().count());
            Line::from(vec![
                Span::raw(" "),
                Span::styled(text.to_string(), lbl_st),
                Span::raw(" ".repeat(pad.max(1))),
                Span::styled(val, Style::default().fg(color).add_modifier(Modifier::BOLD)),
            ])
        };
        // Filled cockpit chip — same pill style as the command-rail mode tabs.
        // `active` lights it (Step 5 wires DC-block / auto-cal state here).
        let chip = |label: &str, active: bool| -> Span<'static> {
            let bg = if active {
                theme.value_hi
            } else {
                theme.border_dim
            };
            let mut st = Style::default().bg(bg).fg(Color::Rgb(4, 6, 15));
            if active {
                st = st.add_modifier(Modifier::BOLD);
            }
            Span::styled(format!(" {label} "), st)
        };
        // " LBL " + bipolar null-meter + "  value"
        let meter_row = |label: &str,
                         value: f64,
                         full_scale: f64,
                         color: Color,
                         val_str: String|
         -> Line<'static> {
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(format!("{label:<3}"), lbl_st),
                Span::raw(" "),
            ];
            spans.extend(null_meter(value, full_scale, track_w, color, dim));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                val_str,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
            Line::from(spans)
        };
        // " LBL " + gradient quality bar + "  value" (frac 0..1 maps the fill).
        let bar_row = |label: &str,
                       frac: f64,
                       lo: Color,
                       hi: Color,
                       val_color: Color,
                       val_str: String|
         -> Line<'static> {
            let v = (frac.clamp(0.0, 1.0) * 1000.0) as u32;
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(format!("{label:<3}"), lbl_st),
                Span::raw(" "),
            ];
            spans.extend(gain_bar_colored(v, 1000, field_w, lo, hi, dim));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                val_str,
                Style::default().fg(val_color).add_modifier(Modifier::BOLD),
            ));
            Line::from(spans)
        };

        let mut lines: Vec<Line> = Vec::new();

        // --- DC OFFSET ---------------------------------------------------------
        // I / Q offsets are deviations from zero → null-meters; magnitude is a
        // green→red quality bar; the spike is a plain level readout.
        lines.push(section("DC offset", "target \u{00b1}0.010", iw, theme));
        let i_color = offset_color(state.iq.dc_offset_i.abs(), theme);
        let q_color = offset_color(state.iq.dc_offset_q.abs(), theme);
        lines.push(meter_row(
            "I",
            state.iq.dc_offset_i as f64,
            0.05,
            i_color,
            format!("{:+.4}", state.iq.dc_offset_i),
        ));
        lines.push(Line::raw(""));
        lines.push(meter_row(
            "Q",
            state.iq.dc_offset_q as f64,
            0.05,
            q_color,
            format!("{:+.4}", state.iq.dc_offset_q),
        ));
        lines.push(Line::raw(""));

        let dc_mag = (state.iq.dc_offset_i as f64).hypot(state.iq.dc_offset_q as f64);
        let dc_color = offset_color(dc_mag as f32, theme);
        lines.push(bar_row(
            "MAG",
            dc_mag / 0.05,
            theme.status_ok,
            theme.status_crit,
            dc_color,
            format!("{dc_mag:.4}"),
        ));
        lines.push(Line::raw(""));

        let spike = dc_spike_dbfs(dc_mag);
        let (spike_str, spike_col) = match spike {
            Some(s) => (format!("{s:.1} dBFS"), spike_color(s, theme)),
            None => ("\u{2014}".to_string(), theme.label),
        };
        lines.push(readout("DC spike @ LO", spike_str, spike_col));

        lines.push(Line::raw(""));

        // --- QUADRATURE --------------------------------------------------------
        // Amplitude / phase imbalance are deviations from balance → null-meters;
        // IRR (higher = better) is a red→green quality bar.
        lines.push(section(
            "Quadrature",
            "gain \u{00b7} phase balance",
            iw,
            theme,
        ));
        let amp_abs = state.iq.iq_imbalance_db.abs();
        lines.push(meter_row(
            "AMP",
            state.iq.iq_imbalance_db as f64,
            4.0,
            imbalance_color(amp_abs, theme),
            format!("{:+.2} dB", state.iq.iq_imbalance_db),
        ));
        lines.push(Line::raw(""));
        let phase_abs = state.iq.phase_imbalance_deg.abs();
        lines.push(meter_row(
            "PHA",
            state.iq.phase_imbalance_deg as f64,
            6.0,
            phase_color(phase_abs, theme),
            format!("{:+.2}\u{b0}", state.iq.phase_imbalance_deg),
        ));
        lines.push(Line::raw(""));

        // --- IMAGE REJECTION ---------------------------------------------------
        lines.push(section(
            "Image rejection",
            "IRR \u{00b7} higher better",
            iw,
            theme,
        ));
        lines.push(Line::raw(""));
        let irr = image_rejection_db(state.iq.iq_imbalance_db, state.iq.phase_imbalance_deg);
        let irr_str = if irr >= 60.0 {
            "> 60 dB".to_string()
        } else {
            format!("{irr:.1} dB")
        };
        lines.push(bar_row(
            "IRR",
            irr / 60.0,
            theme.status_crit,
            theme.status_ok,
            irr_color(irr, theme),
            irr_str,
        ));
        lines.push(Line::raw(""));
        // 60 s trend sparkline, auto-scaled so the IRR jitter is visible even when
        // it sits high and flat. Annotated with the window's peak-to-peak spread.
        let irr_hist: Vec<f32> = state.iq.irr_history.iter().copied().collect();
        // Size the sparkline to leave room for the " trend " prefix (7) and the
        // "±x.x dB/60s" annotation, so the whole row fits in iw (the bars reserve
        // their own value column; the trend's annotation is wider, so it can't reuse
        // field_w or it overruns the right edge).
        const TREND_ANN_W: usize = 13; // budget for "±NN.N dB/60s"
        let spark_w = iw.saturating_sub(7 + 1 + TREND_ANN_W).max(1);
        let (spark, p2p) = spark_minmax(&irr_hist, spark_w);
        if !spark.is_empty() {
            let ann = format!("\u{00b1}{:.1} dB/60s", p2p / 2.0);
            let trend_pad = iw.saturating_sub(7 + spark.chars().count() + ann.chars().count());
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled("trend", lbl_st),
                Span::raw(" "),
                Span::styled(spark, Style::default().fg(irr_color(irr, theme))),
                Span::raw(" ".repeat(trend_pad.max(1))),
                Span::styled(ann, Style::default().fg(dim)),
            ]));
        }

        lines.push(Line::raw(""));

        // --- VERDICT BLOCK -----------------------------------------------------
        // Most-severe issue gets a titled, plain-language explanation. Metrics are the
        // RESIDUAL after any active correction, so a lit chip + a still-bad reading
        // means the correction is not keeping up (re-run it); a clean reading with
        // corrections active gets a "corrections active" ✓ instead.
        let cal = &state.iq.cal;
        let quad_bad = amp_abs > 3.0 || phase_abs > 5.0;
        let dc_bad = dc_mag > 0.02;
        let minor = amp_abs > 1.0 || phase_abs > 2.0;
        let irr_txt = if irr >= 60.0 {
            "> 60".to_string()
        } else {
            format!("{irr:.0}")
        };
        let spk_txt = spike
            .map(|s| format!("{s:.1}"))
            .unwrap_or_else(|| "\u{2014}".into());

        let push_title = |lines: &mut Vec<Line<'static>>, mark: &str, text: &str, col: Color| {
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("{mark} {text}"),
                    Style::default().fg(col).add_modifier(Modifier::BOLD),
                ),
            ]));
        };
        let push_body = |lines: &mut Vec<Line<'static>>, text: String| {
            lines.push(Line::from(vec![Span::raw(" "), Span::styled(text, lbl_st)]));
        };
        let push_ok = |lines: &mut Vec<Line<'static>>, text: String| {
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(text, Style::default().fg(theme.status_ok)),
            ]));
        };

        if quad_bad {
            push_title(
                &mut lines,
                "\u{26a0}",
                "QUADRATURE IMBALANCE",
                theme.status_crit,
            );
            push_body(
                &mut lines,
                format!("I/Q off balance \u{2192} image only \u{2212}{irr_txt} dB."),
            );
            if cal.cal_applied {
                push_body(
                    &mut lines,
                    "Auto-cal on but residual remains \u{2014} re-run [C].".into(),
                );
            } else {
                push_body(&mut lines, "Run auto-cal [C] to correct quadrature.".into());
            }
        } else if dc_bad {
            push_title(&mut lines, "\u{26a0}", "DC OFFSET HIGH", theme.status_warn);
            push_body(
                &mut lines,
                format!("I/Q centroid off-zero \u{2192} DC spike {spk_txt} dBFS at LO."),
            );
            if cal.dc_block_on {
                push_body(
                    &mut lines,
                    "DC-block on but residual offset remains.".into(),
                );
            } else {
                push_body(&mut lines, "Press [D] to block the DC spike.".into());
            }
        } else if minor {
            push_title(&mut lines, "\u{00b7}", "MINOR IMBALANCE", theme.status_warn);
            push_body(
                &mut lines,
                "Within tolerance \u{2014} watch the image level.".into(),
            );
        } else {
            push_title(&mut lines, "\u{2713}", "IQ QUALITY OK", theme.status_ok);
            if cal.cal_applied || cal.dc_block_on {
                push_ok(&mut lines, format!("Corrections active \u{00b7} image \u{2212}{irr_txt} dB \u{00b7} DC centred."));
            } else {
                push_body(&mut lines, format!("Quadrature balanced \u{00b7} image \u{2212}{irr_txt} dB \u{00b7} DC centred."));
            }
        }

        // Action chips lit by the live correction state + a status foot. Full labels
        // span ~37 cols; on a narrow lab pane fall back to single-letter chips so the
        // freeze chip isn't clipped off the right edge.
        let (d_lbl, c_lbl, f_lbl) = if iw >= 37 {
            ("D DC-block", "C auto-cal", "F freeze")
        } else {
            ("D", "C", "F")
        };
        lines.push(Line::from(vec![
            Span::raw(" "),
            chip(d_lbl, cal.dc_block_on),
            Span::raw(" "),
            chip(c_lbl, cal.cal_applied),
            Span::raw(" "),
            chip(f_lbl, cal.frozen),
        ]));
        let dc_txt = if cal.dc_block_on {
            "DC-block ON"
        } else {
            "DC-block OFF"
        };
        let cal_txt = if cal.cal_applied {
            "auto-cal applied"
        } else if cal.cal_pending {
            "auto-cal capturing\u{2026}"
        } else {
            "auto-cal idle"
        };
        let mut foot = format!("{dc_txt} \u{00b7} {cal_txt}");
        if let Some(t) = cal.last_cal_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(t);
            let ago = now.saturating_sub(t);
            let ago_str = if ago < 60 {
                format!("{ago}s")
            } else {
                format!("{}m", ago / 60)
            };
            foot.push_str(&format!(" \u{00b7} last cal {ago_str} ago"));
        }
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(foot, Style::default().fg(dim)),
        ]));

        // Self-adjusting density: collapse spacers when the pane is short, grow them
        // to fill when it is tall (chrome::fit_spacers), so it breathes the same at
        // every height — consistent with the other lab side panels. The section
        // nameplates keep the grouping either way.
        crate::ui::chrome::fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_spike_typical_values() {
        // dc_mag = 0.005 → spike = 20*log10(0.005) ≈ -46 dBFS
        let s = dc_spike_dbfs(0.005).unwrap();
        assert!(
            (s - (-46.0)).abs() < 0.2,
            "expected ~-46 dBFS, got {:.1}",
            s
        );
    }

    #[test]
    fn dc_spike_zero_is_none() {
        assert!(dc_spike_dbfs(0.0).is_none());
    }
}
