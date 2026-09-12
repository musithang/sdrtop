// SPDX-License-Identifier: GPL-3.0-or-later

use libloading::Library;

pub(super) fn load<T>(
    backend: &str,
    candidates: &[&str],
    resolve: fn(Library) -> Result<T, String>,
) -> Result<T, String> {
    let mut errors = Vec::new();
    for name in candidates {
        // The names come from the backend's fixed library list
        let result = unsafe { Library::new(name) }
            .map_err(|err| err.to_string())
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

/// The caller must use the symbol's C ABI type
pub(super) unsafe fn symbol<T: Copy>(lib: &Library, name: &'static [u8]) -> Result<T, String> {
    unsafe { lib.get::<T>(name) }
        .map(|symbol| *symbol)
        .map_err(|err| {
            format!(
                "missing required symbol {}: {err}",
                String::from_utf8_lossy(name).trim_end_matches('\0')
            )
        })
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
}
