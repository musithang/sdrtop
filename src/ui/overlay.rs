use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::state::SdrMetrics;

pub fn render_help(f: &mut Frame, m: &SdrMetrics) {
    let area = centered_rect(62, 32, f.size());

    // Gain controls depend on the device: HackRF's LNA/VGA/AMP vs RTL-SDR's
    // single stepped tuner gain + AGC.
    let gain_section = if m.caps.gain.is_single() {
        "\
 [↑] [↓]    Tuner gain  − / +  (discrete steps)\n\
 [A]        Toggle tuner AGC"
    } else {
        "\
 [↑] [↓]    LNA gain  +8 / −8 dB  (0–40 dB)\n\
 [[] []]    VGA gain  −2 / +2 dB  (0–62 dB)\n\
 [A]        Toggle AMP"
    };

    let text = format!(
        "\
 [Q]        Quit\n\
 [SPACE]    Start / Stop RX\n\
{gain_section}\n\
 [F]        Enter frequency (MHz)\n\
 [S]        Enter sample rate ({lo:.1}–{hi:.1} MHz)\n\
 [R]        Reset all to defaults\n\
 [P]        Cycle presets\n\
 [1]        Preset: main\n\
 [2]        Preset: spectrum\n\
 [3]        Preset: waterfall\n\
 [4]        Preset: spectrum+waterfall\n\
 [5]-[9]    Lab: IQ / RF / timing / signal / sweep\n\
 [0]        Micro field-mode (press again to cycle)\n\
 [W]        Pause / resume waterfall\n\
 [E]        Focus spectrum panel (expand / zoom)\n\
   Esc      Exit spectrum focus\n\
 [I][V][T][G] Focus lab: IQ / vitals / timing / sweep\n\
 [?]        Toggle this help\n\
 [Tab]      Toggle footer bar\n\
\n\
 --theme <name>:  sdr | nord | dracula | gruvbox | catppuccin | solarized\n\
\n\
 In frequency / sample rate input mode:\n\
   digits / .    type value\n\
   Backspace     delete last char\n\
   Enter         confirm\n\
   Esc           cancel",
        gain_section = gain_section,
        lo = m.caps.sample_rate_min_hz / 1e6,
        hi = m.caps.sample_rate_max_hz / 1e6,
    );

    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .title(" Help — press [?] to close ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .alignment(Alignment::Left),
        area,
    );
}

/// The Command Rail's full-log overlay (`L` in rail-focus): a centred, themed
/// panel showing the whole scrollback. Reuses the standard `log::render` so it
/// reads identically to the docked log, just larger and floating over the deck.
pub fn render_log(f: &mut Frame, m: &SdrMetrics, theme: &crate::Theme) {
    let full = f.size();
    let w = ((full.width  as u32 * 7 / 10) as u16).max(24).min(full.width);
    let h = ((full.height as u32 * 7 / 10) as u16).max(7).min(full.height);
    let area = centered_rect(w, h, full);
    f.render_widget(Clear, area);
    crate::ui::panels::core::log::render(f, area, m, theme);
}

fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(r.height.saturating_sub(height) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(r.width.saturating_sub(width) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}
