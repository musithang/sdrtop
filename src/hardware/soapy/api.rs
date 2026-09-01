//! The libSoapySDR symbol table, and the one place in the tree that declares it.
//!
//! **Nothing else calls into the library.** Every `unsafe extern "C"` signature
//! for SoapySDR lives here, transcribed from the installed
//! `/usr/include/SoapySDR/*.h`, because there is no linker behind a `dlopen` to
//! catch a mistake. A signature that is wrong by one argument compiles, links,
//! runs, and corrupts the stack at call time, and it will look exactly like a
//! driver bug. One file is one place to audit.
//!
//! The library is opened at runtime rather than linked, so sdrtop still builds
//! and runs on a machine that has never heard of SoapySDR. There, [`api`] simply
//! answers `None` and no Soapy devices exist. See `dev_docs/soapy-design.md`.

use std::ffi::{c_char, CStr};
use std::sync::OnceLock;

/// The ABI this code was written against, from `SOAPY_SDR_ABI_VERSION` in
/// `SoapySDR/Version.h`.
///
/// **Not the library version.** A libSoapySDR 0.8.1 reports its ABI as `0.8`;
/// `0.8.1` is what `SoapySDR_getLibVersion` returns. Upstream's own guidance in
/// that header is a plain string comparison against this constant.
const WANT_ABI: &str = "0.8";

/// `SoapySDRKwargs` from `SoapySDR/Types.h`: a key/value string map.
///
/// ```c
/// typedef struct { size_t size; char **keys; char **vals; } SoapySDRKwargs;
/// ```
#[repr(C)]
pub struct SoapySDRKwargs {
    pub size: usize,
    pub keys: *mut *mut c_char,
    pub vals: *mut *mut c_char,
}

/// Copy one `SoapySDRKwargs` out into owned Rust strings.
///
/// Everything past this function is safe and pure, which is the whole point:
/// the interpretation in [`super::args`] can then be tested on a machine with
/// no library, no driver and no radio.
///
/// A key or value pointer that is null contributes an empty string rather than a
/// panic. These come from a driver nobody here has seen, and enumeration runs
/// before the TUI is even on screen.
///
/// # Safety
/// `k` must point at a live `SoapySDRKwargs` whose `keys` and `vals` each have
/// at least `size` valid entries. That is what SoapySDR's own enumeration
/// returns.
pub unsafe fn kwargs_to_vec(k: *const SoapySDRKwargs) -> Vec<(String, String)> {
    if k.is_null() {
        return Vec::new();
    }
    let k = unsafe { &*k };
    if k.keys.is_null() || k.vals.is_null() {
        return Vec::new();
    }
    (0..k.size)
        .map(|i| unsafe {
            (
                cstr_to_string(*k.keys.add(i)),
                cstr_to_string(*k.vals.add(i)),
            )
        })
        .collect()
}

/// Every SoapySDR symbol sdrtop uses, resolved once.
///
/// Grows one checkpoint at a time. A field is added when something calls it, so
/// there is never a row here whose signature nobody has had a reason to check.
pub struct SoapyApi {
    /// Keeps the library mapped. The function pointers below point into it, so
    /// dropping this would leave them dangling. It is never dropped in practice:
    /// the whole struct lives in a `OnceLock` for the life of the process.
    _lib: libloading::Library,

    get_abi_version: unsafe extern "C" fn() -> *const c_char,
}

impl SoapyApi {
    /// The ABI string the loaded library reports.
    pub fn abi_version(&self) -> String {
        // Safety: the pointer comes from the library we are holding open, and
        // SoapySDR returns a static string here, not an allocation to free.
        unsafe { cstr_to_string((self.get_abi_version)()) }
    }
}

/// The loaded library, or `None` on a machine that does not have it.
///
/// Resolved once and cached, including the failure: a machine without
/// libSoapySDR should not pay for the lookup on every enumeration, and a machine
/// with a broken one should not log the same complaint repeatedly.
pub fn api() -> Option<&'static SoapyApi> {
    static API: OnceLock<Option<SoapyApi>> = OnceLock::new();
    API.get_or_init(load).as_ref()
}

/// Names to try, most specific first.
///
/// The versioned soname first because it is the one that promises the ABI these
/// signatures were written against. The bare `.so` last: it is the development
/// symlink from the `-dev` package and can point at anything, including a build
/// nobody meant to run against.
const CANDIDATES: &[&str] = &["libSoapySDR.so.0.8", "libSoapySDR.so.0", "libSoapySDR.so"];

fn load() -> Option<SoapyApi> {
    for name in CANDIDATES {
        // Safety: opening a shared library runs its initialisers, which is why
        // this is unsafe. libSoapySDR is a well-behaved library and the names
        // above are fixed, not taken from user input.
        let Ok(lib) = (unsafe { libloading::Library::new(name) }) else {
            continue;
        };
        return match resolve(lib) {
            Ok(api) => {
                let found = api.abi_version();
                if abi_ok(&found) {
                    Some(api)
                } else {
                    // Not a degraded experience, a corrupted stack: `setupStream`
                    // returned an `int` and took a `SoapySDRStream**` in 0.7, and
                    // returns the stream pointer in 0.8. Refuse, and say what was
                    // found so the log answers the question on its own.
                    eprintln!(
                        "SoapySDR: {name} reports ABI {found:?}, sdrtop needs {WANT_ABI:?}. \
                         Soapy devices will not be offered."
                    );
                    None
                }
            }
            Err(missing) => {
                eprintln!("SoapySDR: {name} is missing the symbol {missing}. Ignoring it.");
                None
            }
        };
    }
    // No library, which is the common case and not worth a log line. sdrtop
    // behaves exactly as it did before this backend existed.
    None
}

/// Pull every symbol out of an opened library, or name the first one missing.
fn resolve(lib: libloading::Library) -> Result<SoapyApi, &'static str> {
    macro_rules! sym {
        ($name:literal) => {{
            // Safety: the type is transcribed from the installed header. See the
            // module comment on why that is the load-bearing step.
            match unsafe { lib.get(concat!($name, "\0").as_bytes()) } {
                Ok(s) => *s,
                Err(_) => return Err($name),
            }
        }};
    }

    let get_abi_version = sym!("SoapySDR_getABIVersion");

    Ok(SoapyApi {
        _lib: lib,
        get_abi_version,
    })
}

/// Whether a reported ABI string is one these signatures are safe against.
///
/// Exact equality, which is what `SoapySDR/Version.h` prescribes: "if the values
/// are not equal then the client code was compiled against a different ABI than
/// the library". The format is `version[-extra]` where extra marks a development
/// branch, so `0.8-dev` is deliberately refused rather than waved through as
/// close enough.
///
/// Pure, so the refusal path is testable on a machine with no library at all,
/// which is every CI runner.
fn abi_ok(found: &str) -> bool {
    found == WANT_ABI
}

/// A C string as an owned `String`, lossily.
///
/// Lossy on purpose. These come from a driver nobody here has seen, and a panic
/// inside device enumeration would take the program down before the TUI starts.
/// A mangled character in a device label is a much better outcome.
///
/// # Safety
/// `ptr` must be null or a valid NUL-terminated C string.
unsafe fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact equality, and the reasons for each refusal.
    ///
    /// The `0.8.1` case is the one this plan originally got wrong: that is the
    /// *library* version, and the same library's ABI is `0.8`. Accepting it
    /// would mean accepting a string this function should never see, which is a
    /// small thing until the day some other project's `0.8.1` really is a
    /// different ABI.
    #[test]
    fn only_the_abi_we_were_written_against_is_accepted() {
        assert!(abi_ok("0.8"));
        assert!(
            !abi_ok("0.7"),
            "0.7 changed setupStream and would corrupt the stack"
        );
        assert!(!abi_ok("0.9"));
        assert!(
            !abi_ok("0.8-dev"),
            "a development branch is not the release ABI"
        );
        assert!(!abi_ok("0.8.1"), "that is the library version, not the ABI");
        assert!(
            !abi_ok(""),
            "an unreadable version is a refusal, not a default"
        );
    }

    /// The absent-library path, which is CI's path and most users'.
    ///
    /// Nothing here can assert whether the library loads: that depends on the
    /// machine, and pinning it either way would make the suite pass in one place
    /// and fail in another. What must hold everywhere is that asking twice gives
    /// the same answer and neither call panics.
    #[test]
    fn asking_twice_gives_the_same_answer_and_never_panics() {
        let first = api().is_some();
        let second = api().is_some();
        assert_eq!(first, second, "the cache must be stable");
    }

    /// If the library *is* present on this machine, it has to be one we accept,
    /// because `api()` only returns `Some` after the check passed. This is the
    /// half of the loader that a developer machine with SoapySDR installed can
    /// actually exercise.
    #[test]
    fn a_loaded_library_has_an_abi_we_accept() {
        if let Some(api) = api() {
            let v = api.abi_version();
            assert!(abi_ok(&v), "api() handed back a library reporting {v:?}");
        }
    }

    /// Build a `SoapySDRKwargs` by hand so the extraction can be exercised with
    /// no library present. The `CString`s must outlive the call, which is what
    /// the returned vector is for.
    fn fake_kwargs(
        pairs: &[(&str, &str)],
    ) -> (Vec<std::ffi::CString>, Vec<*mut c_char>, Vec<*mut c_char>) {
        let mut owned = Vec::new();
        let mut keys = Vec::new();
        let mut vals = Vec::new();
        for (k, v) in pairs {
            let ck = std::ffi::CString::new(*k).unwrap();
            let cv = std::ffi::CString::new(*v).unwrap();
            keys.push(ck.as_ptr() as *mut c_char);
            vals.push(cv.as_ptr() as *mut c_char);
            owned.push(ck);
            owned.push(cv);
        }
        (owned, keys, vals)
    }

    /// The real shape, taken from `SoapySDRUtil --find` on a HackRF One.
    #[test]
    fn kwargs_come_out_in_order_with_their_values() {
        let pairs = [
            ("driver", "hackrf"),
            ("label", "HackRF One #0 955c64dc2a3d89c3"),
            ("serial", "0000000000000000955c64dc2a3d89c3"),
        ];
        let (_owned, mut keys, mut vals) = fake_kwargs(&pairs);
        let k = SoapySDRKwargs {
            size: pairs.len(),
            keys: keys.as_mut_ptr(),
            vals: vals.as_mut_ptr(),
        };
        let out = unsafe { kwargs_to_vec(&k) };
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], ("driver".into(), "hackrf".into()));
        assert_eq!(out[2].1, "0000000000000000955c64dc2a3d89c3");
    }

    /// A driver that hands back nothing, or a null pointer where a map was
    /// expected, must not take the process down before the TUI starts.
    #[test]
    fn an_empty_or_null_kwargs_is_an_empty_vector() {
        assert!(unsafe { kwargs_to_vec(std::ptr::null()) }.is_empty());
        let empty = SoapySDRKwargs {
            size: 0,
            keys: std::ptr::null_mut(),
            vals: std::ptr::null_mut(),
        };
        assert!(unsafe { kwargs_to_vec(&empty) }.is_empty());
        // A non-zero size with null arrays is a driver bug, and still must not
        // dereference.
        let lying = SoapySDRKwargs {
            size: 4,
            keys: std::ptr::null_mut(),
            vals: std::ptr::null_mut(),
        };
        assert!(unsafe { kwargs_to_vec(&lying) }.is_empty());
    }

    /// A null string is empty rather than a crash. Drivers return null for
    /// fields they do not have.
    #[test]
    fn a_null_c_string_is_empty() {
        assert_eq!(unsafe { cstr_to_string(std::ptr::null()) }, "");
    }
}
