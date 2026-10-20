// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The USB scan on macOS: IOKit's registry lists every `IOUSBHostDevice`,
//! held by another app or not, and each one's properties are read into an
//! [`IoUsbProps`]. What they mean is decided in [`super::from_iokit`], plain
//! code tested on every host; this file only asks.
//!
//! IOKit and CoreFoundation are system frameworks on every Mac, so they are
//! linked rather than loaded, and the few calls used are declared here by
//! hand rather than through a crate.

use std::ffi::{c_char, c_void, CStr, CString};

use super::{from_iokit, IoUsbProps, UsbDeviceInfo, Wanted};

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
