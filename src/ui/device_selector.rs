// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    backend::Backend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};

use crate::Theme;

struct DeviceSelector<'a> {
    devices: &'a [(usize, String)],
    state: ListState,
    theme: &'a Theme,
}

impl<'a> DeviceSelector<'a> {
    fn new(devices: &'a [(usize, String)], theme: &'a Theme) -> Self {
        let mut state = ListState::default();
        state.select(Some(0));
        Self {
            devices,
            state,
            theme,
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let area = f.size();

        f.render_widget(
            Block::default().style(Style::default().bg(Color::Black)),
            area,
        );

        let count = self.devices.len() as u16;
        let dialog_h = (count + 4).min(area.height.saturating_sub(4));
        let dialog_w = 66u16.min(area.width.saturating_sub(4));

        let dialog_area = {
            let vert = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(area.height.saturating_sub(dialog_h) / 2),
                    Constraint::Length(dialog_h),
                    Constraint::Min(0),
                ])
                .split(area);
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(area.width.saturating_sub(dialog_w) / 2),
                    Constraint::Length(dialog_w),
                    Constraint::Min(0),
                ])
                .split(vert[1])[1]
        };

        f.render_widget(Clear, dialog_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(dialog_area);

        let title = if self.devices.len() == 1 {
            " 1 device found — press Enter ".to_string()
        } else {
            format!(" {} devices found — select one ", self.devices.len())
        };

        let items: Vec<ListItem> = self
            .devices
            .iter()
            .map(|(_, serial)| {
                ListItem::new(format!("  {serial}")).style(Style::default().fg(self.theme.value))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .title_alignment(Alignment::Center)
                    .title_style(Style::default().fg(self.theme.value_hi))
                    .border_style(Style::default().fg(self.theme.border_accent))
                    .style(Style::default().bg(Color::Black)),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(self.theme.border_accent)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("► ");

        f.render_stateful_widget(list, chunks[0], &mut self.state);

        let hint = Paragraph::new("  ↑/↓ navigate    Enter select    q quit")
            .style(Style::default().fg(self.theme.label))
            .alignment(Alignment::Center);
        f.render_widget(hint, chunks[1]);
    }
}

/// Runs a TUI device picker. Returns the selected libhackrf device index, or `None` if quit.
pub fn run<B: Backend>(
    devices: Vec<(usize, String)>,
    theme: &Theme,
    terminal: &mut Terminal<B>,
) -> anyhow::Result<Option<usize>> {
    let mut sel = DeviceSelector::new(&devices, theme);
    loop {
        terminal.draw(|f| sel.draw(f))?;
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    let i = sel.state.selected().unwrap_or(0);
                    sel.state.select(Some(i.saturating_sub(1)));
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let i = sel.state.selected().unwrap_or(0);
                    sel.state
                        .select(Some((i + 1).min(sel.devices.len().saturating_sub(1))));
                }
                KeyCode::Enter => {
                    return Ok(sel.state.selected().map(|pos| sel.devices[pos].0));
                }
                KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                _ => {}
            }
        }
    }
}
