// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The sortable table: columns, a sort key, and a selected row.
//!
//! Design section 9.1 calls this idiom D, and it is the one the app had no
//! answer for. Both arcs need a population keyed by address - who is here, how
//! much airtime each of them spends, how good their clock is - and a list of
//! devices with no way to order it is a list nobody can read.
//!
//! **The widget draws; it does not sort.** Which column orders the rows, which
//! way round, and which row the cursor is on are decisions with consequences
//! outside this file - they survive a redraw, they are shown in the chrome, a
//! key changes them - so they live in the state and arrive here already made.
//! What is here is the part that is only about drawing: how the columns share a
//! width, what happens when there is not enough of it, and where the viewport
//! sits.
//!
//! **A column is dropped whole or drawn whole.** The same rule the feed-health
//! notes follow, for the same reason: half a column of addresses is not a
//! narrower table, it is a table that lies about what it is showing. Columns go
//! from the right, because that is the order they were declared in and the
//! declaration puts the identifying ones first.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::ui::chrome::{selection_gutter, selection_style, SELECTION_GUTTER};

/// Which way a column's text sits in its width.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Align {
    /// Names, addresses, anything read left to right.
    Left,
    /// Numbers, so the digits line up and a column can be scanned.
    Right,
}

/// One column's shape. The contents arrive per row.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Column {
    pub title: &'static str,
    /// The narrowest this column is worth drawing at.
    pub width: usize,
    pub align: Align,
}

/// Which column orders the rows, and which way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Sort {
    pub column: usize,
    pub descending: bool,
}

impl Sort {
    /// The marker drawn beside the sorted column's title.
    pub(crate) fn marker(&self) -> &'static str {
        if self.descending {
            "\u{25be}"
        } else {
            "\u{25b4}"
        }
    }
}

/// Space between columns. One column of gap is enough to separate them and two
/// would cost a column of content at the widths this deck runs at.
const GAP: usize = 1;

/// The columns that fit in `width`, in declaration order.
///
/// A column earns its place only if the whole of it fits, gap included. Nothing
/// is truncated and nothing is squeezed: a table that shrank its address column
/// would still be showing addresses, just not ones anybody could use. The
/// selection gutter comes off the top first: every row has one.
pub(crate) fn columns_that_fit(columns: &[Column], width: usize) -> usize {
    let width = width.saturating_sub(SELECTION_GUTTER);
    let mut used = 0usize;
    for (i, c) in columns.iter().enumerate() {
        let need = if i == 0 { c.width } else { GAP + c.width };
        if used + need > width {
            return i;
        }
        used += need;
    }
    columns.len()
}

/// `columns` with column `index` grown towards `want`, from whatever `width`
/// has left once every column is laid at its narrowest.
///
/// For a column whose contents have a natural length that varies with the data
/// and the terminal, an address with its registrant's name above all: on a wide
/// screen it gets the whole name, on a narrow one it keeps its declared width
/// and its contents are cut to fit, by the panel that knows how. Never
/// narrower than declared, so [`columns_that_fit`] decides exactly what it did
/// before, and never wider than asked, so the other columns do not drift away
/// across an empty stretch.
pub(crate) fn widen(columns: &[Column], width: usize, index: usize, want: usize) -> Vec<Column> {
    let mut out = columns.to_vec();
    let used: usize = SELECTION_GUTTER
        + columns.iter().map(|c| c.width).sum::<usize>()
        + GAP * columns.len().saturating_sub(1);
    let spare = width.saturating_sub(used);
    if let Some(c) = out.get_mut(index) {
        c.width += want.saturating_sub(c.width).min(spare);
    }
    out
}

/// `columns` with each one grown to the widest of its cells in `rows`, the
/// columns in `skip` left as declared (one a panel sizes itself, as
/// [`widen`] does the address).
///
/// **A reading is never cut.** A declared width is a guess at the widest
/// value, and a live room outgrew every guess: `-10.21 ±0.26 kH`, a ppm
/// figure losing its unit. So the column takes what its readings need, and
/// when the table then no longer fits, [`columns_that_fit`] leaves whole
/// columns off the right instead, as it always has.
pub(crate) fn grow_to_contents(
    columns: &[Column],
    rows: &[Vec<String>],
    skip: &[usize],
) -> Vec<Column> {
    let mut out = columns.to_vec();
    for (i, c) in out.iter_mut().enumerate() {
        if skip.contains(&i) {
            continue;
        }
        let widest = rows
            .iter()
            .filter_map(|r| r.get(i))
            .map(|t| t.chars().count())
            .max()
            .unwrap_or(0);
        c.width = c.width.max(widest);
    }
    out
}

/// A cell laid into its column.
fn cell(text: &str, column: &Column) -> String {
    let n = text.chars().count();
    if n > column.width {
        // Unreachable through `grow_to_contents`; a panel that lays cells
        // into declared widths without it gets a cut cell, which keeps the
        // table readable while that is being got wrong.
        return text.chars().take(column.width).collect();
    }
    let pad = " ".repeat(column.width - n);
    match column.align {
        Align::Left => format!("{text}{pad}"),
        Align::Right => format!("{pad}{text}"),
    }
}

/// Where the viewport starts so that `selected` is inside it.
///
/// Scrolls by the least that works, so a cursor moving down a long list does not
/// jump the whole page under it.
pub(crate) fn viewport_start(first: usize, selected: usize, rows: usize, height: usize) -> usize {
    if height == 0 || rows == 0 {
        return 0;
    }
    let last_start = rows.saturating_sub(height);
    let mut start = first.min(last_start);
    if selected < start {
        start = selected;
    } else if selected >= start + height {
        start = selected + 1 - height;
    }
    start.min(last_start)
}

/// The header row: titles, with the sort marker on the column that orders them.
pub(crate) fn header(
    columns: &[Column],
    fit: usize,
    sort: Sort,
    theme: &crate::Theme,
) -> Line<'static> {
    let mut spans = Vec::with_capacity(fit * 2 + 1);
    // The header is never selected, but it keeps the gutter so its titles sit
    // over their columns.
    spans.push(Span::raw(" ".repeat(SELECTION_GUTTER)));
    for (i, column) in columns.iter().take(fit).enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(GAP)));
        }
        // The marker on a column that is not drawn is not moved to one that is:
        // a table that said it was ordered by a column nobody can see would be
        // worse than one that said nothing.
        let sorted = i == sort.column;
        let title = if sorted {
            format!("{}{}", column.title, sort.marker())
        } else {
            column.title.to_string()
        };
        let colour = if sorted { theme.value_hi } else { theme.label };
        spans.push(Span::styled(
            cell(&title, column),
            Style::default().fg(colour),
        ));
    }
    Line::from(spans)
}

/// One row of cells, drawn into the columns that fit.
pub(crate) fn row(
    columns: &[Column],
    fit: usize,
    cells: &[String],
    selected: bool,
    theme: &crate::Theme,
) -> Line<'static> {
    // The mark sits in a gutter every row has, so picking a row never moves the
    // columns beside it.
    let style = selection_style(selected, theme);
    let empty = String::new();
    let mut spans = Vec::with_capacity(fit * 2 + 1);
    spans.push(selection_gutter(selected, theme));
    for (i, column) in columns.iter().take(fit).enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ".repeat(GAP), style));
        }
        let text = cells.get(i).unwrap_or(&empty);
        spans.push(Span::styled(cell(text, column), style));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLUMNS: &[Column] = &[
        Column {
            title: "ADDRESS",
            width: 17,
            align: Align::Left,
        },
        Column {
            title: "SEEN",
            width: 6,
            align: Align::Right,
        },
        Column {
            title: "PKTS",
            width: 6,
            align: Align::Right,
        },
        Column {
            title: "RSSI",
            width: 8,
            align: Align::Right,
        },
    ];

    fn cells(a: &str, seen: &str, pkts: &str, rssi: &str) -> Vec<String> {
        vec![
            a.to_string(),
            seen.to_string(),
            pkts.to_string(),
            rssi.to_string(),
        ]
    }

    /// A reading wider than its column widens the column instead of losing
    /// its unit, and a column that then no longer fits drops whole.
    #[test]
    fn a_wide_reading_grows_its_column_and_is_never_cut() {
        let rows = vec![
            cells("aa:bb", "2 s", "10", "-10.21 ±0.26 kHz"),
            cells("cc:dd", "3 s", "4", "-3 dB"),
        ];
        let grown = grow_to_contents(COLUMNS, &rows, &[0]);
        assert_eq!(grown[3].width, "-10.21 ±0.26 kHz".chars().count());
        assert_eq!(grown[0].width, 17, "a skipped column keeps its width");
        assert_eq!(grown[1].width, 6, "a narrower cell never shrinks one");
        let wide = SELECTION_GUTTER + 17 + 6 + 6 + 16 + 3;
        let line = row(
            &grown,
            columns_that_fit(&grown, wide),
            &rows[0],
            false,
            &crate::Theme::sdr(),
        );
        assert!(text(&line).ends_with("-10.21 ±0.26 kHz"), "{}", text(&line));
        assert_eq!(columns_that_fit(&grown, wide - 1), 3);
    }

    fn text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    /// A column is dropped whole or drawn whole.
    #[test]
    fn a_column_that_does_not_fit_is_not_drawn_at_all() {
        // 1 (gutter) + 17 + 1 + 6 + 1 + 6 + 1 + 8 = 41 for the lot.
        assert_eq!(columns_that_fit(COLUMNS, 41), 4);
        assert_eq!(
            columns_that_fit(COLUMNS, 40),
            3,
            "the last one does not fit"
        );
        assert_eq!(columns_that_fit(COLUMNS, 32), 3);
        assert_eq!(columns_that_fit(COLUMNS, 31), 2);
        assert_eq!(columns_that_fit(COLUMNS, 18), 1);
        // Not even the first: better to draw nothing than a sliced address.
        assert_eq!(columns_that_fit(COLUMNS, 17), 0);
        assert_eq!(columns_that_fit(COLUMNS, 0), 0);
        // Extra width does not add a fifth column out of nowhere.
        assert_eq!(columns_that_fit(COLUMNS, 200), 4);
    }

    /// Numbers right, names left, and every row the same width so a column can
    /// be read down.
    #[test]
    fn the_columns_line_up_at_every_width() {
        for width in 17..60usize {
            let fit = columns_that_fit(COLUMNS, width);
            let a = row(
                COLUMNS,
                fit,
                &cells("a4:83:e7:1c:09:be", "2 s", "1204", "-41.2"),
                false,
                &crate::Theme::sdr(),
            );
            let b = row(
                COLUMNS,
                fit,
                &cells("f0:18:98:00:11:22", "41 s", "7", "-88.0"),
                false,
                &crate::Theme::sdr(),
            );
            let h = header(
                COLUMNS,
                fit,
                Sort {
                    column: 2,
                    descending: true,
                },
                &crate::Theme::sdr(),
            );
            let (ta, tb, th) = (text(&a), text(&b), text(&h));
            assert_eq!(ta.chars().count(), tb.chars().count(), "width {width}");
            assert_eq!(ta.chars().count(), th.chars().count(), "width {width}");
            assert!(ta.chars().count() <= width, "width {width}: {ta:?}");
            if fit >= 3 {
                // The two packet counts end in the same column, which is the
                // whole point of a right-aligned number.
                let end = |t: &str| t.find("1204").map(|i| i + 4);
                assert!(end(&ta).is_some(), "{ta:?}");
                assert_eq!(
                    ta.find("1204").map(|i| i + 4),
                    tb.find('7').map(|i| i + 1),
                    "width {width}\\n{ta}\\n{tb}"
                );
            }
        }
    }

    /// The sort marker is on the column that orders the table, and on no other.
    #[test]
    fn the_header_marks_the_column_that_orders_it() {
        let theme = crate::Theme::sdr();
        let down = text(&header(
            COLUMNS,
            4,
            Sort {
                column: 2,
                descending: true,
            },
            &theme,
        ));
        assert!(down.contains("PKTS\u{25be}"), "{down}");
        assert_eq!(down.matches('\u{25be}').count(), 1, "{down}");
        assert!(!down.contains('\u{25b4}'), "{down}");

        let up = text(&header(
            COLUMNS,
            4,
            Sort {
                column: 0,
                descending: false,
            },
            &theme,
        ));
        assert!(up.contains("ADDRESS\u{25b4}"), "{up}");

        // A sort on a column that is not drawn puts no marker anywhere, rather
        // than one on the wrong column.
        let hidden = text(&header(
            COLUMNS,
            2,
            Sort {
                column: 3,
                descending: true,
            },
            &theme,
        ));
        assert!(!hidden.contains('\u{25be}'), "{hidden}");
    }

    /// The cursor is visible, and it is the row it is on.
    #[test]
    fn the_selected_row_is_the_one_that_is_marked() {
        let theme = crate::Theme::sdr();
        let plain = row(
            COLUMNS,
            4,
            &cells("a4:83:e7:1c:09:be", "2 s", "12", "-41.2"),
            false,
            &theme,
        );
        let picked = row(
            COLUMNS,
            4,
            &cells("a4:83:e7:1c:09:be", "2 s", "12", "-41.2"),
            true,
            &theme,
        );
        let (tp, tk) = (text(&plain), text(&picked));
        assert_eq!(
            tp.chars().count(),
            tk.chars().count(),
            "the cursor must not move the columns"
        );
        // Columns, not bytes: the mark is one column and three bytes.
        let column = |t: &str| t.find("a4:83").map(|b| t[..b].chars().count());
        assert_eq!(
            column(&tp),
            column(&tk),
            "and the first cell starts in the same column either way"
        );
        assert!(tk.starts_with('\u{258c}'), "the gutter marks it: {tk:?}");
        assert!(tp.starts_with(' '), "and marks nothing else: {tp:?}");
        let bold = |l: &Line| {
            l.spans.iter().any(|s| {
                s.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::BOLD)
            })
        };
        assert!(bold(&picked), "the selected row reads brighter");
        assert!(!bold(&plain));
    }

    /// The header's titles sit over their columns, gutter included.
    #[test]
    fn the_header_keeps_the_gutter_so_titles_sit_over_their_columns() {
        let theme = crate::Theme::sdr();
        let h = text(&header(
            COLUMNS,
            4,
            Sort {
                column: 0,
                descending: false,
            },
            &theme,
        ));
        let r = text(&row(
            COLUMNS,
            4,
            &cells("a4:83:e7:1c:09:be", "2 s", "12", "-41.2"),
            false,
            &theme,
        ));
        assert_eq!(h.find("ADDRESS"), r.find("a4:83"), "{h:?}\n{r:?}");
    }

    /// The viewport follows the cursor, by the least that works.
    #[test]
    fn the_view_scrolls_only_as_far_as_it_has_to() {
        // Ten rows in a window of four.
        assert_eq!(viewport_start(0, 0, 10, 4), 0);
        assert_eq!(viewport_start(0, 3, 10, 4), 0, "still on screen");
        assert_eq!(viewport_start(0, 4, 10, 4), 1, "one row, not a page");
        assert_eq!(viewport_start(0, 9, 10, 4), 6, "the end of the list");
        // Back up, and it follows the other way.
        assert_eq!(viewport_start(6, 6, 10, 4), 6);
        assert_eq!(viewport_start(6, 5, 10, 4), 5);
        // A window taller than the list never scrolls.
        assert_eq!(viewport_start(0, 9, 10, 20), 0);
        assert_eq!(
            viewport_start(3, 0, 10, 20),
            0,
            "and it comes back to the top"
        );
        // Degenerate: no height, no rows.
        assert_eq!(viewport_start(0, 0, 0, 4), 0);
        assert_eq!(viewport_start(0, 5, 10, 0), 0);
    }
}
