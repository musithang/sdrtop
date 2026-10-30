// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The FT8 view's keys, live whenever the view is on screen.
//!
//! **Section-scoped and declining**, as the pager's are: on any other view
//! every one of these keys goes on to what it does there. Here `↑` `↓` choose
//! a station rather than step the gain (the gain stays on `,` `.` `[` `]`),
//! and `←` `→` step between FT8's bands: the one key on this view that
//! tunes, and only to a band the radio reaches. Opening the view retunes
//! nothing.

use crossterm::event::{KeyCode, KeyEvent};

use super::{metrics, InputCtx};
use crate::signal::ft8::bands;

/// Handle `key` if the FT8 view is on screen and it is one of its own.
/// Returns whether it was.
pub(super) fn handle(key: KeyEvent, ctx: &mut InputCtx<'_>) -> bool {
    let mut m = metrics(ctx.state);
    if !m.ui.is_ft8_section() {
        return false;
    }
    match key.code {
        KeyCode::Up => m.ft8.select_step(false),
        KeyCode::Down => m.ft8.select_step(true),
        KeyCode::Char('z') => {
            m.ft8.view.range = m.ft8.view.range.next();
            let label = m.ft8.view.range.label();
            m.push_log(format!("FT8 map: {label}"));
        }
        KeyCode::Char('u') => {
            if m.home.callsign.is_none() && !m.ft8.view.only_yours {
                m.push_log(
                    "FT8: no call of yours to look for; set it under Options \u{203a} Station"
                        .to_string(),
                );
            } else {
                m.ft8.view.only_yours = !m.ft8.view.only_yours;
                let word = if m.ft8.view.only_yours {
                    "only yours"
                } else {
                    "every decode"
                };
                m.push_log(format!("FT8 list: {word}"));
            }
        }
        KeyCode::Char('c') => {
            let n = m.ft8.decodes.len();
            m.ft8.clear();
            m.push_log(format!("FT8 cleared, {n} decodes"));
        }
        KeyCode::Left | KeyCode::Right => {
            let up = key.code == KeyCode::Right;
            let from = m.radio.frequency;
            let caps = std::sync::Arc::clone(&m.caps);
            let Some((band, hz)) = bands::step(from, up, |hz| caps.tunes(hz)) else {
                m.push_log(format!(
                    "FT8: no band {} that this radio reaches",
                    if up { "above" } else { "below" }
                ));
                return true;
            };
            drop(m);
            let Some(device) = ctx.device else {
                metrics(ctx.state).push_log("FT8: no radio to tune".to_string());
                return true;
            };
            let result = device.set_frequency(hz);
            let mut m = metrics(ctx.state);
            match result {
                Ok(tuned) => {
                    m.radio.frequency = tuned;
                    m.push_log(format!("FT8: {band}, dial {:.3} MHz", tuned as f64 / 1e6));
                }
                Err(e) => m.push_log(format!("FT8: could not tune to {band}: {e}")),
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::super::handle_key;
    use crate::config::LayoutConfig;
    use crate::state::{MapRange, SdrMetrics};
    use crate::ui::{self, PanelRegistry};
    use crossterm::event::{KeyCode, KeyEvent};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn press(state: &Arc<Mutex<SdrMetrics>>, code: KeyCode) {
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
        );
    }

    fn on_view() -> Arc<Mutex<SdrMetrics>> {
        let mut m = crate::ui::panels::radio::ft8::sample::evening();
        m.ui.section = crate::signal::ft8::SECTION.to_string();
        Arc::new(Mutex::new(m))
    }

    #[test]
    fn arrows_choose_stations_in_list_order() {
        let s = on_view();
        press(&s, KeyCode::Down);
        // The newest cycle by tone: YO3GCI at 340 Hz first.
        assert_eq!(
            s.lock().unwrap().ft8.view.selected.as_deref(),
            Some("YO3GCI")
        );
        press(&s, KeyCode::Down);
        assert_eq!(
            s.lock().unwrap().ft8.view.selected.as_deref(),
            Some("EA8ZQ")
        );
        press(&s, KeyCode::Up);
        press(&s, KeyCode::Up);
        assert_eq!(
            s.lock().unwrap().ft8.view.selected.as_deref(),
            Some("YO3GCI")
        );
        // The gain was not touched.
        assert_eq!(
            s.lock().unwrap().radio.gains,
            SdrMetrics::fixture().radio.gains
        );
    }

    #[test]
    fn z_steps_the_map_u_filters_c_clears() {
        let s = on_view();
        press(&s, KeyCode::Char('z'));
        assert_eq!(s.lock().unwrap().ft8.view.range, MapRange::Km1500);
        press(&s, KeyCode::Char('u'));
        {
            let m = s.lock().unwrap();
            assert!(m.ft8.view.only_yours);
            assert_eq!(m.ft8.listed().len(), 1);
        }
        press(&s, KeyCode::Char('c'));
        let m = s.lock().unwrap();
        assert!(m.ft8.decodes.is_empty() && m.ft8.stations.is_empty());
    }

    #[test]
    fn u_without_a_call_is_refused_and_said() {
        let s = on_view();
        s.lock().unwrap().home.callsign = None;
        press(&s, KeyCode::Char('u'));
        let m = s.lock().unwrap();
        assert!(!m.ft8.view.only_yours);
        assert!(m.ui.log.iter().any(|l| l.text.contains("Options")));
    }

    #[test]
    fn off_the_view_the_keys_are_left_alone() {
        let s = on_view();
        s.lock().unwrap().ui.section = "spectrum".into();
        press(&s, KeyCode::Char('z'));
        assert_eq!(s.lock().unwrap().ft8.view.range, MapRange::Log);
    }
}
