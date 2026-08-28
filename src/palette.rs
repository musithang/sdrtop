use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Selectable waterfall colour gradient (DSN-2026-04 §03). `Classic` follows the
/// active theme's own palette (so each theme keeps its look); the others are
/// fixed "cyberdeck" gradients independent of the theme. Cycled live with `P`
/// while the waterfall is focused, persisted in `[display] waterfall_palette`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaterfallPalette {
    /// The active theme's gradient - the existing behaviour, and the default.
    #[default]
    Classic,
    /// Warm amber CRT.
    Amber,
    /// Cold blue→cyan→white.
    Ice,
    /// Retro phosphor green.
    Phosphor,
}

impl WaterfallPalette {
    /// Cycle order for the `P` toggle: Classic → Amber → Ice → Phosphor → Classic.
    pub fn next(self) -> Self {
        match self {
            Self::Classic => Self::Amber,
            Self::Amber => Self::Ice,
            Self::Ice => Self::Phosphor,
            Self::Phosphor => Self::Classic,
        }
    }

    /// Lower-case name for log messages (matches the serialized form).
    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Amber => "amber",
            Self::Ice => "ice",
            Self::Phosphor => "phosphor",
        }
    }

    /// Fixed gradient stops for the non-theme palettes; `None` for `Classic`
    /// (which defers to the theme's own gradient).
    fn stops(self) -> Option<&'static [(f32, u8, u8, u8)]> {
        match self {
            Self::Classic => None,
            Self::Amber => Some(&[
                (0.00, 10, 4, 0),
                (0.35, 90, 40, 0),
                (0.65, 200, 110, 10),
                (0.85, 255, 180, 40),
                (1.00, 255, 230, 160),
            ]),
            Self::Ice => Some(&[
                (0.00, 0, 4, 20),
                (0.35, 0, 55, 120),
                (0.65, 0, 150, 220),
                (0.85, 120, 220, 255),
                (1.00, 235, 250, 255),
            ]),
            Self::Phosphor => Some(&[
                (0.00, 0, 12, 4),
                (0.35, 0, 70, 25),
                (0.65, 20, 180, 60),
                (0.85, 120, 240, 130),
                (1.00, 220, 255, 220),
            ]),
        }
    }
}

/// Piecewise-linear interpolation over arbitrary gradient stops (sorted by `t`).
fn interp_stops(stops: &[(f32, u8, u8, u8)], t: f32) -> (u8, u8, u8) {
    for w in stops.windows(2) {
        let (t0, r0, g0, b0) = w[0];
        let (t1, r1, g1, b1) = w[1];
        if t <= t1 {
            let s = if (t1 - t0).abs() < f32::EPSILON {
                0.0
            } else {
                (t - t0) / (t1 - t0)
            };
            return (lerp(r0, r1, s), lerp(g0, g1, s), lerp(b0, b1, s));
        }
    }
    stops
        .last()
        .map(|&(_, r, g, b)| (r, g, b))
        .unwrap_or((255, 0, 0))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorDepth {
    TrueColor,
    Color256,
    Color16,
}

static CACHED_DEPTH: OnceLock<ColorDepth> = OnceLock::new();

impl ColorDepth {
    pub fn detect() -> Self {
        *CACHED_DEPTH.get_or_init(|| {
            let colorterm = std::env::var("COLORTERM")
                .unwrap_or_default()
                .to_lowercase();
            if colorterm == "truecolor" || colorterm == "24bit" {
                return Self::TrueColor;
            }
            let term = std::env::var("TERM").unwrap_or_default();
            if term.contains("256color") {
                return Self::Color256;
            }
            Self::Color16
        })
    }
}

// 16-step xterm-256 gradient: dark blue (cold) → cyan → green → yellow → red (hot)
const PALETTE_256: [u8; 16] = [
    17,  // #00005f dark blue
    18,  // #000087
    19,  // #0000af
    21,  // #0000ff blue
    27,  // #005fff blue-cyan
    33,  // #0087ff
    51,  // #00ffff cyan
    46,  // #00ff00 green
    82,  // #5fff00
    118, // #87ff00
    226, // #ffff00 yellow
    220, // #ffd700
    214, // #ffaf00
    208, // #ff8700
    202, // #ff5f00
    196, // #ff0000 red
];

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t) as u8
}

// Piecewise linear gradient: dark blue → blue → cyan → green → yellow → red
fn truecolor_gradient(t: f32) -> (u8, u8, u8) {
    const STOPS: &[(f32, u8, u8, u8)] = &[
        (0.00, 0, 0, 128),
        (0.25, 0, 0, 255),
        (0.40, 0, 255, 255),
        (0.55, 0, 255, 0),
        (0.70, 255, 255, 0),
        (1.00, 255, 0, 0),
    ];
    for i in 0..STOPS.len() - 1 {
        let (t0, r0, g0, b0) = STOPS[i];
        let (t1, r1, g1, b1) = STOPS[i + 1];
        if t <= t1 {
            let s = (t - t0) / (t1 - t0);
            return (lerp(r0, r1, s), lerp(g0, g1, s), lerp(b0, b1, s));
        }
    }
    (255, 0, 0)
}

/// Map a dBFS value to a terminal color appropriate for the detected color depth.
pub fn magnitude_to_color(db: f32, db_min: f32, db_max: f32, depth: ColorDepth) -> Color {
    let t = ((db - db_min) / (db_max - db_min)).clamp(0.0, 1.0);
    match depth {
        ColorDepth::TrueColor => {
            let (r, g, b) = truecolor_gradient(t);
            Color::Rgb(r, g, b)
        }
        ColorDepth::Color256 => {
            let idx = ((t * 15.0) as usize).min(15);
            Color::Indexed(PALETTE_256[idx])
        }
        ColorDepth::Color16 => match (t * 3.0) as u8 {
            0 => Color::DarkGray,
            1 => Color::Blue,
            2 => Color::Cyan,
            _ => Color::White,
        },
    }
}

/// Like `magnitude_to_color` but uses the theme's custom gradient for TrueColor.
/// For Color256 and Color16 it falls back to the existing hardcoded palettes.
pub fn magnitude_to_color_themed(
    db: f32,
    db_min: f32,
    db_max: f32,
    depth: ColorDepth,
    theme: &crate::Theme,
) -> Color {
    let t = ((db - db_min) / (db_max - db_min)).clamp(0.0, 1.0);
    match depth {
        ColorDepth::TrueColor => theme.palette_color(t),
        ColorDepth::Color256 | ColorDepth::Color16 => magnitude_to_color(db, db_min, db_max, depth),
    }
}

/// Like [`magnitude_to_color_themed`] but honours a selected [`WaterfallPalette`]:
/// `Classic` defers to the theme gradient (identical to the themed version), the
/// others use their fixed stops on TrueColor. On 256/16-colour terminals every
/// palette falls back to the shared hardcoded ramp (the curve stays sharp, only
/// the colour-AA is lost - DSN-2026-04 §05).
pub fn magnitude_to_color_palette(
    db: f32,
    db_min: f32,
    db_max: f32,
    depth: ColorDepth,
    theme: &crate::Theme,
    palette: WaterfallPalette,
) -> Color {
    match (depth, palette.stops()) {
        (ColorDepth::TrueColor, Some(stops)) => {
            let t = ((db - db_min) / (db_max - db_min)).clamp(0.0, 1.0);
            let (r, g, b) = interp_stops(stops, t);
            Color::Rgb(r, g, b)
        }
        (ColorDepth::TrueColor, None) => {
            magnitude_to_color_themed(db, db_min, db_max, depth, theme)
        }
        _ => magnitude_to_color(db, db_min, db_max, depth),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truecolor_cold_end_is_dark_blue() {
        let c = magnitude_to_color(-120.0, -120.0, 0.0, ColorDepth::TrueColor);
        assert_eq!(c, Color::Rgb(0, 0, 128));
    }

    #[test]
    fn truecolor_hot_end_is_red() {
        let c = magnitude_to_color(0.0, -120.0, 0.0, ColorDepth::TrueColor);
        assert_eq!(c, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn clamp_below_min_same_as_min() {
        let at_min = magnitude_to_color(-120.0, -120.0, 0.0, ColorDepth::TrueColor);
        let below = magnitude_to_color(-200.0, -120.0, 0.0, ColorDepth::TrueColor);
        assert_eq!(
            at_min, below,
            "values below db_min should clamp to cold end"
        );
    }

    #[test]
    fn color16_covers_all_levels() {
        let cold = magnitude_to_color(-120.0, -120.0, 0.0, ColorDepth::Color16);
        let hot = magnitude_to_color(0.0, -120.0, 0.0, ColorDepth::Color16);
        assert_eq!(cold, Color::DarkGray);
        assert_eq!(hot, Color::White);
    }

    #[test]
    fn themed_truecolor_uses_theme_palette() {
        let theme = crate::Theme::sdr();
        let cold = magnitude_to_color_themed(-120.0, -120.0, 0.0, ColorDepth::TrueColor, &theme);
        let hot = magnitude_to_color_themed(0.0, -120.0, 0.0, ColorDepth::TrueColor, &theme);
        // SDR palette cold end is (10, 10, 80), hot end is (255, 50, 20)
        assert_eq!(cold, Color::Rgb(10, 10, 80));
        assert_eq!(hot, Color::Rgb(255, 50, 20));
    }

    #[test]
    fn themed_256color_falls_back_to_existing() {
        let theme = crate::Theme::sdr();
        let result = magnitude_to_color_themed(-60.0, -120.0, 0.0, ColorDepth::Color256, &theme);
        let existing = magnitude_to_color(-60.0, -120.0, 0.0, ColorDepth::Color256);
        assert_eq!(result, existing);
    }

    #[test]
    fn palette_cycle_wraps() {
        use WaterfallPalette::*;
        assert_eq!(Classic.next(), Amber);
        assert_eq!(Amber.next(), Ice);
        assert_eq!(Ice.next(), Phosphor);
        assert_eq!(Phosphor.next(), Classic);
        assert_eq!(WaterfallPalette::default(), Classic);
    }

    #[test]
    fn classic_palette_matches_themed() {
        let theme = crate::Theme::sdr();
        for db in [-120.0, -90.0, -40.0, 0.0] {
            let c = magnitude_to_color_palette(
                db,
                -120.0,
                0.0,
                ColorDepth::TrueColor,
                &theme,
                WaterfallPalette::Classic,
            );
            let t = magnitude_to_color_themed(db, -120.0, 0.0, ColorDepth::TrueColor, &theme);
            assert_eq!(c, t, "classic must equal themed at {db} dBFS");
        }
    }

    #[test]
    fn amber_palette_uses_fixed_stops() {
        let theme = crate::Theme::sdr();
        let cold = magnitude_to_color_palette(
            -120.0,
            -120.0,
            0.0,
            ColorDepth::TrueColor,
            &theme,
            WaterfallPalette::Amber,
        );
        let hot = magnitude_to_color_palette(
            0.0,
            -120.0,
            0.0,
            ColorDepth::TrueColor,
            &theme,
            WaterfallPalette::Amber,
        );
        assert_eq!(cold, Color::Rgb(10, 4, 0)); // amber cold end
        assert_eq!(hot, Color::Rgb(255, 230, 160)); // amber hot end
    }

    #[test]
    fn non_classic_palette_falls_back_on_256() {
        let theme = crate::Theme::sdr();
        let amber = magnitude_to_color_palette(
            -60.0,
            -120.0,
            0.0,
            ColorDepth::Color256,
            &theme,
            WaterfallPalette::Amber,
        );
        let plain = magnitude_to_color(-60.0, -120.0, 0.0, ColorDepth::Color256);
        assert_eq!(
            amber, plain,
            "256-colour fallback ignores the palette choice"
        );
    }
}
