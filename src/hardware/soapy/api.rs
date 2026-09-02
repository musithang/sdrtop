// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

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

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::OnceLock;

/// The ABI this code was written against, from `SOAPY_SDR_ABI_VERSION` in
/// `SoapySDR/Version.h`.
///
/// **Not the library version.** A libSoapySDR 0.8.1 reports its ABI as `0.8`;
/// `0.8.1` is what `SoapySDR_getLibVersion` returns. Upstream's own guidance in
/// that header is a plain string comparison against this constant.
const WANT_ABI: &str = "0.8";

/// The RX direction, `SOAPY_SDR_RX` from `SoapySDR/Constants.h`. TX is 0.
pub const RX: c_int = 1;

/// sdrtop drives channel 0 and only channel 0.
pub const CHAN: usize = 0;

/// Opaque stream handle, `typedef struct SoapySDRStream SoapySDRStream;`.
#[repr(C)]
pub struct SoapySDRStream {
    _private: [u8; 0],
}

/// `SOAPY_SDR_TIMEOUT` from `SoapySDR/Errors.h`: no samples arrived inside the
/// window. **Normal on a quiet bus**, not a failure.
pub const ERR_TIMEOUT: c_int = -1;
/// `SOAPY_SDR_OVERFLOW`: the driver's own buffer filled and samples were lost.
/// This is the drop signal, and it maps onto the counter HackRF fills from its
/// short transfers.
pub const ERR_OVERFLOW: c_int = -4;

/// Opaque device handle, `typedef struct SoapySDRDevice SoapySDRDevice;`.
#[repr(C)]
pub struct SoapySDRDevice {
    _private: [u8; 0],
}

/// `SoapySDRRange` from `SoapySDR/Types.h`.
///
/// **Returned by value** from `getGainRange`, which is worth noticing: three
/// doubles is over the register limit on x86-64, so it comes back through a
/// hidden pointer. Rust's `extern "C"` handles that, but it is the kind of thing
/// that is silently wrong if the struct layout is off by a field.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SoapySDRRange {
    pub minimum: f64,
    pub maximum: f64,
    pub step: f64,
}

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

    last_error: unsafe extern "C" fn() -> *const c_char,
    free: unsafe extern "C" fn(*mut c_void),
    strings_clear: unsafe extern "C" fn(*mut *mut *mut c_char, usize),
    kwargs_list_clear: unsafe extern "C" fn(*mut SoapySDRKwargs, usize),

    enumerate: unsafe extern "C" fn(*const SoapySDRKwargs, *mut usize) -> *mut SoapySDRKwargs,
    make_str_args: unsafe extern "C" fn(*const c_char) -> *mut SoapySDRDevice,
    unmake: unsafe extern "C" fn(*mut SoapySDRDevice) -> c_int,

    get_driver_key: unsafe extern "C" fn(*const SoapySDRDevice) -> *mut c_char,
    get_hardware_key: unsafe extern "C" fn(*const SoapySDRDevice) -> *mut c_char,

    get_frequency_range:
        unsafe extern "C" fn(*const SoapySDRDevice, c_int, usize, *mut usize) -> *mut SoapySDRRange,
    get_sample_rate_range:
        unsafe extern "C" fn(*const SoapySDRDevice, c_int, usize, *mut usize) -> *mut SoapySDRRange,
    get_bandwidth_range:
        unsafe extern "C" fn(*const SoapySDRDevice, c_int, usize, *mut usize) -> *mut SoapySDRRange,
    get_gain_range: unsafe extern "C" fn(*const SoapySDRDevice, c_int, usize) -> SoapySDRRange,
    list_gains:
        unsafe extern "C" fn(*const SoapySDRDevice, c_int, usize, *mut usize) -> *mut *mut c_char,
    has_gain_mode: unsafe extern "C" fn(*const SoapySDRDevice, c_int, usize) -> bool,
    get_native_stream_format:
        unsafe extern "C" fn(*const SoapySDRDevice, c_int, usize, *mut f64) -> *mut c_char,

    set_frequency: unsafe extern "C" fn(
        *mut SoapySDRDevice,
        c_int,
        usize,
        f64,
        *const SoapySDRKwargs,
    ) -> c_int,
    set_sample_rate: unsafe extern "C" fn(*mut SoapySDRDevice, c_int, usize, f64) -> c_int,
    set_bandwidth: unsafe extern "C" fn(*mut SoapySDRDevice, c_int, usize, f64) -> c_int,
    set_gain: unsafe extern "C" fn(*mut SoapySDRDevice, c_int, usize, f64) -> c_int,
    set_gain_mode: unsafe extern "C" fn(*mut SoapySDRDevice, c_int, usize, bool) -> c_int,

    format_to_size: unsafe extern "C" fn(*const c_char) -> usize,
    err_to_str: unsafe extern "C" fn(c_int) -> *const c_char,
    setup_stream: unsafe extern "C" fn(
        *mut SoapySDRDevice,
        c_int,
        *const c_char,
        *const usize,
        usize,
        *const SoapySDRKwargs,
    ) -> *mut SoapySDRStream,
    activate_stream: unsafe extern "C" fn(
        *mut SoapySDRDevice,
        *mut SoapySDRStream,
        c_int,
        std::ffi::c_longlong,
        usize,
    ) -> c_int,
    read_stream: unsafe extern "C" fn(
        *mut SoapySDRDevice,
        *mut SoapySDRStream,
        *const *mut c_void,
        usize,
        *mut c_int,
        *mut std::ffi::c_longlong,
        std::ffi::c_long,
    ) -> c_int,
    deactivate_stream: unsafe extern "C" fn(
        *mut SoapySDRDevice,
        *mut SoapySDRStream,
        c_int,
        std::ffi::c_longlong,
    ) -> c_int,
    close_stream: unsafe extern "C" fn(*mut SoapySDRDevice, *mut SoapySDRStream) -> c_int,
    get_stream_mtu: unsafe extern "C" fn(*const SoapySDRDevice, *mut SoapySDRStream) -> usize,
}

impl SoapyApi {
    /// The ABI string the loaded library reports.
    pub fn abi_version(&self) -> String {
        // Safety: the pointer comes from the library we are holding open, and
        // SoapySDR returns a static string here, not an allocation to free.
        unsafe { cstr_to_string((self.get_abi_version)()) }
    }

    /// The library's own message for the last failed call.
    ///
    /// Every error path in this backend quotes it. A community issue that
    /// arrives with the driver's own words in it is one we can act on; one that
    /// says "it did not work" is not.
    pub fn last_error(&self) -> String {
        unsafe { cstr_to_string((self.last_error)()) }
    }

    /// Every device SoapySDR can see, as key/value maps.
    pub fn enumerate(&self) -> Vec<Vec<(String, String)>> {
        let mut len: usize = 0;
        // Safety: a null argument means "no filter", which is what the C API
        // example passes.
        let list = unsafe { (self.enumerate)(std::ptr::null(), &mut len) };
        if list.is_null() {
            return Vec::new();
        }
        let out = (0..len)
            .map(|i| unsafe { kwargs_to_vec(list.add(i)) })
            .collect();
        // One of four deallocators, and the wrong one here is heap corruption
        // rather than a leak. A kwargs list is not freed with `free` and not
        // with `SoapySDR_free`.
        unsafe { (self.kwargs_list_clear)(list, len) };
        out
    }

    /// Open a device from an argument string, or say why not.
    ///
    /// # Safety
    /// The returned pointer must be handed to [`Self::unmake`] exactly once.
    pub unsafe fn make(&self, args: &str) -> Result<*mut SoapySDRDevice, String> {
        let Ok(c) = std::ffi::CString::new(args) else {
            return Err(format!("device arguments contain a NUL byte: {args:?}"));
        };
        let dev = unsafe { (self.make_str_args)(c.as_ptr()) };
        if dev.is_null() {
            return Err(self.last_error());
        }
        Ok(dev)
    }

    /// # Safety
    /// `dev` must come from [`Self::make`] and must not be used afterwards.
    pub unsafe fn unmake(&self, dev: *mut SoapySDRDevice) {
        unsafe { (self.unmake)(dev) };
    }

    /// # Safety
    /// `dev` must be a live handle for every method below. That is the whole
    /// contract: nothing here validates the pointer, because nothing can.
    pub unsafe fn driver_key(&self, dev: *const SoapySDRDevice) -> String {
        unsafe { self.owned_string((self.get_driver_key)(dev)) }
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn hardware_key(&self, dev: *const SoapySDRDevice) -> String {
        unsafe { self.owned_string((self.get_hardware_key)(dev)) }
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn freq_ranges(&self, dev: *const SoapySDRDevice) -> Vec<(f64, f64)> {
        unsafe { self.range_list(self.get_frequency_range, dev) }
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn rate_ranges(&self, dev: *const SoapySDRDevice) -> Vec<(f64, f64)> {
        unsafe { self.range_list(self.get_sample_rate_range, dev) }
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn bandwidth_ranges(&self, dev: *const SoapySDRDevice) -> Vec<(f64, f64)> {
        unsafe { self.range_list(self.get_bandwidth_range, dev) }
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn gain_range(&self, dev: *const SoapySDRDevice) -> (f64, f64) {
        let r = unsafe { (self.get_gain_range)(dev, RX, CHAN) };
        (r.minimum, r.maximum)
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn gain_elements(&self, dev: *const SoapySDRDevice) -> Vec<String> {
        let mut len: usize = 0;
        let mut list = unsafe { (self.list_gains)(dev, RX, CHAN, &mut len) };
        if list.is_null() {
            return Vec::new();
        }
        let out = (0..len)
            .map(|i| unsafe { cstr_to_string(*list.add(i)) })
            .collect();
        // A string list has its own deallocator, and it takes the address of the
        // pointer so it can null it out.
        unsafe { (self.strings_clear)(&mut list, len) };
        out
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn has_gain_mode(&self, dev: *const SoapySDRDevice) -> bool {
        unsafe { (self.has_gain_mode)(dev, RX, CHAN) }
    }

    /// The native wire format and the full scale that goes with it.
    ///
    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn native_format(&self, dev: *const SoapySDRDevice) -> (String, f64) {
        let mut full_scale: f64 = 0.0;
        let name = unsafe {
            self.owned_string((self.get_native_stream_format)(
                dev,
                RX,
                CHAN,
                &mut full_scale,
            ))
        };
        (name, full_scale)
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn set_frequency(&self, dev: *mut SoapySDRDevice, hz: f64) -> Result<(), String> {
        self.check(unsafe { (self.set_frequency)(dev, RX, CHAN, hz, std::ptr::null()) })
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn set_sample_rate(&self, dev: *mut SoapySDRDevice, hz: f64) -> Result<(), String> {
        self.check(unsafe { (self.set_sample_rate)(dev, RX, CHAN, hz) })
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn set_bandwidth(&self, dev: *mut SoapySDRDevice, hz: f64) -> Result<(), String> {
        self.check(unsafe { (self.set_bandwidth)(dev, RX, CHAN, hz) })
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn set_gain(&self, dev: *mut SoapySDRDevice, db: f64) -> Result<(), String> {
        self.check(unsafe { (self.set_gain)(dev, RX, CHAN, db) })
    }

    /// # Safety
    /// See [`Self::driver_key`].
    pub unsafe fn set_gain_mode(
        &self,
        dev: *mut SoapySDRDevice,
        automatic: bool,
    ) -> Result<(), String> {
        self.check(unsafe { (self.set_gain_mode)(dev, RX, CHAN, automatic) })
    }

    /// Bytes one element of `format` occupies on the wire.
    ///
    /// Asked rather than derived. `readStream` counts elements and everything
    /// downstream counts bytes, and getting that conversion wrong produces a
    /// spectrum that looks plausible and is wrong.
    pub fn format_size(&self, format: &str) -> usize {
        let Ok(c) = std::ffi::CString::new(format) else {
            return 0;
        };
        unsafe { (self.format_to_size)(c.as_ptr()) }
    }

    /// The library's own name for a stream return code.
    pub fn err_to_str(&self, code: c_int) -> String {
        unsafe { cstr_to_string((self.err_to_str)(code)) }
    }

    /// Open an RX stream in `format` on channel 0.
    ///
    /// # Safety
    /// `dev` must be live, and the returned stream must be closed exactly once.
    pub unsafe fn setup_stream(
        &self,
        dev: *mut SoapySDRDevice,
        format: &str,
    ) -> Result<*mut SoapySDRStream, String> {
        let Ok(c) = std::ffi::CString::new(format) else {
            return Err(format!("sample format {format:?} contains a NUL byte"));
        };
        // A null channel list with a count of zero means channel 0, which is
        // what the upstream C example passes and all sdrtop wants.
        let stream = unsafe {
            (self.setup_stream)(dev, RX, c.as_ptr(), std::ptr::null(), 0, std::ptr::null())
        };
        if stream.is_null() {
            return Err(self.last_error());
        }
        Ok(stream)
    }

    /// # Safety
    /// `dev` and `stream` must be live.
    pub unsafe fn activate_stream(
        &self,
        dev: *mut SoapySDRDevice,
        stream: *mut SoapySDRStream,
    ) -> Result<(), String> {
        self.check(unsafe { (self.activate_stream)(dev, stream, 0, 0, 0) })
    }

    /// # Safety
    /// `dev` and `stream` must be live.
    pub unsafe fn deactivate_stream(&self, dev: *mut SoapySDRDevice, stream: *mut SoapySDRStream) {
        unsafe { (self.deactivate_stream)(dev, stream, 0, 0) };
    }

    /// # Safety
    /// `dev` and `stream` must be live, and `stream` must not be used after.
    pub unsafe fn close_stream(&self, dev: *mut SoapySDRDevice, stream: *mut SoapySDRStream) {
        unsafe { (self.close_stream)(dev, stream) };
    }

    /// # Safety
    /// `dev` and `stream` must be live.
    pub unsafe fn stream_mtu(
        &self,
        dev: *const SoapySDRDevice,
        stream: *mut SoapySDRStream,
    ) -> usize {
        unsafe { (self.get_stream_mtu)(dev, stream) }
    }

    /// Read up to `elems` I/Q pairs into `buf`, which must hold that many
    /// elements of the stream's format.
    ///
    /// Returns the driver's raw return code: a count when positive, one of the
    /// `SOAPY_SDR_*` codes when negative. Classifying it is
    /// [`super::stream::outcome`]'s job, which keeps that decision testable.
    ///
    /// # Safety
    /// `dev` and `stream` must be live, and `buf` must be large enough for
    /// `elems` elements of the format the stream was set up with.
    pub unsafe fn read_stream(
        &self,
        dev: *mut SoapySDRDevice,
        stream: *mut SoapySDRStream,
        buf: *mut c_void,
        elems: usize,
        timeout_us: i64,
    ) -> c_int {
        let buffs: [*mut c_void; 1] = [buf];
        let mut flags: c_int = 0;
        let mut time_ns: std::ffi::c_longlong = 0;
        unsafe {
            (self.read_stream)(
                dev,
                stream,
                buffs.as_ptr(),
                elems,
                &mut flags,
                &mut time_ns,
                timeout_us as std::ffi::c_long,
            )
        }
    }

    /// Zero is success everywhere in this API; anything else is an error whose
    /// text the library is holding.
    fn check(&self, code: c_int) -> Result<(), String> {
        if code == 0 {
            Ok(())
        } else {
            Err(self.last_error())
        }
    }

    /// Read a `SoapySDRRange` array out and free it.
    ///
    /// **These are freed with libc `free`**, not `SoapySDR_free` and not a
    /// `_clear` helper. That is the trap in this API: it is the one returned
    /// allocation whose deallocator is not a SoapySDR function, and it sits
    /// right next to three that are.
    ///
    /// # Safety
    /// See [`Self::driver_key`].
    unsafe fn range_list(
        &self,
        f: unsafe extern "C" fn(
            *const SoapySDRDevice,
            c_int,
            usize,
            *mut usize,
        ) -> *mut SoapySDRRange,
        dev: *const SoapySDRDevice,
    ) -> Vec<(f64, f64)> {
        let mut len: usize = 0;
        let list = unsafe { f(dev, RX, CHAN, &mut len) };
        if list.is_null() {
            return Vec::new();
        }
        let out = (0..len)
            .map(|i| {
                let r = unsafe { *list.add(i) };
                (r.minimum, r.maximum)
            })
            .collect();
        unsafe { libc::free(list as *mut c_void) };
        out
    }

    /// A `char*` the library allocated for us: copy it, then hand it back.
    ///
    /// # Safety
    /// `ptr` must be null or an allocation from a SoapySDR call that documents
    /// `SoapySDR_free` as its deallocator.
    unsafe fn owned_string(&self, ptr: *mut c_char) -> String {
        if ptr.is_null() {
            return String::new();
        }
        let out = unsafe { cstr_to_string(ptr) };
        unsafe { (self.free)(ptr as *mut c_void) };
        out
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

    let api = SoapyApi {
        get_abi_version: sym!("SoapySDR_getABIVersion"),
        last_error: sym!("SoapySDRDevice_lastError"),
        free: sym!("SoapySDR_free"),
        strings_clear: sym!("SoapySDRStrings_clear"),
        kwargs_list_clear: sym!("SoapySDRKwargsList_clear"),
        enumerate: sym!("SoapySDRDevice_enumerate"),
        make_str_args: sym!("SoapySDRDevice_makeStrArgs"),
        unmake: sym!("SoapySDRDevice_unmake"),
        get_driver_key: sym!("SoapySDRDevice_getDriverKey"),
        get_hardware_key: sym!("SoapySDRDevice_getHardwareKey"),
        get_frequency_range: sym!("SoapySDRDevice_getFrequencyRange"),
        get_sample_rate_range: sym!("SoapySDRDevice_getSampleRateRange"),
        get_bandwidth_range: sym!("SoapySDRDevice_getBandwidthRange"),
        get_gain_range: sym!("SoapySDRDevice_getGainRange"),
        list_gains: sym!("SoapySDRDevice_listGains"),
        has_gain_mode: sym!("SoapySDRDevice_hasGainMode"),
        get_native_stream_format: sym!("SoapySDRDevice_getNativeStreamFormat"),
        set_frequency: sym!("SoapySDRDevice_setFrequency"),
        set_sample_rate: sym!("SoapySDRDevice_setSampleRate"),
        set_bandwidth: sym!("SoapySDRDevice_setBandwidth"),
        set_gain: sym!("SoapySDRDevice_setGain"),
        set_gain_mode: sym!("SoapySDRDevice_setGainMode"),
        format_to_size: sym!("SoapySDR_formatToSize"),
        err_to_str: sym!("SoapySDR_errToStr"),
        setup_stream: sym!("SoapySDRDevice_setupStream"),
        activate_stream: sym!("SoapySDRDevice_activateStream"),
        read_stream: sym!("SoapySDRDevice_readStream"),
        deactivate_stream: sym!("SoapySDRDevice_deactivateStream"),
        close_stream: sym!("SoapySDRDevice_closeStream"),
        get_stream_mtu: sym!("SoapySDRDevice_getStreamMTU"),
        // Last, so every `sym!` above has already borrowed it.
        _lib: lib,
    };
    Ok(api)
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
