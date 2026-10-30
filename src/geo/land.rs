// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where the land is: Natural Earth's land polygons at 1:110m (public
//! domain, naturalearthdata.com; the `natural-earth-vector` repository,
//! version 5.2.0-pre), embedded.
//!
//! **Generated, not typed** (POLICY.md, rule 7). `land.bin` is the GeoJSON in
//! `packaging/data/ne_110m_land.geojson` put through `convert` (test-only), and a test
//! converts it again on every run and compares every byte, so the embedded
//! map cannot drift from its source. To refresh it: replace the GeoJSON and
//! run `cargo test regenerate_the_embedded_land -- --ignored`.
//!
//! The format, little-endian: the magic `SDRLAND1`, a `u16` count of rings,
//! then each ring as a `u16` count of points and that many `i16` pairs of
//! longitude and latitude in hundredths of a degree (a kilometre, finer than
//! the 1:110m source itself). Holes are rings like any other: a point is on
//! land when it is inside an odd number of rings.

use std::sync::OnceLock;

const MAGIC: &[u8; 8] = b"SDRLAND1";

static EMBEDDED: &[u8] = include_bytes!("land.bin");

/// One ring: its points (longitude, latitude in degrees) and bounding box.
struct Ring {
    points: Vec<(f32, f32)>,
    lon: (f32, f32),
    lat: (f32, f32),
}

fn rings() -> &'static [Ring] {
    static RINGS: OnceLock<Vec<Ring>> = OnceLock::new();
    RINGS.get_or_init(|| parse(EMBEDDED).expect("the embedded land is well formed"))
}

fn parse(bytes: &[u8]) -> Option<Vec<Ring>> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let mut at = 0usize;
    let u16_at = |at: &mut usize| -> Option<u16> {
        let v = u16::from_le_bytes(rest.get(*at..*at + 2)?.try_into().ok()?);
        *at += 2;
        Some(v)
    };
    let count = u16_at(&mut at)?;
    let mut out = Vec::with_capacity(count.into());
    for _ in 0..count {
        let n = u16_at(&mut at)?;
        let mut points = Vec::with_capacity(n.into());
        for _ in 0..n {
            let lon = u16_at(&mut at)? as i16;
            let lat = u16_at(&mut at)? as i16;
            points.push((f32::from(lon) / 100.0, f32::from(lat) / 100.0));
        }
        let fold = |f: fn(&(f32, f32)) -> f32| {
            points
                .iter()
                .map(f)
                .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)))
        };
        let lon = fold(|p| p.0);
        let lat = fold(|p| p.1);
        out.push(Ring { points, lon, lat });
    }
    (at == rest.len()).then_some(out)
}

/// Whether a place is on land.
pub fn on_land(lat: f64, lon: f64) -> bool {
    let (x, y) = (lon as f32, lat as f32);
    let mut inside = false;
    for r in rings() {
        if x < r.lon.0 || x > r.lon.1 || y < r.lat.0 || y > r.lat.1 {
            continue;
        }
        // Even-odd ray casting towards the east.
        let pts = &r.points;
        let mut j = pts.len() - 1;
        for i in 0..pts.len() {
            let (xi, yi) = pts[i];
            let (xj, yj) = pts[j];
            if (yi > y) != (yj > y) && x < xi + (y - yi) * (xj - xi) / (yj - yi) {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

/// The GeoJSON of Natural Earth's land to the embedded format.
#[cfg(test)]
pub fn convert(geojson: &str) -> Vec<u8> {
    let doc: serde_json::Value = serde_json::from_str(geojson).expect("GeoJSON");
    let mut rings: Vec<Vec<(i16, i16)>> = Vec::new();
    for f in doc["features"].as_array().expect("features") {
        let g = &f["geometry"];
        let polygons: Vec<&serde_json::Value> = match g["type"].as_str() {
            Some("Polygon") => vec![&g["coordinates"]],
            Some("MultiPolygon") => g["coordinates"].as_array().unwrap().iter().collect(),
            other => panic!("unexpected geometry {other:?}"),
        };
        for p in polygons {
            for ring in p.as_array().unwrap() {
                let pts = ring
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| {
                        let q =
                            |v: &serde_json::Value| (v.as_f64().unwrap() * 100.0).round() as i16;
                        (q(&c[0]), q(&c[1]))
                    })
                    .collect();
                rings.push(pts);
            }
        }
    }
    let mut out = MAGIC.to_vec();
    out.extend((rings.len() as u16).to_le_bytes());
    for r in &rings {
        out.extend((r.len() as u16).to_le_bytes());
        for &(lon, lat) in r {
            out.extend(lon.to_le_bytes());
            out.extend(lat.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/packaging/data/ne_110m_land.geojson"
        );
        std::fs::read_to_string(path).expect("the Natural Earth source beside the code")
    }

    #[test]
    fn the_embedded_land_is_its_source_converted() {
        assert!(
            convert(&source()) == EMBEDDED,
            "land.bin is stale: regenerate it"
        );
    }

    #[test]
    fn the_land_is_the_shape_natural_earth_gives() {
        let r = rings();
        assert_eq!(r.len(), 128);
        assert_eq!(r.iter().map(|r| r.points.len()).sum::<usize>(), 5143);
    }

    #[test]
    fn known_land_and_known_sea() {
        for (name, lat, lon) in [
            ("Budapest", 47.5, 19.04),
            ("the Sahara", 23.0, 12.0),
            ("Siberia", 62.0, 100.0),
            ("Brazil", -10.0, -55.0),
            ("Australia", -25.0, 134.0),
            ("Antarctica", -85.0, 0.0),
            ("Honshu", 36.2, 138.25),
        ] {
            assert!(on_land(lat, lon), "{name}");
        }
        for (name, lat, lon) in [
            ("the North Atlantic", 30.0, -40.0),
            ("the Pacific", 0.0, -150.0),
            ("the Indian Ocean", -20.0, 80.0),
            ("the Arctic", 88.0, 0.0),
            ("the Mediterranean", 35.0, 18.0),
            ("the Caspian", 42.0, 50.5),
        ] {
            assert!(!on_land(lat, lon), "{name}");
        }
    }

    #[test]
    fn a_truncated_file_is_refused() {
        assert!(parse(&EMBEDDED[..EMBEDDED.len() - 1]).is_none());
        assert!(parse(b"NOTLAND!").is_none());
    }

    /// Writes `land.bin` from the source.
    #[test]
    #[ignore]
    fn regenerate_the_embedded_land() {
        let out = concat!(env!("CARGO_MANIFEST_DIR"), "/src/geo/land.bin");
        std::fs::write(out, convert(&source())).unwrap();
    }
}
