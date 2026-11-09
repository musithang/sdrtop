// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The FT8 views' keys, live whenever World or the FT8 Lab is on screen.
//!
//! **Section-scoped and declining**, as the pager's are: on any other view
//! every one of these keys goes on to what it does there. Here `↑` `↓` choose
//! a station rather than step the gain, unless a gain stage was picked with
//! `,` `.`, which hands them back to it until the stage is let go,
//! and `←` `→` step between FT8's bands: the one key on this view that
//! tunes, and only to a band the radio reaches. Opening the view retunes
//! nothing.
//!
//! **One key between World and the Lab.** `L` opens the FT8 Lab from World
//! and World from the Lab, each at the layout it was last left from. The
//! selection is the state's, so the station chosen on one side is the
//! station on the other.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{global, metrics, InputCtx, KeyAction};
use crate::signal::ft8::bands;
use crate::state::ft8::MapDir;

/// What `L` opens on each side the first time.
const FIRST_WORLD: &str = "radio_ft8";
const FIRST_LAB: &str = "lab_ft8_signal";

/// Handle `key` if an FT8 view is on screen and it is one of its own.
/// Returns whether it was.
pub(super) fn handle(key: KeyEvent, ctx: &mut InputCtx<'_>) -> bool {
    let mut m = metrics(ctx.state);
    if !m.ui.is_ft8_section() {
        return false;
    }
    // A gain stage picked by hand with `,` `.` takes the arrows back, as on
    // every view, until the stage is let go: the attenuator has no other key.
    let gain_picked = m.ui.gain_stage.is_some();
    match key.code {
        KeyCode::Up | KeyCode::Down if gain_picked => return false,
        KeyCode::Up => m.ft8.select_step(false),
        KeyCode::Down => m.ft8.select_step(true),
        // Shown as `L`; the input layer folds case, so it arrives as `l`.
        // With World's map focused, `l` is the map's day and night instead.
        KeyCode::Char('l') => {
            let leaving = ctx.engine.active_preset().to_string();
            let target = if m.ui.is_ft8_lab() {
                m.ft8.view.lab_preset = Some(leaving);
                m.ft8
                    .view
                    .world_preset
                    .clone()
                    .unwrap_or_else(|| FIRST_WORLD.into())
            } else {
                m.ft8.view.world_preset = Some(leaving);
                m.ft8
                    .view
                    .lab_preset
                    .clone()
                    .unwrap_or_else(|| FIRST_LAB.into())
            };
            drop(m);
            super::global::presets::try_set_preset(ctx.engine, ctx.state, &target);
        }
        // The map's range is World's: the Lab has no map.
        KeyCode::Char('z') if !m.ui.is_ft8_lab() => {
            // A range chosen ends a zoom: the two would say different things.
            m.ft8.view.zoomed = false;
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

/// The map's focus mode: what it draws. Each switch says in the log what it
/// now is, and the map's title says whatever one hides. Anything else goes
/// on as it would without focus: the view's own `z` `u` `c` and arrows, and
/// the global keys.
pub(super) fn ft8_map(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    // On the map the arrows move over the map; a gain stage picked with
    // `,` `.` still takes them, as everywhere.
    let dir = match key.code {
        _ if m.ui.gain_stage.is_some() => None,
        KeyCode::Up => Some(MapDir::Up),
        KeyCode::Down => Some(MapDir::Down),
        KeyCode::Left => Some(MapDir::Left),
        KeyCode::Right => Some(MapDir::Right),
        _ => None,
    };
    if let Some(dir) = dir {
        let Some(home) = m.home.position() else {
            m.push_log("FT8 map: no position, so no map to move on".to_string());
            return KeyAction::Continue;
        };
        if !m.ft8.step_on_map(home, dir) {
            m.push_log("FT8 map: no station that way".to_string());
        }
        return KeyAction::Continue;
    }
    if key.code == KeyCode::Enter {
        let v = &mut m.ft8.view;
        let line = if v.zoomed {
            v.zoomed = false;
            "FT8 map: back home".to_string()
        } else if let Some(call) = v.selected.clone() {
            v.zoomed = true;
            format!("FT8 map: zoomed on {call}")
        } else {
            "FT8 map: choose a station to zoom on first".to_string()
        };
        m.push_log(line);
        return KeyAction::Continue;
    }
    let onoff = |on: bool| if on { "on" } else { "off" };
    let line = {
        let map = &mut m.ft8.view.map;
        match key.code {
            KeyCode::Char('r') => {
                map.rays = !map.rays;
                format!("FT8 map: rays {}", onoff(map.rays))
            }
            KeyCode::Char('g') => {
                map.grid = !map.grid;
                format!("FT8 map: locator grid {}", onoff(map.grid))
            }
            KeyCode::Char('k') => {
                map.rings = !map.rings;
                format!("FT8 map: range rings {}", onoff(map.rings))
            }
            KeyCode::Char('l') => {
                map.sun = !map.sun;
                format!("FT8 map: day, night and grey line {}", onoff(map.sun))
            }
            KeyCode::Char('n') => {
                map.names = map.names.next();
                format!("FT8 map: names {}", map.names.word())
            }
            KeyCode::Char('t') => {
                map.window = map.window.next();
                format!("FT8 map: stations heard in {}", map.window.word())
            }
            KeyCode::Char('w') => {
                map.long_path = !map.long_path;
                format!("FT8 map: the long way round {}", onoff(map.long_path))
            }
            KeyCode::Char('b') => {
                map.this_band = !map.this_band;
                if map.this_band {
                    "FT8 map: only stations on this band".to_string()
                } else {
                    "FT8 map: stations on every band".to_string()
                }
            }
            _ => {
                drop(m);
                return global::handle(key, ctx);
            }
        }
    };
    m.push_log(line);
    KeyAction::Continue
}

#[cfg(test)]
mod tests {
    use super::super::handle_key;
    use crate::config::LayoutConfig;
    use crate::state::ft8::MapRange;
    use crate::state::SdrMetrics;
    use crate::ui::{self, PanelRegistry};
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
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

    /// `L` crosses from World to the FT8 Lab and back, each time to the
    /// layout last left, the chosen station carried; on the Lab `z` (the
    /// map's range) does nothing, since the Lab has no map.
    #[test]
    fn l_crosses_between_world_and_the_lab_with_the_station() {
        let (mut engine, keys) =
            crate::app::App::build_ui("radio_ft8", &HashMap::new(), None, true);
        engine.set_preset("radio_ft8");
        let s = on_view();
        s.lock().unwrap().ft8.view.selected = Some("JA1VQD".into());
        let press_on = |code: KeyCode, engine: &mut ui::LayoutEngine| {
            let mut footer = true;
            handle_key(KeyEvent::from(code), &s, None, engine, &mut footer, &keys);
            // What `App::draw` does every frame: the section is the layout's.
            let section = engine.scope().map(|sc| sc.id.clone()).unwrap_or_default();
            s.lock().unwrap().ui.section = section;
        };
        press_on(KeyCode::Char('L'), &mut engine);
        assert_eq!(engine.active_preset(), "lab_ft8_signal");
        {
            let m = s.lock().unwrap();
            assert!(m.ui.is_ft8_lab() && m.ui.is_ft8_section());
            assert_eq!(m.ft8.view.selected.as_deref(), Some("JA1VQD"));
            assert_eq!(m.ft8.view.world_preset.as_deref(), Some("radio_ft8"));
        }
        let range = s.lock().unwrap().ft8.view.range;
        press_on(KeyCode::Char('z'), &mut engine);
        assert_eq!(s.lock().unwrap().ft8.view.range, range);
        press_on(KeyCode::Char('L'), &mut engine);
        assert_eq!(engine.active_preset(), "radio_ft8");
        let m = s.lock().unwrap();
        assert_eq!(m.ui.section, crate::signal::ft8::SECTION);
        assert_eq!(m.ft8.view.lab_preset.as_deref(), Some("lab_ft8_signal"));
        assert_eq!(m.ft8.view.selected.as_deref(), Some("JA1VQD"));
    }

    #[test]
    fn m_focuses_the_map_and_its_keys_switch_what_it_draws() {
        use crate::state::ft8::{Names, Window};
        let (mut engine, keys) =
            crate::app::App::build_ui("radio_ft8", &HashMap::new(), None, true);
        let s = on_view();
        let mut press_on = |code: KeyCode| {
            let mut footer = true;
            handle_key(
                KeyEvent::from(code),
                &s,
                None,
                &mut engine,
                &mut footer,
                &keys,
            );
        };
        press_on(KeyCode::Char('m'));
        for k in ['r', 'g', 'k', 'l', 'n', 't', 'b', 'w', 'z'] {
            press_on(KeyCode::Char(k));
        }
        let m = s.lock().unwrap();
        assert_eq!(m.ui.focused_panel.as_deref(), Some("ft8_map"));
        let map = &m.ft8.view.map;
        assert!(!map.rays && !map.grid && !map.rings && !map.sun && map.this_band);
        assert!(map.long_path);
        assert_eq!(map.names, Names::ThisCycle);
        assert_eq!(map.window, Window::Session);
        // The view's own keys still work in focus.
        assert_eq!(m.ft8.view.range, MapRange::Km1500);
        assert!(m
            .ui
            .log
            .iter()
            .any(|l| l.text.contains("FT8 map: rays off")));
    }

    #[test]
    fn in_the_maps_focus_arrows_move_on_the_map_and_enter_zooms() {
        let (mut engine, keys) =
            crate::app::App::build_ui("radio_ft8", &HashMap::new(), None, true);
        let s = on_view();
        let mut press_on = |code: KeyCode| {
            let mut footer = true;
            handle_key(
                KeyEvent::from(code),
                &s,
                None,
                &mut engine,
                &mut footer,
                &keys,
            );
        };
        press_on(KeyCode::Char('m'));
        // North of Budapest there are stations in the sample; which one is
        // the direction tests' business, here only that the map chose.
        press_on(KeyCode::Up);
        let chosen = s.lock().unwrap().ft8.view.selected.clone();
        assert!(chosen.is_some(), "Up chose nobody");
        let dial = s.lock().unwrap().radio.frequency;
        press_on(KeyCode::Left);
        // The arrows in focus are the map's: the band did not change.
        assert_eq!(s.lock().unwrap().radio.frequency, dial);
        press_on(KeyCode::Enter);
        assert!(s.lock().unwrap().ft8.view.zoomed);
        press_on(KeyCode::Enter);
        assert!(!s.lock().unwrap().ft8.view.zoomed);
        // Esc leaves the focus and the zoom with it; z ends one too.
        press_on(KeyCode::Enter);
        press_on(KeyCode::Esc);
        assert!(!s.lock().unwrap().ft8.view.zoomed);
        s.lock().unwrap().ft8.view.zoomed = true;
        press_on(KeyCode::Char('z'));
        assert!(!s.lock().unwrap().ft8.view.zoomed);
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

    #[test]
    fn a_picked_gain_stage_takes_the_arrows_back() {
        let s = on_view();
        s.lock().unwrap().ui.gain_stage = Some(0);
        press(&s, KeyCode::Down);
        assert_eq!(s.lock().unwrap().ft8.view.selected, None);
    }
}
