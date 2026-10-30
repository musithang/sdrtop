// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Maidenhead locator: a field of 20 by 10 degrees (letters A to R from
//! 180 W and from 90 S), a square of 2 by 1 degrees (digits 0 to 9), and a
//! subsquare of 5 by 2.5 minutes (letters A to X), each pair longitude first.
//! The same grid FT8's `g15` and `g25` fields count in.
//!
//! Written with the subsquare in lower case (`JN97ml`), the usual hand; read
//! in either case.

/// The locator of a place, `pairs` pairs long (2 for `JN97`, 3 for
/// `JN97ml`). `None` for a place that is not one.
pub fn locator(lat: f64, lon: f64, pairs: usize) -> Option<String> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    // The north pole and the antimeridian belong to the last cell, not to
    // one past it.
    let x = (lon + 180.0).min(360.0 - 1e-9);
    let y = (lat + 90.0).min(180.0 - 1e-9);
    let mut s = String::new();
    s.push((b'A' + (x / 20.0) as u8) as char);
    s.push((b'A' + (y / 10.0) as u8) as char);
    if pairs >= 2 {
        s.push((b'0' + ((x % 20.0) / 2.0) as u8) as char);
        s.push((b'0' + (y % 10.0) as u8) as char);
    }
    if pairs >= 3 {
        s.push((b'a' + ((x % 2.0) * 12.0) as u8) as char);
        s.push((b'a' + ((y % 1.0) * 24.0) as u8) as char);
    }
    Some(s)
}

/// The centre of a 4- or 6-character locator, or `None` for text that is
/// not one.
pub fn centre(loc: &str) -> Option<(f64, f64)> {
    let b = loc.trim().to_ascii_uppercase().into_bytes();
    if b.len() != 4 && b.len() != 6 {
        return None;
    }
    let field = |c: u8| (b'A'..=b'R').contains(&c).then(|| f64::from(c - b'A'));
    let digit = |c: u8| c.is_ascii_digit().then(|| f64::from(c - b'0'));
    let sub = |c: u8| (b'A'..=b'X').contains(&c).then(|| f64::from(c - b'A'));
    let mut lon = -180.0 + field(b[0])? * 20.0 + digit(b[2])? * 2.0;
    let mut lat = -90.0 + field(b[1])? * 10.0 + digit(b[3])?;
    if b.len() == 6 {
        lon += sub(b[4])? * (2.0 / 24.0) + 1.0 / 24.0;
        lat += sub(b[5])? * (1.0 / 24.0) + 1.0 / 48.0;
    } else {
        lon += 1.0;
        lat += 0.5;
    }
    Some((lat, lon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_starts_at_180_west_90_south() {
        assert_eq!(locator(-90.0, -180.0, 3).as_deref(), Some("AA00aa"));
        assert_eq!(locator(90.0, 180.0, 3).as_deref(), Some("RR99xx"));
        assert_eq!(locator(0.0, 0.0, 2).as_deref(), Some("JJ00"));
        assert_eq!(locator(91.0, 0.0, 2), None);
    }

    #[test]
    fn a_place_and_its_locator_agree_both_ways() {
        // Budapest, by the grid's own arithmetic: lon 19.04 is field J (0..20
        // from 0), square 9 (18..20), subsquare (1.04 * 12 = 12) m; lat 47.497
        // is field N (40..50), square 7, subsquare (0.497 * 24 = 11) l.
        assert_eq!(locator(47.497, 19.04, 3).as_deref(), Some("JN97ml"));
        let (lat, lon) = centre("JN97ml").unwrap();
        assert_eq!(locator(lat, lon, 3).as_deref(), Some("JN97ml"));
        assert!((lat - 47.497).abs() < 1.0 / 48.0 + 1e-9);
        assert!((lon - 19.04).abs() < 1.0 / 24.0 + 1e-9);
        // Every square's centre reads back as itself.
        for f in ["AA", "JN", "PM", "RR", "FN", "QF"] {
            for sq in ["00", "55", "99"] {
                let loc = format!("{f}{sq}");
                let (lat, lon) = centre(&loc).unwrap();
                assert_eq!(locator(lat, lon, 2).unwrap(), loc);
            }
        }
    }

    #[test]
    fn a_four_character_locator_is_the_middle_of_its_square() {
        assert_eq!(centre("JN97"), Some((47.5, 19.0)));
        assert_eq!(centre("jn97"), Some((47.5, 19.0)));
    }

    #[test]
    fn what_is_not_a_locator_is_refused() {
        // (RR73 is a square in the Arctic: only FT8 reads it as a word.)
        for bad in ["", "JN9", "SN97", "JN9A", "JN97yz", "JN97m"] {
            assert_eq!(centre(bad), None, "{bad}");
        }
    }
}
