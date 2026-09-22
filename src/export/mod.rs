// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Taking the data away.
//!
//! Design section 15. Not under `signal`, because it is not DSP, and not under
//! `ui`, because it is not drawing.
//!
//! **The reusable part is not the CSV writer.** It is [`provenance`], the header
//! that says what radio measured this and against what, and [`destination`], the
//! answer to where a file goes and what happens when it is already there. A
//! later IQ-sample export is a completely different body - binary, large, with
//! its own metadata sidecar - and wants both of those unchanged. The bodies here
//! are the first two things that needed them, and there are two of them on
//! purpose: one body proves nothing about a seam.

pub mod census;
pub mod destination;
pub mod occupancy;
pub mod provenance;

use std::path::PathBuf;

use crate::state::SdrMetrics;

/// One export: a file written, or the sentence saying why not.
pub struct Written {
    pub path: PathBuf,
    /// What the body had to say when it had no rows. `None` when it did.
    pub note: Option<String>,
    pub rows: usize,
}

/// Write both bodies of the NET section.
///
/// **Two files, one header, one destination answer.** The bodies know nothing
/// about each other; what they share is the part built to be shared. A body that
/// has nothing to say still gets a file, with the reason in its header - an
/// export that silently produced no file is the failure mode `destination`
/// exists to prevent, and "there was nothing to export" is itself a finding
/// worth keeping.
pub fn net_section(
    state: &SdrMetrics,
    dir: &std::path::Path,
    unix_secs: i64,
) -> Vec<Result<Written, String>> {
    let occupancy = match occupancy::rows(state) {
        Ok(rows) => (rows, None),
        Err(why) => (Vec::new(), Some(why)),
    };
    let census = census::rows(state);
    // The same two empties the panel tells apart (`NetState::
    // counting_addresses`): a quiet room is a finding, nobody counting is not.
    let census_note = census.is_empty().then(|| {
        if state.net.counting_addresses() {
            "no device has been counted: the decoder was reading addresses and none passed a CRC"
                .to_string()
        } else {
            "no device has been counted: nothing was decoding addresses".to_string()
        }
    });

    vec![
        one(
            state,
            dir,
            unix_secs,
            "net-band",
            occupancy::HEADER,
            occupancy.0,
            occupancy.1,
        ),
        one(
            state,
            dir,
            unix_secs,
            "net-census",
            census::HEADER,
            census,
            census_note,
        ),
    ]
}

fn one(
    state: &SdrMetrics,
    dir: &std::path::Path,
    unix_secs: i64,
    stem: &str,
    header: &str,
    rows: Vec<String>,
    note: Option<String>,
) -> Result<Written, String> {
    let path = dir.join(destination::file_name(stem, unix_secs));
    let mut lines = provenance::block(state, unix_secs);
    if let Some(note) = &note {
        lines.push(format!("{} note         {note}", provenance::COMMENT));
    }
    lines.push(header.to_string());
    let rows_written = rows.len();
    lines.extend(rows);
    destination::write(&path, &lines)?;
    Ok(Written {
        path,
        note,
        rows: rows_written,
    })
}

/// One CSV field, quoted when it has to be (RFC 4180 2.6 and 2.7): a field
/// holding a comma, a double quote or a line break goes in double quotes, with
/// every quote inside doubled. Anything else is written as it is.
///
/// Needed since addresses can carry a registrant's name: 21,696 of the IEEE's
/// names hold a comma and 78 a quote, and one written bare would shift every
/// column after it in that row.
pub fn csv_field(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains([',', '"', '\n', '\r']) {
        std::borrow::Cow::Owned(format!("\"{}\"", text.replace('"', "\"\"")))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_is_quoted_only_when_it_must_be() {
        assert_eq!(csv_field("Apple ..09:be"), "Apple ..09:be");
        assert_eq!(csv_field("Foo, Bar ..09:be"), "\"Foo, Bar ..09:be\"");
        assert_eq!(csv_field("The \"Q\" Co"), "\"The \"\"Q\"\" Co\"");
    }

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sdrtop-export-section-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// **The seam, demonstrated.** Two bodies that know nothing about each other
    /// produce two files with the same header and the same naming, which is the
    /// property a later IQ-sample export needs.
    #[test]
    fn two_bodies_share_one_header_and_one_naming() {
        let dir = scratch();
        let m = SdrMetrics::fixture().streaming();
        let out = net_section(&m, &dir, 1_788_632_561);
        assert_eq!(out.len(), 2);

        let paths: Vec<_> = out
            .iter()
            .map(|w| w.as_ref().unwrap().path.clone())
            .collect();
        assert!(
            paths[0].ends_with("net-band-20260905-182241.csv"),
            "{paths:?}"
        );
        assert!(
            paths[1].ends_with("net-census-20260905-182241.csv"),
            "{paths:?}"
        );

        let head = |p: &PathBuf| {
            std::fs::read_to_string(p)
                .unwrap()
                .lines()
                .take_while(|l| l.starts_with('#'))
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        };
        let a = head(&paths[0]);
        let b = head(&paths[1]);
        // The provenance is identical up to each file's own note line.
        let common = |v: &[String]| {
            v.iter()
                .filter(|l| !l.contains("note"))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(common(&a), common(&b));
        assert!(a.iter().any(|l| l.contains("exported")), "{a:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A body with nothing to say still gets a file, and the file says why.
    ///
    /// The alternative is an export that produced no file, which the user reads
    /// as "it worked" until they go looking.
    #[test]
    fn a_body_with_no_rows_still_writes_a_file_that_explains_itself() {
        let dir = scratch();
        let m = SdrMetrics::fixture().streaming();
        let out = net_section(&m, &dir, 1_788_632_561);

        for written in &out {
            let w = written.as_ref().unwrap();
            assert_eq!(w.rows, 0);
            let note = w.note.as_ref().expect("a reason");
            let text = std::fs::read_to_string(&w.path).unwrap();
            assert!(text.contains(note), "the reason is not in the file: {text}");
            // The column header is there even with no rows, so the shape of the
            // eventual answer is visible.
            assert!(
                text.lines().any(|l| !l.starts_with('#') && l.contains(',')),
                "no column header: {text}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Running it twice in the same second refuses rather than overwriting, and
    /// says which file it refused.
    #[test]
    fn a_second_export_in_the_same_second_is_refused() {
        let dir = scratch();
        let m = SdrMetrics::fixture().streaming();
        assert!(net_section(&m, &dir, 1_788_632_561)
            .iter()
            .all(|w| w.is_ok()));
        let again = net_section(&m, &dir, 1_788_632_561);
        for result in &again {
            let err = result.as_ref().err().expect("refused");
            assert!(err.contains("exists already"), "{err}");
            assert!(err.contains("net-"), "the path is named: {err}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
