use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::palette::{magnitude_to_color_palette, ColorDepth};
use crate::state::SdrMetrics;
use crate::ui::widgets::band_plan::BAND_PLAN;
use crate::ui::chrome;
use crate::ui::panel::{Bond, Panel};
use crate::ui::panels::core::spectrum::{fmt_spectrum_step, freq_scale_spans};

// ── Waterfall row-stride steps ────────────────────────────────────────────────

pub const WF_STRIDES: &[usize] = &[1, 2, 4, 8, 16, 32, 64];

pub fn prev_wf_stride(current: usize) -> usize {
    WF_STRIDES.iter().rev().find(|&&s| s < current).copied().unwrap_or(1)
}

pub fn next_wf_stride(current: usize) -> usize {
    WF_STRIDES.iter().find(|&&s| s > current).copied().unwrap_or(64)
}

// ── Waterfall frequency zoom levels ──────────────────────────────────────────

pub const WF_ZOOM_LEVELS: &[u32] = &[1, 2, 4, 8, 16, 32];

pub fn prev_wf_zoom(current: u32) -> u32 {
    WF_ZOOM_LEVELS.iter().rev().find(|&&z| z < current).copied().unwrap_or(1)
}

pub fn next_wf_zoom(current: u32) -> u32 {
    WF_ZOOM_LEVELS.iter().find(|&&z| z > current).copied().unwrap_or(32)
}

const DB_MAX: f32 = 0.0;

pub struct WaterfallPanel;

impl WaterfallPanel {
    pub fn new() -> Self { Self }
}

impl Panel for WaterfallPanel {
    fn name(&self) -> &'static str { "waterfall" }
    fn min_size(&self) -> (u16, u16) { (40, 5) }
    fn focus_key(&self) -> Option<char> { Some('l') }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("↑ ↓", "Zoom colour scale"),
            ("+ -",  "Frequency zoom"),
            ("J K",  "Scroll history"),
            ("[ ]",  "Row stride (speed)"),
            ("M",    "Place/remove cursor"),
            ("← →",  "Move cursor"),
            ("W",    "Pause / resume"),
            ("P",    "Colour palette"),
        ]
    }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        render(f, area, state, theme, focused, Bond::None);
    }
}

/// Max dB over the bin range `[start, end)` of one waterfall row, clamped to the
/// row's own length. Rows are normally all the (fixed) FFT bin count, but reading
/// each row against its own length means a row that ever differs — e.g. if the FFT
/// size changed at runtime — degrades to a partial read instead of slicing out of
/// bounds and panicking. An empty or fully out-of-range span reads −∞ (the floor).
fn band_max(row: &[f32], start: usize, end: usize) -> f32 {
    let s = start.min(row.len());
    let e = end.min(row.len()).max(s);
    if s == e { f32::NEG_INFINITY } else { row[s..e].iter().copied().fold(f32::NEG_INFINITY, f32::max) }
}

/// Free render entry point so the layout engine can bond the waterfall to the
/// spectrum above it. `Bond::Above` forces `zoom = 1` (the shared ruler shows the
/// spectrum's full span), drops the nameplate (its identity + live tags move to
/// the ruler's end-cap tabs), and overlays the shared frequency ruler on the top
/// border so the two panels read as one instrument.
pub fn render(f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool, bond: Bond) {
        let bonded  = bond == Bond::Above;
        let buf = &state.waterfall.buffer;
        let db_min  = state.waterfall.db_min;
        let scroll  = state.waterfall.scroll_offset;
        let stride  = buf.row_stride;
        // `hz_zoom` is the shared frequency zoom: in the bonded view the spectrum
        // above narrows to the same centre slice, so `+`/`-` zoom both plots at
        // once around the tuned frequency.
        let zoom    = state.waterfall.hz_zoom;

        let no_data = state.waterfall.last_fft.is_none();
        let stale = state.waterfall.last_fft.as_ref()
            .map(|fr| fr.timestamp.elapsed() > std::time::Duration::from_millis(500))
            .unwrap_or(false);
        let border_color = if focused { theme.border_focused }
            else if buf.paused || stale || no_data { theme.stale }
            else { theme.border_accent };

        // Nameplate: WATERFALL with the second 'L' focus key highlighted, plus tags.
        // In bonded mode the nameplate is suppressed; the live tags move to the
        // shared ruler's tabs (left `[L]`, right status), so the top border stays
        // a clean frequency rule.
        let key_style  = Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD);
        let name_style = Style::default().fg(theme.label).add_modifier(Modifier::BOLD);
        let mut title_spans = chrome::nameplate(vec![
            Span::styled("WATERFA", name_style),
            Span::styled("L", key_style),
            Span::styled("L", name_style),
        ], border_color);
        if buf.paused {
            title_spans.push(Span::styled(" [PAUSED]", Style::default().fg(theme.status_warn)));
        } else if stale {
            title_spans.push(Span::styled(" [STALE]", Style::default().fg(theme.stale)));
        }
        if stride > 1 {
            title_spans.push(Span::styled(
                format!(" [×{}]", stride),
                Style::default().fg(theme.label),
            ));
        }
        if zoom > 1 {
            title_spans.push(Span::styled(
                format!(" [Z×{}]", zoom),
                Style::default().fg(theme.value_hi),
            ));
        }
        // Clamp scroll display to actual max: content ≈ area minus borders(2) and indicator(1)
        let approx_content_h = area.height.saturating_sub(3) as usize;
        let max_scroll_display = buf.rows.len().saturating_sub(approx_content_h * 2) / 2;
        let display_scroll = scroll.min(max_scroll_display);
        if display_scroll > 0 {
            title_spans.push(Span::styled(
                format!(" [↑{}]", display_scroll),
                Style::default().fg(theme.value_hi),
            ));
        }
        let title_line = if bonded { Line::from("") } else { Line::from(title_spans) };

        if buf.rows.is_empty() {
            f.render_widget(
                Paragraph::new("Waiting for RX\u{2026}")
                    .block(chrome::deck_block(border_color).title(title_line))
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(theme.label)),
                area,
            );
            chrome::corner_accents(f, area, border_color);
            return;
        }

        let block = chrome::deck_block(border_color).title(title_line);
        let inner = block.inner(area);
        f.render_widget(block, area);
        chrome::corner_accents(f, area, border_color);

        // When focused, reserve one row for the indicator line (like spectrum panel)
        let (content_area, indicator_area) = if focused && inner.height > 2 {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(inner);
            (split[0], Some(split[1]))
        } else {
            (inner, None)
        };

        // Offset matches spectrum panel's dB-label column so both panels share the same x-axis
        const DB_COL: u16 = 6;
        let wf_area = Rect {
            x: content_area.x + DB_COL,
            y: content_area.y,
            width: content_area.width.saturating_sub(DB_COL),
            height: content_area.height,
        };
        let cols = wf_area.width as usize;
        if cols == 0 { return; }

        // Frequency bounds — narrowed by zoom factor (centred on tuned frequency)
        let (left_hz, bw) = state.waterfall.last_fft.as_ref()
            .map(|fr| {
                let visible_bw = fr.sample_rate / zoom as f64;
                (fr.center_freq_hz as f64 - visible_bw / 2.0, visible_bw)
            })
            .unwrap_or((0.0, 1.0));

        // ── Bonded shared ruler ────────────────────────────────────────────
        // The top border of the waterfall doubles as the spectrum+waterfall
        // shared frequency rule: a `┬`-ticked MHz scale aligned to the plot
        // (`wf_area`), with `─` fill so it reads as a continuous engraved rule.
        // The `WATERFALL` nameplate rides the rule's left cap (the `L` focus key
        // highlighted), and live tags (`⏸ ×N ↑N`) ride a right cap on the bottom
        // border — so the panel keeps its identity without a second frame.
        if bonded {
            let ruler = freq_scale_spans(left_hz, bw, wf_area.width as usize,
                                         border_color, theme.value, '\u{2500}'); // ─
            f.render_widget(
                Paragraph::new(Line::from(ruler)),
                Rect { x: wf_area.x, y: area.y, width: wf_area.width, height: 1 },
            );
            // Left cap: `╴WATERFALL╶` nameplate (the second-`L` focus key lit),
            // overlaid on the rule's left so the panel name stays where the eye
            // expects it; the rule's ticks stay aligned to the plot beneath.
            let name = chrome::nameplate(vec![
                Span::styled("WATERFA", name_style),
                Span::styled("L", key_style),
                Span::styled("L", name_style),
            ], border_color);
            let name_w: u16 = name.iter().map(|s| s.width() as u16).sum::<u16>()
                .min(area.width.saturating_sub(2));
            f.render_widget(
                Paragraph::new(Line::from(name)),
                Rect { x: area.x + 1, y: area.y, width: name_w, height: 1 },
            );
            // Right cap on the BOTTOM border: live status the nameplate used
            // to carry (paused / row-stride / scroll-back), right-aligned.
            let mut tags: Vec<Span> = Vec::new();
            if buf.paused { tags.push(Span::styled("\u{23F8} ", Style::default().fg(theme.status_warn))); }
            else if stale { tags.push(Span::styled("STALE ", Style::default().fg(theme.stale))); }
            if stride > 1 { tags.push(Span::styled(format!("\u{00D7}{} ", stride), Style::default().fg(theme.label))); }
            if display_scroll > 0 { tags.push(Span::styled(format!("\u{2191}{} ", display_scroll), Style::default().fg(theme.value_hi))); }
            if !tags.is_empty() && area.height >= 2 {
                let mut status = vec![Span::styled("\u{2574}", Style::default().fg(border_color))];
                status.extend(tags);
                status.push(Span::styled("\u{2576}", Style::default().fg(border_color)));
                let w: u16 = status.iter().map(|s| s.width() as u16).sum();
                let x = (area.x + area.width).saturating_sub(2 + w);
                f.render_widget(
                    Paragraph::new(Line::from(status)),
                    Rect { x, y: area.y + area.height - 1, width: w, height: 1 },
                );
            }
        }

        // Cursor column in display space
        let cursor_col: Option<usize> = state.waterfall.cursor_freq.and_then(|cf| {
            let frac = (cf as f64 - left_hz) / bw;
            if (0.0..=1.0).contains(&frac) {
                Some(((frac * cols as f64) as usize).min(cols - 1))
            } else {
                None
            }
        });

        let rows_to_show  = wf_area.height as usize;
        // Half-block rendering: each character cell encodes two time-rows (▀ fg=top bg=bottom)
        let data_rows     = rows_to_show * 2;
        let max_scroll    = buf.rows.len().saturating_sub(data_rows) / 2;
        let skip_chars    = scroll.min(max_scroll);
        let skip_data     = skip_chars * 2;

        let depth = ColorDepth::detect();
        let wp = state.waterfall.palette;
        let floor_color = magnitude_to_color_palette(f32::NEG_INFINITY, db_min, DB_MAX, depth, theme, wp);
        let mut lines: Vec<Line> = Vec::with_capacity(rows_to_show);

        // Zoom: show only the centre slice of bins
        let zoom_i      = zoom as usize;
        let row_bins    = buf.rows.front().map(|(_, r)| r.len()).unwrap_or(1);
        let visible_n   = (row_bins / zoom_i).max(1);
        let lo_bin      = (row_bins / 2).saturating_sub(visible_n / 2);

        let mut data_iter = buf.rows.iter().skip(skip_data).take(data_rows);
        loop {
            let Some((_ts, top_row)) = data_iter.next() else { break };
            let bot_row = data_iter.next().map(|(_ts, r)| r.as_ref());
            let mut spans: Vec<Span> = Vec::with_capacity(cols);
            for col in 0..cols {
                let bin_start = (lo_bin + col * visible_n / cols).min(row_bins - 1);
                let bin_end   = (lo_bin + ((col + 1) * visible_n) / cols).max(bin_start + 1).min(row_bins);
                // Read each row against its own length (band_max clamps) so a row
                // that ever differs from the newest row's bin count can't slice OOB.
                let top_db    = band_max(top_row, bin_start, bin_end);
                let top_color = magnitude_to_color_palette(top_db, db_min, DB_MAX, depth, theme, wp);
                let bot_color = bot_row.map(|r| {
                    let db = band_max(r, bin_start, bin_end);
                    magnitude_to_color_palette(db, db_min, DB_MAX, depth, theme, wp)
                }).unwrap_or(floor_color);
                if Some(col) == cursor_col {
                    spans.push(Span::styled("▀", Style::default().fg(theme.value_hi).bg(bot_color)));
                } else {
                    spans.push(Span::styled("▀", Style::default().fg(top_color).bg(bot_color)));
                }
            }
            lines.push(Line::from(spans));
        }

        f.render_widget(Paragraph::new(lines), wf_area);

        // ── Band plan overlay (dim labels on the top row of the waterfall) ─
        // Skipped when bonded — the spectrum above already carries the band plan,
        // so showing it twice would be redundant.
        if !bonded && wf_area.height >= 2 && wf_area.width > 4 {
            let cw = wf_area.width as f64;
            let right_hz = left_hz + bw;
            let mut next_free_col: i32 = -1;
            for &(band_s, band_e, label) in BAND_PLAN {
                let bs = band_s as f64;
                let be = band_e as f64;
                if bs >= right_hz || be <= left_hz { continue; }
                let vis_s  = bs.max(left_hz);
                let vis_e  = be.min(right_hz);
                let center = (vis_s + vis_e) / 2.0;
                let frac   = (center - left_hz) / bw;
                let col    = (frac * cw) as u16;
                let lw     = label.len() as u16;
                let col    = col.min(wf_area.width.saturating_sub(lw));
                if (col as i32) < next_free_col { continue; }
                next_free_col = col as i32 + lw as i32 + 1;
                // Reset bg so the band names sit on the panel's black instead of
                // letting the colourful waterfall cells bleed through the glyphs.
                f.render_widget(
                    Paragraph::new(Span::styled(
                        label,
                        Style::default().fg(theme.label).bg(Color::Reset),
                    )),
                    Rect { x: wf_area.x + col, y: wf_area.y, width: lw, height: 1 },
                );
            }
        }

        // ── Time axis — elapsed-time ticks down the right edge ─────────────
        // The newest row is at the top (≈now); each tick reads the real timestamp
        // of the row at that screen line, so you can see how far back the visible
        // history reaches. Kept on the right edge so the left dB legend and the
        // shared-with-spectrum frequency span are untouched.
        if wf_area.height >= 6 && wf_area.width > 12 {
            let h = wf_area.height as usize;
            for k in 1..=4 {
                let r = (h - 1) * k / 4;            // screen row, skipping the top "now" row
                let data_idx = skip_data + r * 2;  // top sub-row of that character cell
                if let Some((ts, _)) = buf.rows.get(data_idx) {
                    let label = format!("\u{2574}{}s", ts.elapsed().as_secs()); // ╴12s
                    let lw = label.chars().count() as u16;
                    // Explicit Reset bg: a default Style leaves bg = None, so the
                    // colourful waterfall cells show through and the text blends in.
                    // Resetting to the panel's black makes it an engraved tab.
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            label,
                            Style::default().fg(theme.value_hi).bg(Color::Reset).add_modifier(Modifier::BOLD),
                        )),
                        Rect { x: wf_area.x + wf_area.width - lw, y: wf_area.y + r as u16, width: lw, height: 1 },
                    );
                }
            }
        }

        // dBFS colour scale legend — tracks dynamic db_min
        let legend_area = Rect {
            x: content_area.x,
            y: content_area.y,
            width: DB_COL,
            height: content_area.height,
        };
        let h = legend_area.height as usize;
        if h > 0 {
            let steps = (h * 2).max(2);
            let mut legend: Vec<Line> = Vec::with_capacity(h);
            for row in 0..h {
                let t_top = (row * 2) as f32 / (steps - 1) as f32;
                let t_bot = (row * 2 + 1) as f32 / (steps - 1) as f32;
                let top_color = magnitude_to_color_palette(DB_MAX + (db_min - DB_MAX) * t_top, db_min, DB_MAX, depth, theme, wp);
                let bot_color = magnitude_to_color_palette(DB_MAX + (db_min - DB_MAX) * t_bot, db_min, DB_MAX, depth, theme, wp);
                let label = match row {
                    0 => format!("{:>+4} ", DB_MAX as i32),
                    r if r == h.saturating_sub(1) => format!("{:>4} ", db_min as i32),
                    r if r == h / 2 => format!("{:>4} ", ((DB_MAX + db_min) / 2.0) as i32),
                    _ => "     ".to_string(),
                };
                legend.push(Line::from(vec![
                    Span::styled("▀", Style::default().fg(top_color).bg(bot_color)),
                    Span::styled(label, Style::default().fg(theme.value)),
                ]));
            }
            f.render_widget(Paragraph::new(legend), legend_area);
        }

        // ── Indicator row (focus only) ─────────────────────────────────
        if let Some(ind_area) = indicator_area {
            let right_info: String = if let Some(cf) = state.waterfall.cursor_freq {
                // Cursor active: show freq, dBFS at top visible row, time ago
                let freq_mhz = cf as f64 / 1_000_000.0;
                let (db_at_cursor, secs_ago) = buf.rows.iter().skip(skip_data).next()
                    .and_then(|(ts, row)| {
                        let frac = (cf as f64 - left_hz) / bw;
                        if !(0.0..=1.0).contains(&frac) { return None; }
                        let col = ((frac * cols as f64) as usize).min(cols - 1);
                        let n = row.len();
                        let lo = col * n / cols;
                        let hi = ((col + 1) * n / cols).max(lo + 1).min(n);
                        let db = row[lo..hi].iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                        Some((db, ts.elapsed().as_secs()))
                    })
                    .unwrap_or((f32::NEG_INFINITY, 0));
                if db_at_cursor.is_finite() {
                    format!("  cur: {:.3} MHz  {:.1} dBFS  {}s ago  ← →  M", freq_mhz, db_at_cursor, secs_ago)
                } else {
                    format!("  cur: {:.3} MHz  ← →  M", freq_mhz)
                }
            } else {
                let step_str = fmt_spectrum_step(state.spectrum.step_hz);
                format!("  ×{}  frames/row  [ ]  M cursor  step {}  ↑↓ zoom  J/K scroll", stride, step_str)
            };

            let dashes = (ind_area.width as usize).saturating_sub(right_info.len());
            let line = Line::from(vec![
                Span::styled("─".repeat(dashes), Style::default().fg(theme.border_dim)),
                Span::styled(right_info, Style::default().fg(theme.label)),
            ]);
            f.render_widget(Paragraph::new(line), ind_area);
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_max_reads_in_range() {
        let row = [-90.0, -50.0, -70.0, -60.0];
        assert_eq!(band_max(&row, 0, 4), -50.0, "max over the whole row");
        assert_eq!(band_max(&row, 2, 4), -60.0, "max over a sub-range");
    }

    #[test]
    fn band_max_clamps_out_of_range_indices() {
        let row = [-90.0, -50.0, -70.0];
        // End past the row length must clamp, not panic.
        assert_eq!(band_max(&row, 1, 99), -50.0);
        // A start at/after the row length yields the floor (empty span).
        assert_eq!(band_max(&row, 3, 9), f32::NEG_INFINITY);
        assert_eq!(band_max(&row, 5, 2), f32::NEG_INFINITY);
    }

    #[test]
    fn band_max_empty_row_is_floor() {
        assert_eq!(band_max(&[], 0, 4), f32::NEG_INFINITY);
    }
}
