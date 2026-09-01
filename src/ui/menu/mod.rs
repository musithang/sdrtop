//! The menu: the app's launcher and its key reference.
//!
//! **Not a panel.** Full-screen UI in the family of [`crate::ui::overlay`] and
//! [`crate::ui::device_selector`], drawn outside the layout engine, into a
//! `Rect` the caller supplies: the whole screen at startup, a centred box over
//! the deck during a session. One function, two callers.
//!
//! Split by what each part draws, the way `panels/core/spectrum/` is:
//!
//! - [`model`]: presets to sections. The only part with logic, and the only part
//!   that never touches ratatui.
//! - [`sections`]: the left column.
//! - [`entries`]: the right column, a section's layouts.
//! - [`keys`]: the right column, the key reference.
//! - [`options`]: the right column, settings. Empty for now, and honest about it.
//!
//! [`render`] is the orchestrator. It resolves the frame, carves the rows and
//! columns, and calls each part once. **The parts do not call each other.**

pub mod entries;
pub mod keys;
pub mod model;
pub mod options;
pub mod sections;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::{MenuPane, MenuState, SdrMetrics};
use crate::ui::chrome;

use model::Menu;

/// Below this many columns the two columns stop fitting side by side, so the
/// left one folds away and the current section's title becomes the heading of
/// the single remaining column. The number keys are unaffected: they never
/// depended on the section list being visible.
const TWO_COLUMN_MIN: u16 = 50;

/// Width of the section column: "Command Rail" plus the cursor gutter and the
/// rule.
const LEFT_WIDTH: u16 = 16;

pub fn render(
    f: &mut Frame,
    area: Rect,
    m: &SdrMetrics,
    menu: &Menu,
    state: &MenuState,
    theme: &crate::Theme,
) {
    let block = chrome::deck_block(theme.border_accent).title(Line::from(chrome::nameplate(
        vec![Span::styled(
            format!(" sdrtop {} ", crate::cli::VERSION),
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        )],
        theme.border_accent,
    )));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Header, body, footer. The body wins when the terminal is too short for all
    // three: a menu with no list is not a menu.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    header(f, rows[0], m, theme);
    footer(f, rows[2], theme);

    // The cursor is cloned into the frame snapshot and arrives here without the
    // engine, so it is clamped rather than trusted. An out of range index would
    // panic during draw and take the terminal with it.
    let Some((si, ei)) = menu.clamp(state.section, state.entry) else {
        f.render_widget(
            Paragraph::new(Span::styled(
                "  no layouts are defined",
                Style::default().fg(theme.status_warn),
            )),
            rows[1],
        );
        return;
    };

    if rows[1].width >= TWO_COLUMN_MIN {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(LEFT_WIDTH), Constraint::Min(1)])
            .split(rows[1]);
        let row = sections::selected_row(menu, si, state.pane);
        sections::render(f, cols[0], menu, row, theme);
        right_pane(f, cols[1], m, menu, si, ei, state, false, theme);
    } else {
        right_pane(f, rows[1], m, menu, si, ei, state, true, theme);
    }
}

/// The one place that decides which pane the right column is showing. A new pane
/// is a variant here and a module beside `entries`, not a rewrite.
///
/// `folded` says whether the section list is off screen. It is passed in rather
/// than re-derived from `area.width`, because by the time this runs `area` is
/// only the *remaining* column: measuring that against the two-column threshold
/// put the folded form's heading on screen while the section list was still
/// beside it, at every terminal between roughly 66 and 82 columns.
#[allow(clippy::too_many_arguments)]
fn right_pane(
    f: &mut Frame,
    area: Rect,
    m: &SdrMetrics,
    menu: &Menu,
    section: usize,
    cursor: usize,
    state: &MenuState,
    folded: bool,
    theme: &crate::Theme,
) {
    // In the folded single-column form the pane names itself, because the list
    // that would otherwise name it is not on screen.
    let (heading, body) = if folded {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);
        (Some(split[0]), split[1])
    } else {
        (None, area)
    };
    if let Some(heading) = heading {
        let title = match state.pane {
            MenuPane::Views => menu.sections[section].title.as_str(),
            MenuPane::Keys => "Keys",
            MenuPane::Options => "Options",
        };
        f.render_widget(
            Paragraph::new(chrome::section(title, "", heading.width as usize, theme)),
            heading,
        );
    }

    match state.pane {
        MenuPane::Views => entries::render(f, body, &menu.sections[section], cursor, theme),
        MenuPane::Keys => keys::render(f, body, &m.caps.gain, state.scroll, theme),
        MenuPane::Options => options::render(f, body, theme),
    }
}

/// Who you are and where the radio is pointing, so the menu is not a screen that
/// hides the one number you were watching.
fn header(f: &mut Frame, area: Rect, m: &SdrMetrics, theme: &crate::Theme) {
    let line = Line::from(vec![
        Span::styled(
            format!(" {}", m.system.board_name),
            Style::default().fg(theme.value),
        ),
        Span::styled("   ", Style::default()),
        Span::styled(
            format!("{:.3} MHz", m.radio.frequency as f64 / 1e6),
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// The keys, in the order the design's "Moving around" table lists them.
fn footer(f: &mut Frame, area: Rect, theme: &crate::Theme) {
    let key = Style::default().fg(theme.border_accent);
    let what = Style::default().fg(theme.label);
    let mut spans = Vec::new();
    for (k, w) in [
        ("Tab", "section"),
        ("\u{2191}\u{2193}", "move"),
        ("1-9", "open"),
        ("Enter", "open"),
        ("Esc", "close"),
    ] {
        spans.push(Span::styled(format!(" {k} "), key));
        spans.push(Span::styled(w, what));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LayoutConfig;
    use ratatui::{backend::TestBackend, Terminal};

    /// Render the menu at a fixed size and hand back the buffer as lines.
    ///
    /// The same idea as `state::fixture::draw`, but without the panel registry:
    /// the menu is not a panel, so it cannot go through
    /// `PanelRegistry::render_panel` and needs its own harness.
    fn draw(w: u16, h: u16, state: &MenuState) -> Vec<String> {
        let menu = model::build(&LayoutConfig::default_config().presets);
        let metrics = SdrMetrics::fixture();
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| render(f, f.size(), &metrics, &menu, state, &theme))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf.get(x, y).symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn at(section: usize, entry: usize) -> MenuState {
        MenuState {
            section,
            entry,
            pane: MenuPane::Views,
            scroll: 0,
        }
    }

    #[test]
    fn both_columns_are_drawn() {
        let all = draw(80, 24, &at(1, 0)).join("\n");
        for wanted in ["Command Rail", "Lab", "Sweep", "Micro", "IQ", "Timing"] {
            assert!(all.contains(wanted), "'{wanted}' missing from:\n{all}");
        }
    }

    /// The number beside a view is its slot, which is the key that selects it.
    /// If these ever disagree the menu is teaching a key that does not work.
    ///
    /// Matched as the rendered pair `"2  RF"` rather than "some line mentioning
    /// RF": the header says `HackRF One`, which contains `RF` and satisfied a
    /// looser version of this test for the wrong reason.
    #[test]
    fn the_numbers_shown_are_the_keys_that_work() {
        let all = draw(80, 24, &at(1, 0));
        let joined = all.join("\n");
        for (slot, title) in [(1, "IQ"), (2, "RF"), (3, "Timing"), (4, "Signal")] {
            let pair = format!("{slot}  {title}");
            assert!(
                all.iter().any(|l| l.contains(&pair)),
                "expected '{pair}' on a row, in:\n{joined}"
            );
        }
    }

    /// The header keeps the tuned frequency on screen, so opening the menu does
    /// not hide the one number you were watching.
    #[test]
    fn the_header_shows_the_device_and_the_frequency() {
        let all = draw(80, 24, &at(0, 0)).join("\n");
        assert!(all.contains("100.000 MHz"), "{all}");
    }

    /// A narrow terminal folds to one column instead of spilling, and names the
    /// section it is showing, since the list that would name it is gone.
    #[test]
    fn a_narrow_terminal_folds_to_one_column() {
        let all = draw(44, 16, &at(1, 0));
        let joined = all.join("\n");
        assert!(all.iter().all(|l| l.chars().count() <= 44), "{joined}");
        assert!(joined.contains("IQ"), "the views must survive:\n{joined}");
        assert!(
            joined.contains("LAB"),
            "the folded form must name its section:\n{joined}"
        );
    }

    /// The folded heading belongs to the folded form only.
    ///
    /// Found by running the real binary at 70 columns, not by this suite: the
    /// narrow test above folds properly and the wide one never folds, so the
    /// middle was the gap. There the menu still had both columns while the right
    /// one, being only the remainder, measured below the threshold and drew the
    /// heading anyway. The widths here bracket that gap on both sides.
    #[test]
    fn the_section_heading_appears_only_when_the_list_is_gone() {
        for w in [56, 60, 70, 80, 120] {
            let all = draw(w, 20, &at(0, 0)).join("\n");
            assert!(
                all.contains("Command Rail"),
                "the section list must be on screen at {w}:\n{all}"
            );
            assert!(
                !all.contains("COMMAND RAIL"),
                "the folded heading must not be drawn beside the list at {w}:\n{all}"
            );
        }
        // And the folded form does still name its section.
        let folded = draw(44, 16, &at(0, 0)).join("\n");
        assert!(!folded.contains("Command Rail"), "{folded}");
        assert!(folded.contains("COMMAND RAIL"), "{folded}");
    }

    /// The Keys pane is a row in the left column and the content of the right
    /// one, and it replaces an overlay that had drifted out of step with the
    /// dispatch. `keys.rs` owns the check that it cannot drift again.
    #[test]
    fn the_keys_pane_lists_the_global_keys() {
        let state = MenuState {
            section: 0,
            entry: 0,
            pane: MenuPane::Keys,
            scroll: 0,
        };
        let all = draw(90, 30, &state).join("\n");
        assert!(
            all.contains("Keys"),
            "the left column needs the row:\n{all}"
        );
        assert!(all.contains("start or stop RX"), "{all}");
        assert!(all.contains("type a frequency"), "{all}");
        // The section list is still there: the pane replaces the right column,
        // not the whole menu.
        assert!(all.contains("Command Rail"), "{all}");
    }

    /// The reference is taller than a short terminal, so it scrolls, and the
    /// scroll actually moves the content rather than being stored and ignored.
    #[test]
    fn the_keys_pane_scrolls() {
        let top = draw(
            90,
            18,
            &MenuState {
                section: 0,
                entry: 0,
                pane: MenuPane::Keys,
                scroll: 0,
            },
        )
        .join("\n");
        let down = draw(
            90,
            18,
            &MenuState {
                section: 0,
                entry: 0,
                pane: MenuPane::Keys,
                scroll: 8,
            },
        )
        .join("\n");
        assert!(top.contains("start or stop RX"), "{top}");
        assert!(!down.contains("start or stop RX"), "scrolled away:\n{down}");
        assert_ne!(top, down);
    }

    /// The Options pane is empty by design, so what is being pinned is that the
    /// emptiness is stated on screen. A pane that opens to blank space reads as
    /// a bug; one that says why it is blank reads as a decision.
    #[test]
    fn the_options_pane_admits_it_is_empty() {
        let state = MenuState {
            section: 0,
            entry: 0,
            pane: MenuPane::Options,
            scroll: 0,
        };
        let all = draw(90, 30, &state).join("\n");
        assert!(
            all.contains("Options"),
            "the left column needs the row:\n{all}"
        );
        assert!(all.contains("Nothing to configure yet"), "{all}");
        // And the pane replaces the right column only, the same as Keys.
        assert!(all.contains("Command Rail"), "{all}");
    }

    /// Small enough that nothing sensible fits. The requirement is only that it
    /// does not panic and does not draw outside its area.
    #[test]
    fn it_survives_a_tiny_terminal() {
        for (w, h) in [(40, 10), (20, 5), (8, 3), (4, 2)] {
            let lines = draw(w, h, &at(3, 3));
            assert_eq!(lines.len(), h as usize);
            assert!(lines.iter().all(|l| l.chars().count() <= w as usize));
        }
    }

    /// An out of range cursor is clamped, not indexed. `MenuState` is cloned into
    /// the frame snapshot and arrives here without the engine that built the
    /// table, so this is the path that must not panic.
    #[test]
    fn an_out_of_range_cursor_does_not_panic() {
        let all = draw(80, 24, &at(99, 99)).join("\n");
        assert!(all.contains("Micro"), "clamps to the last section:\n{all}");
    }
}
