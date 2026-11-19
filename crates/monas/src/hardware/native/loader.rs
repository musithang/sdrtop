// SPDX-License-Identifier: GPL-3.0-or-later

use crate::hardware::runtime_lib;
use libloading::Library;

pub(super) fn load<T>(
    backend: &str,
    candidates: &[&str],
    resolve: fn(Library) -> Result<T, String>,
) -> Result<T, String> {
    let mut errors = Vec::new();
    for name in candidates {
        // The names come from the backend's fixed library list
        let result = unsafe { runtime_lib::open(name) }
            .map_err(|err| system_text(&err))
            .and_then(resolve);
        match result {
            Ok(api) => return Ok(api),
            Err(err) => errors.push(format!("{name}: {err}")),
        }
    }
    Err(format!(
        "{backend} backend unavailable. Install a compatible {backend} runtime library. Tried: {}",
        errors.join("; ")
    ))
}

/// What the system said about a failed open or lookup, the loader's own text
/// (`libhackrf.so.0: cannot open shared object file: No such file or
/// directory`), which is what tells a user which file is missing or which
/// symbol their library lacks. libloading's own message for these is only
/// `dlopen failed`; the system's text is its source.
fn system_text(err: &libloading::Error) -> String {
    use std::error::Error as _;
    match err.source() {
        Some(system) => system.to_string(),
        None => err.to_string(),
    }
}

/// The caller must use the symbol's C ABI type
pub(super) unsafe fn symbol<T: Copy>(lib: &Library, name: &'static [u8]) -> Result<T, String> {
    unsafe { lib.get::<T>(name) }
        .map(|symbol| *symbol)
        .map_err(|err| {
            format!(
                "missing required symbol {}: {}",
                String::from_utf8_lossy(name).trim_end_matches('\0'),
                system_text(&err)
            )
        })
}

/// A symbol a feature needs and the backend does not: `None` when the library
/// lacks it, so that feature can say why it is missing while everything else
/// still works.
///
/// # Safety
/// The caller must use the symbol's C ABI type.
pub(super) unsafe fn optional_symbol<T: Copy>(lib: &Library, name: &'static [u8]) -> Option<T> {
    unsafe { lib.get::<T>(name) }.ok().map(|symbol| *symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_library_reports_backend_and_candidate() {
        let name = "/sdrtop-nonexistent-test-directory/libmissing.so";
        let err = load("test", &[name], |_| Ok(())).unwrap_err();
        assert!(err.contains("test backend unavailable"));
        assert!(err.contains(name));
    }

    /// **A failed open carries the system's own words**, which name the file
    /// and the reason, not a bare "dlopen failed".
    #[test]
    fn a_failed_open_says_what_the_system_said() {
        let name = "/sdrtop-nonexistent-test-directory/libmissing.so";
        let err = load("test", &[name], |_| Ok(())).unwrap_err();
        assert!(!err.contains("dlopen failed"), "{err}");
        // Once in sdrtop's "Tried:" list, once in the loader's own text.
        assert!(err.matches(name).count() >= 2, "{err}");
    }

    /// **A missing symbol carries the system's own words** too, not a bare
    /// "dlsym failed".
    #[test]
    fn a_missing_symbol_says_what_the_system_said() {
        let fixture = super::super::test_support::TestLibrary::new(false);
        let lib = fixture.library();
        let err = unsafe { symbol::<unsafe extern "C" fn()>(&lib, b"sdrtop_no_such_symbol\0") }
            .unwrap_err();
        assert!(
            err.starts_with("missing required symbol sdrtop_no_such_symbol: "),
            "{err}"
        );
        assert!(!err.contains("dlsym failed"), "{err}");
        assert!(err.matches("sdrtop_no_such_symbol").count() >= 2, "{err}");
    }
}
