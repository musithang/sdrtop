// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The pager view's keys: its buttons, live whenever the view is on screen,
//! as a pager's are in the hand.
//!
//! **Section-scoped and declining**, like NET's `m` and `i`
//! (`global::radio::toggle_net_mode`): on any other view every one of these
//! keys goes on to what it does there. On this one they are claimed before
//! the global table, because a pager's buttons are not a panel to focus first.
//!
//! Not `a` for "my pager", which the design first gave it: `a` is the
//! amplifier on every view, and the radio's own controls stay the radio's.

use crossterm::event::{KeyCode, KeyEvent};

use super::{metrics, InputCtx};

/// Handle `key` if the pager view is on screen and it is one of its own.
/// Returns whether it was.
pub(super) fn handle(key: KeyEvent, ctx: &mut InputCtx<'_>) -> bool {
    let mut m = metrics(ctx.state);
    if !m.ui.is_pager_section() {
        return false;
    }
    let p = &mut m.pager;
    match key.code {
        KeyCode::Left => p.step(false),
        KeyCode::Right => p.step(true),
        KeyCode::Enter => p.read_on(),
        KeyCode::Char('m') => {
            p.view.masked = !p.view.masked;
            let word = if p.view.masked { "masked" } else { "shown" };
            m.push_log(format!("Pager text {word}"));
        }
        KeyCode::Char('u') => {
            p.toggle_mine();
            let line = match p.view.mine {
                Some(ric) => format!("Pager: my pager is {ric:07}, its pages only"),
                None => "Pager: every address".to_string(),
            };
            m.push_log(line);
        }
        KeyCode::Char('b') => p.view.night = !p.view.night,
        KeyCode::Char('c') => {
            let n = p.inbox.len();
            p.clear();
            m.push_log(format!("Pager inbox cleared, {n} pages"));
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::super::{handle_key, KeyAction};
    use crate::config::LayoutConfig;
    use crate::signal::pocsag::demod::Link;
    use crate::signal::pocsag::page::{Content, Decided, End, Function, Page};
    use crate::state::SdrMetrics;
    use crate::ui::{self, PanelRegistry};
    use crossterm::event::{KeyCode, KeyEvent};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    fn page(ric: u32) -> Page {
        Page {
            ric,
            function: Function::D,
            content: Content::Tone,
            decided: Decided::NoMessage,
            end: End::Idle,
            codewords: Vec::new(),
        }
    }

    fn link() -> Link {
        Link {
            baud: 1200,
            measured_baud: 1200.0,
            clock_ppm: 0.0,
            shift_hz: 4500.0,
            offset_hz: 0.0,
            decision_snr_db: None,
            level_dbfs: -30.0,
            preamble_bits: 576,
            sync_errors: 0,
            one_is_higher: false,
            batches: 1,
        }
    }

    fn press(state: &Arc<Mutex<SdrMetrics>>, code: KeyCode) -> KeyAction {
        let mut engine =
            ui::LayoutEngine::new(LayoutConfig::default_config(), PanelRegistry::new());
        let mut footer = true;
        handle_key(
            KeyEvent::from(code),
            state,
            None,
            &mut engine,
            &mut footer,
            &HashMap::new(),
        )
    }

    fn on(section: &str) -> Arc<Mutex<SdrMetrics>> {
        let mut m = SdrMetrics::fixture();
        m.ui.section = section.to_string();
        for ric in [8, 16, 8] {
            m.pager
                .push(page(ric), link(), 439_987_500, Instant::now(), 0.0);
        }
        Arc::new(Mutex::new(m))
    }

    /// On the pager view its buttons work without focusing anything.
    #[test]
    fn the_buttons_work_on_the_view() {
        let state = on(crate::signal::pocsag::SECTION);
        press(&state, KeyCode::Left);
        assert_eq!(state.lock().unwrap().pager.current().map(|e| e.n), Some(2));
        press(&state, KeyCode::Enter);
        assert!(state.lock().unwrap().pager.view.read.contains(&2));
        press(&state, KeyCode::Char('m'));
        assert!(state.lock().unwrap().pager.view.masked);
        press(&state, KeyCode::Char('u'));
        assert_eq!(state.lock().unwrap().pager.view.mine, Some(16));
        press(&state, KeyCode::Char('b'));
        assert!(state.lock().unwrap().pager.view.night);
        press(&state, KeyCode::Char('c'));
        assert!(state.lock().unwrap().pager.inbox.is_empty());
    }

    /// Anywhere else they are not the pager's: `m` there is not masking.
    #[test]
    fn elsewhere_they_decline() {
        let state = on("lab");
        press(&state, KeyCode::Char('m'));
        press(&state, KeyCode::Char('u'));
        let m = state.lock().unwrap();
        assert!(!m.pager.view.masked);
        assert_eq!(m.pager.view.mine, None);
    }
}
