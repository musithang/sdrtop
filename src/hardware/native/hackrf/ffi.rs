// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use libc::{c_char, c_int, c_void};
use std::sync::OnceLock;

use super::super::loader::{load, symbol};

#[repr(C)]
pub struct hackrf_transfer {
    pub device: *mut c_void,
    pub buffer: *mut u8,
    pub buffer_length: i32,
    pub valid_length: i32,
    pub rx_ctx: *mut c_void,
    pub tx_ctx: *mut c_void,
}

// Matches hackrf_device_list_t in hackrf.h exactly
#[repr(C)]
pub struct HackrfDeviceList {
    pub serial_numbers: *mut *mut c_char,
    pub usb_board_ids: *mut c_int,
    pub usb_device_index: *mut c_int,
    pub devicecount: c_int,
    pub usb_devices: *mut *mut c_void,
    pub usb_devicecount: c_int,
}

#[repr(C)]
pub struct ReadPartidSerialno {
    pub part_id: [u32; 2],
    pub serial_no: [u32; 4],
}

pub type HackrfTransferCallback = extern "C" fn(*mut hackrf_transfer) -> c_int;

pub struct HackrfApi {
    // The static API must keep the library mapped through device and callback cleanup
    _lib: libloading::Library,
    pub hackrf_init: unsafe extern "C" fn() -> c_int,
    pub hackrf_exit: unsafe extern "C" fn() -> c_int,
    pub hackrf_close: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub hackrf_device_list: unsafe extern "C" fn() -> *mut HackrfDeviceList,
    pub hackrf_device_list_free: unsafe extern "C" fn(*mut HackrfDeviceList),
    pub hackrf_device_list_open:
        unsafe extern "C" fn(*mut HackrfDeviceList, c_int, *mut *mut c_void) -> c_int,
    pub hackrf_version_string_read: unsafe extern "C" fn(*mut c_void, *mut c_char, u8) -> c_int,
    pub hackrf_is_streaming: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub hackrf_set_sample_rate: unsafe extern "C" fn(*mut c_void, f64) -> c_int,
    pub hackrf_set_baseband_filter_bandwidth: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    pub hackrf_set_freq: unsafe extern "C" fn(*mut c_void, u64) -> c_int,
    pub hackrf_set_amp_enable: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    pub hackrf_start_rx:
        unsafe extern "C" fn(*mut c_void, HackrfTransferCallback, *mut c_void) -> c_int,
    pub hackrf_stop_rx: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub hackrf_set_lna_gain: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    pub hackrf_set_vga_gain: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    pub hackrf_board_partid_serialno_read:
        unsafe extern "C" fn(*mut c_void, *mut ReadPartidSerialno) -> c_int,
    pub hackrf_board_id_read: unsafe extern "C" fn(*mut c_void, *mut u8) -> c_int,
    pub hackrf_board_id_name: unsafe extern "C" fn(c_int) -> *const c_char,
    pub hackrf_error_name: unsafe extern "C" fn(c_int) -> *const c_char,
    pub hackrf_board_rev_read: unsafe extern "C" fn(*mut c_void, *mut u8) -> c_int,
    pub hackrf_usb_api_version_read: unsafe extern "C" fn(*mut c_void, *mut u16) -> c_int,
}

const CANDIDATES: &[&str] = &["libhackrf.so.0", "libhackrf.so"];

pub fn api() -> anyhow::Result<&'static HackrfApi> {
    static API: OnceLock<Result<HackrfApi, String>> = OnceLock::new();
    API.get_or_init(|| load("libhackrf", CANDIDATES, resolve))
        .as_ref()
        .map_err(|err| {
            anyhow::anyhow!(
                "{err}\nHackRF needs libhackrf 2023.01.1 or newer. \
                 Install libhackrf0 on Debian/Ubuntu or hackrf on Arch/Fedora."
            )
        })
}

pub(super) fn resolve(lib: libloading::Library) -> Result<HackrfApi, String> {
    macro_rules! sym {
        ($name:ident) => {
            unsafe { symbol(&lib, concat!(stringify!($name), "\0").as_bytes())? }
        };
    }
    Ok(HackrfApi {
        hackrf_init: sym!(hackrf_init),
        hackrf_exit: sym!(hackrf_exit),
        hackrf_close: sym!(hackrf_close),
        hackrf_device_list: sym!(hackrf_device_list),
        hackrf_device_list_free: sym!(hackrf_device_list_free),
        hackrf_device_list_open: sym!(hackrf_device_list_open),
        hackrf_version_string_read: sym!(hackrf_version_string_read),
        hackrf_is_streaming: sym!(hackrf_is_streaming),
        hackrf_set_sample_rate: sym!(hackrf_set_sample_rate),
        hackrf_set_baseband_filter_bandwidth: sym!(hackrf_set_baseband_filter_bandwidth),
        hackrf_set_freq: sym!(hackrf_set_freq),
        hackrf_set_amp_enable: sym!(hackrf_set_amp_enable),
        hackrf_start_rx: sym!(hackrf_start_rx),
        hackrf_stop_rx: sym!(hackrf_stop_rx),
        hackrf_set_lna_gain: sym!(hackrf_set_lna_gain),
        hackrf_set_vga_gain: sym!(hackrf_set_vga_gain),
        hackrf_board_partid_serialno_read: sym!(hackrf_board_partid_serialno_read),
        hackrf_board_id_read: sym!(hackrf_board_id_read),
        hackrf_board_id_name: sym!(hackrf_board_id_name),
        hackrf_error_name: sym!(hackrf_error_name),
        hackrf_board_rev_read: sym!(hackrf_board_rev_read),
        hackrf_usb_api_version_read: sym!(hackrf_usb_api_version_read),
        _lib: lib,
    })
}
