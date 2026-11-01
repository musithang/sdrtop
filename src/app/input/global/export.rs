// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `[o]` - write the section's data out.

use super::super::{metrics, InputCtx};

/// Export the NET section, or the FT8 view's decodes, and say in the log
/// exactly what happened to each file.
///
/// **Section-scoped, and it declines rather than absorbing.** The same rule
/// `[m]` follows: a global arm added for one section must fall through when that
/// section is not on screen, or it quietly takes a key away from the rest of the
/// deck.
///
/// The write happens here rather than on a task. It is two files of under a
/// hundred rows and the user asked for it this instant; a task would buy nothing
/// and would put the "did it work" answer somewhere other than the keypress.
pub(super) fn export_section(ctx: &mut InputCtx<'_>) -> bool {
    let mut m = metrics(ctx.state);
    let ft8 = m.ui.is_ft8_section();
    if !m.ui.is_net_section() && !ft8 {
        return false;
    }

    let dir = crate::export::destination::default_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        // The one directory this is allowed to create, because it is the one it
        // chose. A directory the *user* named is never created for them.
        m.push_log(format!("Export: cannot use {}: {e}", dir.display()));
        return true;
    }
    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let results = if ft8 {
        vec![crate::export::ft8_section(&m, &dir, unix_secs)]
    } else {
        crate::export::net_section(&m, &dir, unix_secs)
    };
    for result in results {
        m.push_log(match result {
            Ok(w) => {
                let what = match (w.rows, &w.note) {
                    (0, Some(note)) => format!("nothing to write, {note}"),
                    (n, _) => format!("{n} rows"),
                };
                format!("Export: {} ({what})", w.path.display())
            }
            Err(why) => format!("Export failed: {why}"),
        });
    }
    true
}
