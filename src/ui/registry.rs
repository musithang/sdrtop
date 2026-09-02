// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use crate::state::SdrMetrics;
use crate::ui::chrome::frame;
use crate::ui::panel::{FrameStyle, Panel};
use ratatui::{layout::Rect, Frame};
use std::collections::HashMap;

pub struct PanelRegistry {
    panels: HashMap<&'static str, Box<dyn Panel>>,
}

impl PanelRegistry {
    pub fn new() -> Self {
        Self {
            panels: HashMap::new(),
        }
    }

    pub fn register(&mut self, panel: impl Panel + 'static) {
        self.panels.insert(panel.name(), Box::new(panel));
    }

    pub fn get(&self, name: &str) -> Option<&dyn Panel> {
        self.panels.get(name).map(|p| p.as_ref())
    }

    pub fn panels_iter(&self) -> impl Iterator<Item = &Box<dyn Panel>> {
        self.panels.values()
    }

    /// Frame a panel and render it into what is left.
    ///
    /// The frame is drawn here, from the panel's own [`PanelChrome`], so the
    /// border rule, the nameplate and the `[STALE]` tag are decided once for the
    /// whole deck instead of once per panel file. The
    /// [`FrameStyle::SelfFramed`] branch is the trait default's safety net - no
    /// registered panel takes it, and the lint says so.
    ///
    /// [`PanelChrome`]: crate::ui::panel::PanelChrome
    pub fn render_panel(
        &self,
        name: &str,
        f: &mut Frame,
        area: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        focused: bool,
    ) {
        let Some(panel) = self.get(name) else { return };
        let chrome = panel.chrome(state);
        if chrome.frame == FrameStyle::SelfFramed {
            panel.render(f, area, state, theme, focused);
            return;
        }
        let stale = chrome.staleness.resolve(state);
        // A frame that leaves no inner rect still draws: the border is the only
        // honest thing to show in a panel too small for its contents.
        if let Some(inner) =
            frame::render_frame(f, area, &chrome, panel.focus_key(), stale, focused, theme)
        {
            panel.render(f, inner, state, theme, focused);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::panel::Panel;

    struct NamedPanel(&'static str);

    impl Panel for NamedPanel {
        fn name(&self) -> &'static str {
            self.0
        }
        fn min_size(&self) -> (u16, u16) {
            (0, 0)
        }
        fn render(
            &self,
            _f: &mut Frame,
            _area: Rect,
            _state: &SdrMetrics,
            _theme: &crate::Theme,
            _focused: bool,
        ) {
        }
    }

    #[test]
    fn register_and_retrieve() {
        let mut reg = PanelRegistry::new();
        reg.register(NamedPanel("alpha"));
        reg.register(NamedPanel("beta"));
        assert!(reg.get("alpha").is_some());
        assert!(reg.get("beta").is_some());
        assert!(reg.get("gamma").is_none());
    }
}
