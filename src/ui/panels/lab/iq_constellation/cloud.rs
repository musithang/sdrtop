//! The cloud's heat layers: splitting the dot cloud by local point density so a
//! dense core can be drawn hotter than the sparse skirt around it.

use ratatui::style::Color;

use super::BOUND;

/// Density-grid resolution (cells per axis) for the persistence colouring.
const DENSITY_GRID: usize = 28;

/// Number of heat buckets the cloud is split into, coolest → hottest.
pub(super) const HEAT_LEVELS: usize = 5;

/// Cool→hot persistence palette: sparse points are a cool blue, dense cores glow
/// orange — the classic phosphor-scope look.
pub(super) const HEAT: [Color; HEAT_LEVELS] = [
    Color::Rgb(35, 65, 115),  // sparse — cool blue
    Color::Rgb(30, 140, 150), // teal
    Color::Rgb(70, 180, 90),  // green
    Color::Rgb(215, 200, 55), // yellow
    Color::Rgb(245, 130, 35), // hot orange (dense core)
];

/// Split the cloud into [`HEAT_LEVELS`] layers by local point density, so each can
/// be drawn in its own heat colour. Bins points on a [`DENSITY_GRID`]² grid over
/// the canvas extent; a point's bucket is `sqrt(cell_count / max_count)` (the sqrt
/// spreads the low end so sparse structure stays visible).
pub(super) fn density_layers(coords: &[(f64, f64)]) -> Vec<Vec<(f64, f64)>> {
    let cell = |v: f64| -> usize {
        (((v + BOUND) / (2.0 * BOUND) * DENSITY_GRID as f64) as isize)
            .clamp(0, DENSITY_GRID as isize - 1) as usize
    };
    let mut counts = vec![0u32; DENSITY_GRID * DENSITY_GRID];
    for &(x, y) in coords {
        counts[cell(y) * DENSITY_GRID + cell(x)] += 1;
    }
    let max_c = counts.iter().copied().max().unwrap_or(1).max(1) as f64;

    let mut layers = vec![Vec::new(); HEAT_LEVELS];
    for &(x, y) in coords {
        let c = counts[cell(y) * DENSITY_GRID + cell(x)] as f64;
        let t = (c / max_c).sqrt();
        let k = ((t * HEAT_LEVELS as f64) as usize).min(HEAT_LEVELS - 1);
        layers[k].push((x, y));
    }
    layers
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::ring;
    use super::*;

    #[test]
    fn density_layers_partition_all_points() {
        let coords = ring(200, 1.0, 1.0);
        let layers = density_layers(&coords);
        assert_eq!(layers.len(), HEAT_LEVELS);
        let total: usize = layers.iter().map(|l| l.len()).sum();
        assert_eq!(
            total,
            coords.len(),
            "every point lands in exactly one layer"
        );
    }

    #[test]
    fn density_layers_hot_core_for_concentrated_cloud() {
        // Most points piled on one spot + a few scattered → the hottest layer is used.
        let mut coords = vec![(0.1, 0.1); 500];
        coords.extend(ring(20, 1.0, 1.0));
        let layers = density_layers(&coords);
        assert!(
            !layers[HEAT_LEVELS - 1].is_empty(),
            "dense core should reach the hot layer"
        );
    }
}
