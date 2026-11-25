// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The golden render: every built-in layout and every registered panel,
//! drawn as text from a handful of fixture states at two terminal sizes, one
//! file each, into a directory.
//!
//! It asserts nothing. It is a yardstick for a change that must not move a
//! character on screen (a refactor, a split of the source into crates): write
//! the directory before, write it again after, and `diff -r` the two. Every
//! file is named after what it shows, so a difference points at the panel,
//! the layout, the state and the size it is in.
//!
//! Ignored, because it writes files and is only worth running in pairs:
//!
//! ```text
//! SDRTOP_GOLDEN_DIR=/tmp/before cargo test golden_render -- --ignored
//! ```

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use crate::app::App;
    use crate::state::{BandOccupancy, CellReading, SdrMetrics};

    /// The sizes every view is drawn at: the smallest terminal anyone runs
    /// in, and a large one.
    const SIZES: [(u16, u16); 2] = [(80, 24), (200, 60)];

    /// The fixture states, each with the name its files carry.
    fn states() -> Vec<(&'static str, SdrMetrics)> {
        vec![
            ("default", SdrMetrics::fixture()),
            ("streaming", SdrMetrics::fixture().streaming()),
            ("rtlsdr", SdrMetrics::fixture().rtlsdr()),
            ("named_chain", SdrMetrics::fixture().named_chain()),
            (
                "carrier",
                SdrMetrics::fixture().streaming().with_carrier(0.0, 30.0),
            ),
            ("net_survey", surveyed()),
        ]
    }

    /// A NET survey with a band measured: cells 12 to 46 observed, three
    /// busy runs in them.
    fn surveyed() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        let mut cells = vec![CellReading::default(); crate::signal::net::occupancy::CELLS];
        for (i, c) in cells.iter_mut().enumerate() {
            if (12..=46).contains(&i) {
                c.windows = 8_000;
                c.peak_dbfs = -70.0;
            }
        }
        for (i, duty, peak) in [(24usize, 0.62, -32.0), (36, 0.18, -48.0), (41, 0.95, -22.0)] {
            cells[i].duty = duty;
            cells[i].peak_dbfs = peak;
        }
        m.net.band = BandOccupancy {
            cells,
            noise_dbfs: Some(-78.4),
            trusted: true,
            tail: 2.07,
            spread: 40.2,
            window_s: 6.4e-6,
            watch_start: None,
            history: Default::default(),
            last_column: None,
            columns_taken: 0,
            tally: Default::default(),
        };
        m
    }

    /// The buffer as text, one line a row.
    ///
    /// **One thing is drawn from the wall clock:** the header's RX lamp
    /// breathes through `∙ • ● •`, a phase every 220 ms. Written as it is,
    /// two runs a second apart differ on every streaming header. So its four
    /// phases are written as one mark, `◌`, and only where they stand before
    /// ` RX `; a change to the lamp's place or width still shows.
    fn text(buffer: &ratatui::buffer::Buffer) -> String {
        let area = buffer.area;
        let mut out = String::new();
        for y in 0..area.height {
            let mut line: String = (0..area.width).map(|x| buffer[(x, y)].symbol()).collect();
            for phase in ["\u{2219}", "\u{2022}", "\u{25CF}"] {
                line = line.replace(&format!("{phase} RX "), "\u{25CC} RX ");
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("the golden directory is writable");
    }

    #[test]
    #[ignore = "writes the golden render; set SDRTOP_GOLDEN_DIR and run with --ignored"]
    fn golden_render() {
        let dir = std::env::var_os("SDRTOP_GOLDEN_DIR")
            .expect("SDRTOP_GOLDEN_DIR names the directory to write into");
        let dir = Path::new(&dir);
        let layouts = dir.join("layouts");
        let panels = dir.join("panels");
        std::fs::create_dir_all(&layouts).unwrap();
        std::fs::create_dir_all(&panels).unwrap();

        let theme = crate::Theme::sdr();
        let (mut engine, _) = App::build_ui("command_rail", &HashMap::new(), None, true);
        let mut presets = engine.preset_names();
        presets.sort();
        let mut names: Vec<&'static str> = engine.registered_panel_names().collect();
        names.sort_unstable();

        for preset in &presets {
            engine.set_preset(preset);
            let section = engine.scope().map(|s| s.id.clone()).unwrap_or_default();
            for (state_name, state) in states() {
                let mut m = state;
                // What the app does before every frame: the view on screen in
                // the screen's own state, and the mode it asks the engine for.
                let ui = crate::ui::UiState {
                    section: section.clone(),
                    active_preset: preset.clone(),
                    preset_names: engine.preset_names(),
                    ..crate::ui::UiState::default()
                };
                m.show_view(&section, preset);
                for (w, h) in SIZES {
                    let backend = ratatui::backend::TestBackend::new(w, h);
                    let mut terminal = ratatui::Terminal::new(backend).unwrap();
                    terminal
                        .draw(|f| engine.draw(f, &crate::ui::View::new(&m, &ui), &theme))
                        .unwrap();
                    write(
                        &layouts,
                        &format!("{preset}__{state_name}__{w}x{h}.txt"),
                        &text(terminal.backend().buffer()),
                    );
                }
            }
        }

        for name in names {
            for (state_name, m) in states() {
                for (w, h) in SIZES {
                    let backend = ratatui::backend::TestBackend::new(w, h);
                    let mut terminal = ratatui::Terminal::new(backend).unwrap();
                    terminal
                        .draw(|f| {
                            let area = f.area();
                            engine.registry().render_panel(
                                name,
                                f,
                                area,
                                &crate::ui::View::of(&m),
                                &theme,
                                false,
                            );
                        })
                        .unwrap();
                    write(
                        &panels,
                        &format!("{name}__{state_name}__{w}x{h}.txt"),
                        &text(terminal.backend().buffer()),
                    );
                }
            }
        }
    }
}
