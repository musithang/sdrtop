use std::collections::HashMap;
use ratatui::{layout::Rect, Frame};
use crate::state::SdrMetrics;
use crate::ui::panel::Panel;

pub struct PanelRegistry {
    panels: HashMap<&'static str, Box<dyn Panel>>,
}

impl PanelRegistry {
    pub fn new() -> Self {
        Self { panels: HashMap::new() }
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

    pub fn render_panel(&self, name: &str, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        if let Some(panel) = self.get(name) {
            panel.render(f, area, state, theme, focused);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::panel::Panel;

    struct NamedPanel(&'static str);

    impl Panel for NamedPanel {
        fn name(&self) -> &'static str { self.0 }
        fn min_size(&self) -> (u16, u16) { (0, 0) }
        fn render(&self, _f: &mut Frame, _area: Rect, _state: &SdrMetrics, _theme: &crate::Theme, _focused: bool) {}
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
