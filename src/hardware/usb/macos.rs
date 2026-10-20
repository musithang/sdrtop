// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The USB scan on macOS: IOKit's registry lists every `IOUSBHostDevice`,
//! held by another app or not, and each one's properties are read into an
//! [`IoUsbProps`]. What they mean is decided in [`super::from_iokit`], plain
//! code tested on every host; this file only asks.
//!
//! The holder is the process IOKit names in `UsbExclusiveOwner`, on the
//! device or on one of its interfaces; what that process is doing is read
//! from libproc and the `KERN_PROCARGS2` sysctl, as far as macOS lets one
//! user read another's process.
//!
//! IOKit and CoreFoundation are system frameworks on every Mac, so they are
//! linked rather than loaded, and the few calls used are declared here by
//! hand rather than through a crate.

use std::ffi::{c_char, c_void, CStr, CString};

use super::{from_iokit, parse_procargs2, Holder, IoUsbProps, OwnerInfo, UsbDeviceInfo, Wanted};

type KernReturn = i32;
type MachPort = u32;
type IoObject = MachPort;
type CfTypeRef = *const c_void;
type CfIndex = isize;
type CfTypeId = usize;

const KERN_SUCCESS: KernReturn = 0;
/// `kIOMainPortDefault`: the default port, spelled as the null port.
const MAIN_PORT_DEFAULT: MachPort = 0;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const CF_NUMBER_SINT64_TYPE: CfIndex = 4;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(
        main_port: MachPort,
        matching: *mut c_void,
        existing: *mut IoObject,
    ) -> KernReturn;
    fn IOIteratorNext(iterator: IoObject) -> IoObject;
    fn IORegistryEntryGetChildIterator(
        entry: IoObject,
        plane: *const c_char,
        iterator: *mut IoObject,
    ) -> KernReturn;
    fn IOObjectRelease(object: IoObject) -> KernReturn;
    fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: CfTypeRef,
        allocator: CfTypeRef,
        options: u32,
    ) -> CfTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CfTypeRef);
    fn CFGetTypeID(cf: CfTypeRef) -> CfTypeId;
    fn CFStringGetTypeID() -> CfTypeId;
    fn CFNumberGetTypeID() -> CfTypeId;
    fn CFStringCreateWithCString(
        allocator: CfTypeRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CfTypeRef;
    fn CFStringGetLength(string: CfTypeRef) -> CfIndex;
    fn CFStringGetMaximumSizeForEncoding(length: CfIndex, encoding: u32) -> CfIndex;
    fn CFStringGetCString(
        string: CfTypeRef,
        buffer: *mut c_char,
        size: CfIndex,
        encoding: u32,
    ) -> u8;
    fn CFNumberGetValue(number: CfTypeRef, kind: CfIndex, value: *mut c_void) -> u8;
}

/// `mach_timebase_info`: how many nanoseconds a mach tick is, as
/// `numer / denom`. Declared here because libc marks its own binding
/// deprecated in favour of a crate this needs nothing else from.
#[repr(C)]
struct MachTimebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_timebase_info(info: *mut MachTimebase) -> KernReturn;
}

/// A CoreFoundation object this code created and releases.
///
/// Made only through [`Cf::created`], which turns a null result into
/// `None`: CoreFoundation stops the whole process on `CFRelease(NULL)`, and
/// a property the device does not have comes back null.
struct Cf(CfTypeRef);

impl Cf {
    fn created(object: CfTypeRef) -> Option<Cf> {
        (!object.is_null()).then(|| Cf(object))
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: built only from a non-null Create-rule result, released once.
        unsafe { CFRelease(self.0) }
    }
}

/// An IOKit object this code was handed and releases.
struct Io(IoObject);

impl Drop for Io {
    fn drop(&mut self) {
        // SAFETY: built only from a non-zero object IOKit returned to us.
        unsafe { IOObjectRelease(self.0) };
    }
}

fn cf_string(text: &str) -> Option<Cf> {
    let c = CString::new(text).ok()?;
    // SAFETY: `c` is a valid NUL-terminated string for the length of the call.
    let s =
        unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), CF_STRING_ENCODING_UTF8) };
    Cf::created(s)
}

/// One registry property of `entry`, or `None` when it has no such key.
fn property(entry: &Io, key: &str) -> Option<Cf> {
    let key = cf_string(key)?;
    // SAFETY: `entry` is a live registry entry and `key` a live CFString.
    let value = unsafe { IORegistryEntryCreateCFProperty(entry.0, key.0, std::ptr::null(), 0) };
    Cf::created(value)
}

fn number(entry: &Io, key: &str) -> Option<i64> {
    let value = property(entry, key)?;
    // SAFETY: the type is checked before the value is read as a number, and
    // `out` is an i64 as kCFNumberSInt64Type asks for.
    unsafe {
        if CFGetTypeID(value.0) != CFNumberGetTypeID() {
            return None;
        }
        let mut out: i64 = 0;
        let ok = CFNumberGetValue(value.0, CF_NUMBER_SINT64_TYPE, (&raw mut out).cast());
        (ok != 0).then_some(out)
    }
}

fn string(entry: &Io, key: &str) -> Option<String> {
    let value = property(entry, key)?;
    // SAFETY: the type is checked before the value is read as a string, and
    // the buffer is the size CoreFoundation says the UTF-8 form can need.
    unsafe {
        if CFGetTypeID(value.0) != CFStringGetTypeID() {
            return None;
        }
        let size =
            CFStringGetMaximumSizeForEncoding(CFStringGetLength(value.0), CF_STRING_ENCODING_UTF8)
                + 1;
        let mut buffer = vec![0 as c_char; usize::try_from(size).ok()?];
        if CFStringGetCString(value.0, buffer.as_mut_ptr(), size, CF_STRING_ENCODING_UTF8) == 0 {
            return None;
        }
        let text = CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .trim()
            .to_string();
        (!text.is_empty()).then_some(text)
    }
}

fn props(entry: &Io) -> IoUsbProps {
    let narrow = |key: &str| number(entry, key).and_then(|n| u32::try_from(n).ok());
    IoUsbProps {
        vid: number(entry, "idVendor").and_then(|n| u16::try_from(n).ok()),
        pid: number(entry, "idProduct").and_then(|n| u16::try_from(n).ok()),
        product: string(entry, "USB Product Name"),
        vendor: string(entry, "USB Vendor Name"),
        serial: string(entry, "USB Serial Number"),
        location_id: narrow("locationID"),
        address: narrow("USB Address"),
        speed: narrow("Device Speed"),
        exclusive_owner: exclusive_owner(entry),
    }
}

/// `UsbExclusiveOwner` from the device itself, or else from the first of
/// its children (its interfaces) that has one. Each entry is asked for its
/// own property, never through a search of its parents, so a device's
/// owner is not read back as an interface's.
fn exclusive_owner(device: &Io) -> Option<String> {
    const KEY: &str = "UsbExclusiveOwner";
    if let Some(owner) = string(device, KEY) {
        return Some(owner);
    }
    let mut children: IoObject = 0;
    // SAFETY: `device` is a live registry entry, the plane name a static C
    // string, and `children` is released by `Io` when it was handed over.
    let kr =
        unsafe { IORegistryEntryGetChildIterator(device.0, c"IOService".as_ptr(), &mut children) };
    if kr != KERN_SUCCESS || children == 0 {
        return None;
    }
    let children = Io(children);
    loop {
        // SAFETY: `children` is the live iterator IOKit returned above.
        let next = unsafe { IOIteratorNext(children.0) };
        if next == 0 {
            return None;
        }
        if let Some(owner) = string(&Io(next), KEY) {
            return Some(owner);
        }
    }
}

/// What macOS lets this user read about the process IOKit named: its
/// memory, CPU time and age from libproc, its command line from the
/// `KERN_PROCARGS2` sysctl. Another user's process keeps them to itself,
/// and they stay missing; the name IOKit gave is shown either way.
pub fn owner(holder: &Holder) -> OwnerInfo {
    let task = task_info(holder.pid);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs());
    OwnerInfo {
        pid: holder.pid,
        name: holder.name.clone(),
        cmdline: command_line(holder.pid),
        cpu_s: task.as_ref().and_then(|t| {
            let ticks = t.ptinfo.pti_total_user + t.ptinfo.pti_total_system;
            Some(ticks as f64 * nanoseconds_per_tick()? / 1e9)
        }),
        rss_mb: task
            .as_ref()
            .map(|t| t.ptinfo.pti_resident_size / (1024 * 1024)),
        running_secs: task
            .as_ref()
            .zip(now)
            .map(|(t, now)| now.saturating_sub(t.pbsd.pbi_start_tvsec)),
    }
}

/// libproc's account of `pid`, whole or not at all.
fn task_info(pid: u32) -> Option<libc::proc_taskallinfo> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_taskallinfo>()).ok()?;
    // SAFETY: the call fills at most `size` bytes of a zeroed struct of that
    // size, and reports how many it wrote.
    unsafe {
        let mut info: libc::proc_taskallinfo = std::mem::zeroed();
        let got = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKALLINFO,
            0,
            (&raw mut info).cast(),
            size,
        );
        (got == size).then_some(info)
    }
}

/// libproc's CPU times are in mach ticks: nanoseconds on Intel, about 41.7
/// ns each on Apple Silicon.
fn nanoseconds_per_tick() -> Option<f64> {
    let mut base = MachTimebase { numer: 0, denom: 0 };
    // SAFETY: fills the two fields of `base` and nothing else.
    let kr = unsafe { mach_timebase_info(&raw mut base) };
    (kr == KERN_SUCCESS && base.denom != 0).then(|| f64::from(base.numer) / f64::from(base.denom))
}

/// The process's arguments, as `KERN_PROCARGS2` lists them.
fn command_line(pid: u32) -> Option<String> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: each sysctl writes at most the size it is given into a buffer
    // of that size, and says how much it wrote.
    unsafe {
        let mut argmax: libc::c_int = 0;
        let mut size = std::mem::size_of::<libc::c_int>();
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        if libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut argmax).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        let mut buffer = vec![0u8; usize::try_from(argmax).ok()?];
        let mut size = buffer.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        if libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        parse_procargs2(&buffer[..size.min(buffer.len())])
    }
}

/// The radio as IOKit lists it. An error is IOKit refusing the question,
/// which is not the same answer as the radio not being there.
pub fn find(wanted: &Wanted) -> Result<Option<UsbDeviceInfo>, String> {
    let class = c"IOUSBHostDevice";
    // SAFETY: `class` is a static C string. The matching dictionary is
    // consumed by IOServiceGetMatchingServices whether or not it succeeds,
    // so it is not released here.
    let iterator = unsafe {
        let matching = IOServiceMatching(class.as_ptr());
        if matching.is_null() {
            return Err("IOKit could not build a match for IOUSBHostDevice".into());
        }
        let mut iterator: IoObject = 0;
        let kr = IOServiceGetMatchingServices(MAIN_PORT_DEFAULT, matching, &mut iterator);
        if kr != KERN_SUCCESS {
            return Err(format!(
                "IOKit could not list USB devices (kern_return {kr:#x})"
            ));
        }
        Io(iterator)
    };
    loop {
        // SAFETY: `iterator` is the live iterator IOKit returned above.
        let next = unsafe { IOIteratorNext(iterator.0) };
        if next == 0 {
            return Ok(None);
        }
        let entry = Io(next);
        if let Some(info) = from_iokit(&props(&entry), wanted) {
            return Ok(Some(info));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegistryGetRootEntry(main_port: MachPort) -> IoObject;
    }

    fn own_cpu_s() -> f64 {
        // SAFETY: fills a zeroed struct of the right type.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
        let seconds = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
        seconds(usage.ru_utime) + seconds(usage.ru_stime)
    }

    /// This test's own process, read the way a holder is: the name IOKit
    /// would give is kept, every reading is there for a process of our own
    /// user, and the CPU time is in seconds. Read as raw mach ticks it would
    /// come out about 42 times too small on Apple Silicon, so half a second
    /// of work is done first and the reading must land between the
    /// `getrusage` accounts taken either side of it.
    #[test]
    fn our_own_process_reads_as_a_holder_would() {
        let busy = std::time::Instant::now();
        let mut x = 0u64;
        while busy.elapsed().as_millis() < 500 {
            x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
        }
        let before = own_cpu_s();
        let me = owner(&Holder {
            pid: std::process::id(),
            name: "sdrtop".into(),
        });
        let after = own_cpu_s();
        assert_eq!(me.name, "sdrtop");
        assert!(
            me.cmdline.as_deref().is_some_and(|c| c.contains("sdrtop")),
            "{me:?}"
        );
        assert!(me.rss_mb.is_some_and(|mb| mb > 0), "{me:?}");
        assert!(me.running_secs.is_some_and(|s| s < 3600), "{me:?}");
        assert!(
            me.cpu_s
                .is_some_and(|s| s >= before * 0.9 - 0.05 && s <= after * 1.1 + 0.05),
            "{me:?}, getrusage {before}..{after}"
        );

        let gone = owner(&Holder {
            pid: i32::MAX as u32,
            name: "gone".into(),
        });
        assert_eq!((gone.cmdline, gone.cpu_s, gone.rss_mb), (None, None, None));
    }

    /// **A key the entry does not have is `None`, and nothing else.** IOKit
    /// answers it with null, and releasing that null stops the process; a
    /// USB device with no serial number is the everyday case.
    #[test]
    fn a_property_the_entry_does_not_have_is_none() {
        // SAFETY: the root entry always exists; it is released by `Io`.
        let root = Io(unsafe { IORegistryGetRootEntry(MAIN_PORT_DEFAULT) });
        assert_ne!(root.0, 0, "the registry has a root");
        assert!(property(&root, "sdrtop: no such key").is_none());
        assert_eq!(number(&root, "sdrtop: no such key"), None);
        assert_eq!(string(&root, "sdrtop: no such key"), None);
    }
}
