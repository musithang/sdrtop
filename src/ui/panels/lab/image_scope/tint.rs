//! The scope's colour vocabulary.
//!
//! Carrier, image and DC each get one colour, and every part of the panel uses
//! the same one: the `▼` in the read-out is the same yellow as the `▼` over the
//! bar, which is the same yellow as the bar itself. That correspondence is what
//! lets a reader tie a number to a column without a legend, so the three colours
//! are resolved once here rather than per-zone.

use ratatui::style::Color;

/// The three colours a column can take, plus the base for everything else and
/// the rule colour the chart's gutter, ticks and axis are drawn in.
pub(super) struct Tint {
    pub base: Color,
    pub carrier: Color,
    pub image: Color,
    pub dc: Color,
    pub rule: Color,
}

impl Tint {
    pub(super) fn new(theme: &crate::Theme) -> Self {
        Self {
            // Half-brightness accent: the un-marked spectrum is context, and at
            // full strength it competes with the three columns that matter.
            base: dim(theme.border_accent, 0.5),
            carrier: theme.value_hi,
            image: theme.status_warn,
            dc: theme.status_crit,
            rule: theme.border_dim,
        }
    }
}

/// Dim an `Rgb` colour's brightness by `f`. Non-Rgb colours pass through.
fn dim(c: Color, f: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * f) as u8,
            (g as f32 * f) as u8,
            (b as f32 * f) as u8,
        ),
        other => other,
    }
}

/// Colour for an image-suppression figure: deeper is better.
pub(super) fn supp_color(supp_db: f32, theme: &crate::Theme) -> Color {
    if supp_db >= 40.0 {
        theme.status_ok
    } else if supp_db >= 20.0 {
        theme.status_warn
    } else {
        theme.status_crit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supp_color_thresholds() {
        let t = crate::theme::Theme::sdr();
        assert_eq!(supp_color(50.0, &t), t.status_ok);
        assert_eq!(supp_color(30.0, &t), t.status_warn);
        assert_eq!(supp_color(10.0, &t), t.status_crit);
    }

    #[test]
    fn dim_scales_rgb_and_passes_named_colours_through() {
        assert_eq!(dim(Color::Rgb(200, 100, 50), 0.5), Color::Rgb(100, 50, 25));
        // A 16-colour theme has no channels to scale; dimming must not turn one
        // into a black hole.
        assert_eq!(dim(Color::Yellow, 0.5), Color::Yellow);
    }
}
