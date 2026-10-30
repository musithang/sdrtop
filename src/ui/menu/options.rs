// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The menu's right column under Options: the settings the active radio
//! exposes, then yours (`[station]`), which follow you to every radio.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{
    state::{DeviceOptionUpdate, InputMode, SdrMetrics, StationField},
    ui::chrome,
};

/// The pane as lines, for an inner width of `iw` and a height of `height`:
/// the radio's own settings, then yours (`[station]`), the selected row kept
/// in view. `selected` counts the radio's options first, then
/// [`StationField::ALL`].
fn lines(
    m: &SdrMetrics,
    selected: usize,
    iw: usize,
    height: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    if let InputMode::DeviceOptionInput { id, error } = &m.ui.input_mode {
        return editor_lines(m, id, error.as_deref(), iw, height, theme);
    }
    if let InputMode::StationInput { field, error } = &m.ui.input_mode {
        return station_editor_lines(m, *field, error.as_deref(), iw, height, theme);
    }
    if iw == 0 || height == 0 {
        return Vec::new();
    }
    let n = m.device_options.len();
    let selected = selected.min(n + StationField::ALL.len() - 1);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut at = 0;

    out.push(Line::from(""));
    out.push(heading("  Device options", iw, theme));
    if n == 0 {
        out.push(Line::from(Span::styled(
            fit_text("  This radio has nothing to set here.", iw),
            Style::default().fg(theme.border_dim),
        )));
    }
    for (index, option) in m.device_options.iter().enumerate() {
        if index == selected {
            at = out.len();
        }
        out.push(option_line(
            option,
            index == selected,
            &m.ui.device_option_update,
            iw,
            theme,
        ));
    }

    out.push(Line::from(""));
    out.push(heading("  Station", iw, theme));
    for row in chrome::wrap(
        "Where you listen from, and your call. Kept in the config, for every radio.",
        iw.saturating_sub(2),
        3,
    ) {
        out.push(Line::from(Span::styled(
            format!("  {row}"),
            Style::default().fg(theme.border_dim),
        )));
    }
    for (i, field) in StationField::ALL.into_iter().enumerate() {
        if n + i == selected {
            at = out.len();
        }
        out.push(station_line(m, field, n + i == selected, iw, theme));
        if field == StationField::Longitude {
            out.push(locator_line(m, iw, theme));
        }
    }

    // Keep the selected row in view: as much above it as fits, then below.
    let first = at
        .saturating_sub(height - 1)
        .min(out.len().saturating_sub(height));
    out.into_iter().skip(first).take(height).collect()
}

fn heading(text: &str, iw: usize, theme: &crate::Theme) -> Line<'static> {
    Line::from(Span::styled(
        fit_text(text, iw),
        Style::default()
            .fg(theme.value_hi)
            .add_modifier(Modifier::BOLD),
    ))
}

/// One `[station]` setting: its name and its value, or that it is unset.
fn station_line(
    m: &SdrMetrics,
    field: StationField,
    active: bool,
    iw: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    let marker = if active { "\u{25b8} " } else { "  " };
    if iw < 7 {
        return Line::from(Span::styled(
            fit_text(marker, iw),
            Style::default().fg(theme.border_accent),
        ));
    }
    let content = iw - 2;
    let label_width = content.min(12).min(content / 2);
    let value_width = content - label_width - 1;
    let (value, style) = match m.home.shown(field) {
        Some(v) => (
            v,
            Style::default()
                .fg(if active { theme.value_hi } else { theme.value })
                .add_modifier(if active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        None => ("not set".to_string(), Style::default().fg(theme.border_dim)),
    };
    Line::from(vec![
        Span::styled(marker, Style::default().fg(theme.border_accent)),
        Span::styled(
            fit_cell(field.label(), label_width),
            Style::default().fg(theme.label),
        ),
        Span::raw(" "),
        Span::styled(fit_text(&value, value_width), style),
    ])
}

/// The locator, worked out from the position: never typed, so not a row
/// the cursor stops on.
fn locator_line(m: &SdrMetrics, iw: usize, theme: &crate::Theme) -> Line<'static> {
    if iw < 7 {
        return Line::from("");
    }
    let content = iw - 2;
    let label_width = content.min(12).min(content / 2);
    let value_width = content - label_width - 1;
    let value = match m.home.position() {
        Some((lat, lon)) => {
            crate::geo::maidenhead::locator(lat, lon, 3).unwrap_or_default()
                + "  worked out, not typed"
        }
        None => "needs both coordinates".to_string(),
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            fit_cell("Locator", label_width),
            Style::default().fg(theme.label),
        ),
        Span::raw(" "),
        Span::styled(
            fit_text(&value, value_width),
            Style::default().fg(theme.border_dim),
        ),
    ])
}

/// Typing a `[station]` setting: the field as typed, what was wrong with
/// the last try, and what it holds now.
fn station_editor_lines(
    m: &SdrMetrics,
    field: StationField,
    error: Option<&str>,
    iw: usize,
    height: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    if iw == 0 || height == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let input = format!("{}: {}\u{258c}", field.label(), m.ui.input_buf);
    let tail: String = input.chars().rev().take(iw).collect();
    out.push(Line::from(Span::styled(
        tail.chars().rev().collect::<String>(),
        Style::default().fg(theme.value_hi),
    )));
    if let Some(error) = error {
        for row in chrome::wrap(error, iw, height.saturating_sub(out.len())) {
            out.push(Line::from(Span::styled(
                row,
                Style::default().fg(theme.status_warn),
            )));
        }
    }
    let now = m.home.shown(field).unwrap_or_else(|| "not set".into());
    let hint = match field {
        StationField::Latitude => "Degrees north, -90 to 90. Empty clears it.",
        StationField::Longitude => "Degrees east, -180 to 180. Empty clears it.",
        StationField::Callsign => "Letters, digits and /. Empty clears it.",
    };
    for text in [
        format!("Now: {now}"),
        hint.to_string(),
        "Enter keeps it, Esc leaves it.".into(),
    ] {
        for row in chrome::wrap(&text, iw, height.saturating_sub(out.len())) {
            out.push(Line::from(Span::styled(
                row,
                Style::default().fg(theme.label),
            )));
        }
    }
    out.truncate(height);
    out
}

fn editor_lines(
    m: &SdrMetrics,
    id: &str,
    error: Option<&str>,
    iw: usize,
    height: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    if iw == 0 || height == 0 {
        return Vec::new();
    }
    let option = m.device_options.iter().find(|option| option.id == id);
    let mut out = Vec::new();
    let input = format!("Value: {}\u{258c}", m.ui.input_buf);
    let tail: String = input.chars().rev().take(iw).collect();
    out.push(Line::from(Span::styled(
        tail.chars().rev().collect::<String>(),
        Style::default().fg(theme.value_hi),
    )));
    let error = error.or_else(|| {
        option
            .is_none()
            .then_some("Option is no longer available. Esc cancels.")
    });
    if let Some(error) = error {
        for row in chrome::wrap(error, iw, height.saturating_sub(out.len())) {
            out.push(Line::from(Span::styled(
                row,
                Style::default().fg(theme.status_warn),
            )));
        }
    }
    if let Some(option) = option {
        if out.len() < height {
            out.push(Line::from(Span::styled(
                fit_text(&format!("{}: {}", option.label, option.selected_choice), iw),
                Style::default().fg(theme.label),
            )));
        }
        if let Some(range) = &option.integer_range {
            for row in chrome::wrap(
                &format!("Integer {} to {}", range.start(), range.end()),
                iw,
                height.saturating_sub(out.len()),
            ) {
                out.push(Line::from(Span::styled(
                    row,
                    Style::default().fg(theme.label),
                )));
            }
        }
    }
    out
}

fn option_line(
    option: &crate::hardware::DeviceOption,
    active: bool,
    update: &DeviceOptionUpdate,
    iw: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    let marker = if active { "\u{25b8} " } else { "  " };
    let value_style = Style::default()
        .fg(if active { theme.value_hi } else { theme.value })
        .add_modifier(if active {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    if iw < 7 {
        return Line::from(Span::styled(
            fit_text(marker, iw),
            Style::default().fg(theme.border_accent),
        ));
    }
    let content_width = iw - 7;
    let label_width = content_width.min(18).min(content_width / 2);
    let choice_width = content_width - label_width;
    let label = fit_cell(&option.label, label_width);
    let shown = match update {
        DeviceOptionUpdate::Pending { request, .. } if request.id == option.id => {
            format!("{} -> {}...", option.selected_choice, request.choice)
        }
        DeviceOptionUpdate::Failed { id } if id == &option.id => {
            format!("{} (failed)", option.selected_choice)
        }
        _ => option.selected_choice.clone(),
    };
    let choice = fit_text(&shown, choice_width);
    Line::from(vec![
        Span::styled(marker, Style::default().fg(theme.border_accent)),
        Span::styled(label, Style::default().fg(theme.label)),
        Span::raw(" "),
        Span::styled(format!("\u{25c0} {choice} \u{25b6}"), value_style),
    ])
}

fn fit_cell(text: &str, width: usize) -> String {
    let mut fitted = fit_text(text, width);
    fitted.push_str(&" ".repeat(width.saturating_sub(text_width(&fitted))));
    fitted
}

fn fit_text(text: &str, width: usize) -> String {
    let mut fitted = String::new();
    for character in text.chars() {
        fitted.push(character);
        if text_width(&fitted) > width {
            fitted.pop();
            break;
        }
    }
    fitted
}

fn text_width(text: &str) -> usize {
    Line::from(text).width()
}

pub fn render(f: &mut Frame, area: Rect, m: &SdrMetrics, selected: usize, theme: &crate::Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let all = lines(
        m,
        selected,
        area.width as usize,
        area.height as usize,
        theme,
    );
    let shown: Vec<Line> = all.into_iter().take(area.height as usize).collect();
    f.render_widget(Paragraph::new(shown), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn text(m: &SdrMetrics, selected: usize, iw: usize, height: usize) -> String {
        lines(m, selected, iw, height, &crate::Theme::sdr())
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_radio_with_nothing_to_set_says_so_and_the_station_follows() {
        let t = text(&SdrMetrics::fixture(), 0, 60, 20);
        assert!(t.contains("nothing to set"), "{t}");
        assert!(t.contains("Station"), "{t}");
        assert!(t.contains("Latitude") && t.contains("not set"), "{t}");
        assert!(t.contains("needs both coordinates"), "{t}");
    }

    #[test]
    fn a_set_station_shows_its_values_and_the_locator_worked_out() {
        let mut m = SdrMetrics::fixture();
        m.home.latitude = Some(47.497);
        m.home.longitude = Some(19.04);
        m.home.callsign = Some("HA5ZZT".into());
        let t = text(&m, 2, 60, 20);
        assert!(t.contains("47.497") && t.contains("19.040"), "{t}");
        assert!(t.contains("JN97ml"), "{t}");
        assert!(
            t.contains("\u{25b8} Callsign") && t.contains("HA5ZZT"),
            "{t}"
        );
    }

    #[test]
    fn every_row_fits_the_pane() {
        let mut m = SdrMetrics::fixture();
        m.home.latitude = Some(-33.868);
        m.home.longitude = Some(151.209);
        for iw in [1, 4, 7, 12, 28, 40, 60, 100] {
            for line in lines(&m, 1, iw, 20, &crate::Theme::sdr()) {
                assert!(line.width() <= iw, "{iw}: {line:?}");
            }
        }
    }

    #[test]
    fn the_station_editor_says_what_went_wrong_and_what_is_kept() {
        let mut m = SdrMetrics::fixture();
        m.home.latitude = Some(47.497);
        m.ui.input_mode = InputMode::StationInput {
            field: StationField::Latitude,
            error: Some("Latitude runs -90 to 90".into()),
        };
        m.ui.input_buf = "95".into();
        let t = text(&m, 0, 60, 20);
        assert!(t.contains("Latitude: 95\u{258c}"), "{t}");
        assert!(t.contains("runs -90 to 90"), "{t}");
        assert!(t.contains("Now: 47.497"), "{t}");
        for iw in [1, 7, 20] {
            for line in lines(&m, 0, iw, 20, &crate::Theme::sdr()) {
                assert!(line.width() <= iw, "{line:?}");
            }
        }
    }

    #[test]
    fn options_name_their_current_choices() {
        let mut m = SdrMetrics::fixture();
        Arc::make_mut(&mut m.device_options).push(crate::hardware::DeviceOption {
            id: "bandwidth".into(),
            label: "Bandwidth".into(),
            choices: vec!["Narrow".into(), "Wide".into()],
            selected_choice: "Wide".into(),
            integer_range: None,
        });
        let text = lines(&m, 0, 60, 20, &crate::Theme::sdr())
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Bandwidth"), "{text}");
        assert!(text.contains("Wide"), "{text}");
    }

    #[test]
    fn the_selected_option_stays_in_a_short_viewport() {
        let mut m = SdrMetrics::fixture();
        for index in 0..6 {
            Arc::make_mut(&mut m.device_options).push(crate::hardware::DeviceOption {
                id: format!("option-{index}"),
                label: format!("Option {index}"),
                choices: vec!["Off".into(), "On".into()],
                selected_choice: "Off".into(),
                integer_range: None,
            });
        }

        let text = lines(&m, 5, 60, 5, &crate::Theme::sdr())
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Option 5"), "{text}");
        assert!(!text.contains("Option 0"), "{text}");
    }

    #[test]
    fn the_selected_option_uses_the_only_available_row() {
        let mut m = SdrMetrics::fixture();
        Arc::make_mut(&mut m.device_options).push(crate::hardware::DeviceOption {
            id: "bandwidth".into(),
            label: "Bandwidth".into(),
            choices: vec!["Narrow".into(), "Wide".into()],
            selected_choice: "Wide".into(),
            integer_range: None,
        });

        let text = lines(&m, 0, 60, 1, &crate::Theme::sdr())
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Bandwidth"), "{text}");
        assert!(text.contains("Wide"), "{text}");
    }

    #[test]
    fn backend_text_never_exceeds_the_pane_width() {
        let mut m = SdrMetrics::fixture();
        Arc::make_mut(&mut m.device_options).push(crate::hardware::DeviceOption {
            id: "long".into(),
            label: "A device-provided label that is much too long".into(),
            choices: vec!["A device-provided choice that is much too long".into()],
            selected_choice: "A device-provided choice that is much too long".into(),
            integer_range: None,
        });

        for width in [1, 4, 7, 8, 12, 20, 28] {
            for line in lines(&m, 0, width, 6, &crate::Theme::sdr()) {
                assert!(
                    line.width() <= width,
                    "a {}-wide row does not fit {width} columns: {line:?}",
                    line.width()
                );
            }
        }
    }

    #[test]
    fn pending_change_keeps_the_accepted_choice_visible() {
        let mut m = SdrMetrics::fixture();
        Arc::make_mut(&mut m.device_options).push(crate::hardware::DeviceOption {
            id: "bandwidth".into(),
            label: "Bandwidth".into(),
            choices: vec!["Narrow".into(), "Wide".into()],
            selected_choice: "Narrow".into(),
            integer_range: None,
        });
        m.ui.device_option_update = DeviceOptionUpdate::Pending {
            request: crate::event::DeviceOptionRequest {
                id: "bandwidth".into(),
                label: "Bandwidth".into(),
                choice: "Wide".into(),
            },
            quit_requested: false,
        };

        let text = lines(&m, 0, 80, 1, &crate::Theme::sdr())
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Narrow -> Wide..."), "{text}");
    }

    #[test]
    fn failed_change_keeps_the_accepted_choice_visible() {
        let mut m = SdrMetrics::fixture();
        Arc::make_mut(&mut m.device_options).push(crate::hardware::DeviceOption {
            id: "bandwidth".into(),
            label: "Bandwidth".into(),
            choices: vec!["Narrow".into(), "Wide".into()],
            selected_choice: "Narrow".into(),
            integer_range: None,
        });
        m.ui.device_option_update = DeviceOptionUpdate::Failed {
            id: "bandwidth".into(),
        };

        let text = lines(&m, 0, 80, 1, &crate::Theme::sdr())
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Narrow (failed)"), "{text}");
        assert!(!text.contains("Wide"), "{text}");
    }

    #[test]
    fn numeric_editor_shows_validation_errors_and_fits_narrow_panes() {
        let mut m = SdrMetrics::fixture();
        m.device_options = Arc::new(vec![crate::hardware::DeviceOption {
            id: "gain".into(),
            label: "Gain".into(),
            choices: vec!["0".into(), "12".into()],
            selected_choice: "0".into(),
            integer_range: Some(-100..=100),
        }]);
        m.ui.input_mode = InputMode::DeviceOptionInput {
            id: "gain".into(),
            error: Some("Enter an advertised integer from -100 to 100".into()),
        };
        m.ui.input_buf = "101".into();
        for width in [0, 1, 4, 7, 12, 28, 60] {
            for line in lines(&m, 0, width, 20, &crate::Theme::sdr()) {
                assert!(line.width() <= width, "{line:?}");
            }
        }
        let text = lines(&m, 0, 60, 20, &crate::Theme::sdr())
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Gain: 0"), "{text}");
        assert!(text.contains("Value: 101\u{258c}"), "{text}");
        assert!(text.contains("Enter an advertised integer"), "{text}");

        m.device_options = Arc::new(Vec::new());
        m.ui.input_mode = InputMode::DeviceOptionInput {
            id: "gain".into(),
            error: None,
        };
        let text = lines(&m, 0, 60, 20, &crate::Theme::sdr())
            .into_iter()
            .map(|line| line.to_string())
            .collect::<String>();
        assert!(
            text.contains("Option is no longer available. Esc cancels."),
            "{text}"
        );
    }

    #[test]
    fn short_numeric_panes_prioritize_input_tail_then_error() {
        let mut m = SdrMetrics::fixture();
        m.device_options = Arc::new(vec![crate::hardware::DeviceOption {
            id: "gain".into(),
            label: "Gain".into(),
            choices: vec!["0".into()],
            selected_choice: "0".into(),
            integer_range: Some(-100..=100),
        }]);
        m.ui.input_buf = "12345678901234567890".into();
        for error in [None, Some("Out of range: -100 to 100".to_string())] {
            m.ui.input_mode = InputMode::DeviceOptionInput {
                id: "gain".into(),
                error: error.clone(),
            };
            for width in [1_u16, 4, 12, 40] {
                for height in [1_u16, 2, 3] {
                    let mut terminal =
                        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
                            .unwrap();
                    terminal
                        .draw(|f| render(f, f.size(), &m, 0, &crate::Theme::sdr()))
                        .unwrap();
                    let buffer = terminal.backend().buffer();
                    let rows: Vec<String> = (0..height)
                        .map(|y| {
                            (0..width)
                                .map(|x| buffer.get(x, y).symbol())
                                .collect::<String>()
                                .trim_end()
                                .to_string()
                        })
                        .collect();
                    assert!(rows[0].ends_with('\u{258c}'), "{rows:?}");
                    if width >= 4 {
                        assert!(rows[0].ends_with("890\u{258c}"), "{rows:?}");
                    }
                    if height >= 2 {
                        let expected = if error.is_some() {
                            "Out of range:"
                        } else {
                            "Gain: 0"
                        };
                        assert_eq!(
                            rows[1],
                            fit_text(
                                if width == 40 && error.is_some() {
                                    "Out of range: -100 to 100"
                                } else {
                                    expected
                                },
                                width as usize
                            )
                            .trim_end(),
                            "{rows:?}"
                        );
                    }
                    assert_eq!(m.ui.input_buf, "12345678901234567890");
                    assert!(
                        lines(&m, 0, width as usize, height as usize, &crate::Theme::sdr()).len()
                            <= height as usize
                    );
                }
            }
        }
    }
}
