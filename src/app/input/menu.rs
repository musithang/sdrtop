//! The menu's own keys, live only while the menu is open.
//!
//! Layer 2 of the dispatch: above panel focus, below text entry. The menu is
//! modal but it is **not** an `InputMode`: those five variants are all text being
//! typed, and this is not text.
//!
//! Modal here means modal. Nothing falls through to [`super::global`] except the
//! quit key, so `[F]` cannot start a frequency entry behind the menu and `[W]`
//! cannot pause a waterfall you are not looking at.

use crossterm::event::{KeyCode, KeyEvent};

use crate::state::MenuState;

use super::{global, metrics, InputCtx, KeyAction};

pub(super) fn handle(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    // Nothing to steer. Should not happen, since the caller only routes here
    // while the menu is open, but closing is a better answer than a panic.
    let Some(state) = metrics(ctx.state).ui.menu else {
        return KeyAction::Continue;
    };

    match key.code {
        KeyCode::Esc => close(ctx),
        KeyCode::Char('q') => return KeyAction::Quit,

        KeyCode::Tab | KeyCode::Right => move_section(ctx, state, 1),
        KeyCode::BackTab | KeyCode::Left => move_section(ctx, state, -1),
        KeyCode::Down => move_entry(ctx, state, 1),
        KeyCode::Up => move_entry(ctx, state, -1),

        // A digit opens that slot in this section, which is the same thing the
        // digit does on the deck. The menu is a picture of the number keys, so
        // the two must not be able to disagree.
        KeyCode::Char(c @ '1'..='9') => {
            let slot = c as u8 - b'0';
            let target = section_of(ctx, state).and_then(|s| {
                s.entries
                    .iter()
                    .find(|e| e.slot == Some(slot))
                    .map(|e| e.preset.clone())
            });
            if let Some(name) = target {
                return open(ctx, &name);
            }
        }
        KeyCode::Enter => {
            let target = ctx
                .engine
                .menu()
                .at(state.section, state.entry)
                .map(|e| e.preset.clone());
            if let Some(name) = target {
                return open(ctx, &name);
            }
        }
        _ => {}
    }
    KeyAction::Continue
}

/// Load a layout and close the menu.
fn open(ctx: &mut InputCtx<'_>, preset: &str) -> KeyAction {
    let action = global::presets::try_set_preset(ctx.engine, ctx.state, preset);
    close(ctx);
    action
}

fn close(ctx: &mut InputCtx<'_>) {
    metrics(ctx.state).ui.menu = None;
}

fn section_of<'a>(
    ctx: &'a InputCtx<'_>,
    state: MenuState,
) -> Option<&'a crate::ui::menu::model::Section> {
    let (si, _) = ctx.engine.menu().clamp(state.section, state.entry)?;
    ctx.engine.menu().sections.get(si)
}

/// Step to another section, pulling the entry cursor back into range: the new
/// section may be shorter than the one we came from.
fn move_section(ctx: &mut InputCtx<'_>, state: MenuState, step: isize) {
    let count = ctx.engine.menu().sections.len();
    if count == 0 {
        return;
    }
    let Some((si, _)) = ctx.engine.menu().clamp(state.section, state.entry) else {
        return;
    };
    let next = wrap(si, step, count);
    let entries = ctx.engine.menu().sections[next].entries.len();
    metrics(ctx.state).ui.menu = Some(MenuState {
        section: next,
        entry: state.entry.min(entries.saturating_sub(1)),
        ..state
    });
}

/// Step within the current section, wrapping at its ends.
fn move_entry(ctx: &mut InputCtx<'_>, state: MenuState, step: isize) {
    let Some((si, ei)) = ctx.engine.menu().clamp(state.section, state.entry) else {
        return;
    };
    let count = ctx.engine.menu().sections[si].entries.len();
    if count == 0 {
        return;
    }
    metrics(ctx.state).ui.menu = Some(MenuState {
        section: si,
        entry: wrap(ei, step, count),
        ..state
    });
}

/// `i + step` modulo `len`, for a step of -1 or 1. Pure, so the wrap-around at
/// both ends is testable without a terminal.
fn wrap(i: usize, step: isize, len: usize) -> usize {
    debug_assert!(len > 0);
    if step >= 0 {
        (i + step as usize) % len
    } else {
        (i + len - (step.unsigned_abs() % len)) % len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_moves_forward_and_back() {
        assert_eq!(wrap(0, 1, 4), 1);
        assert_eq!(wrap(3, 1, 4), 0, "forward off the end comes round");
        assert_eq!(wrap(1, -1, 4), 0);
        assert_eq!(wrap(0, -1, 4), 3, "back off the front comes round");
    }

    #[test]
    fn wrap_handles_a_single_entry() {
        assert_eq!(wrap(0, 1, 1), 0);
        assert_eq!(wrap(0, -1, 1), 0);
    }
}
