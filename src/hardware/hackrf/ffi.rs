// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use libc::{c_char, c_int, c_void};

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
    pub usb_device_count: *mut c_int,
    pub usb_devices: *mut *mut c_void,
    pub usb_device_index: *mut c_int,
    pub devicecount: c_int,
}

#[repr(C)]
pub struct ReadPartidSerialno {
    pub part_id: [u32; 2],
    pub serial_no: [u32; 4],
}

pub type HackrfTransferCallback = extern "C" fn(*mut hackrf_transfer) -> c_int;

extern "C" {
    pub fn hackrf_init() -> c_int;
    pub fn hackrf_exit() -> c_int;
    pub fn hackrf_close(device: *mut c_void) -> c_int;
    pub fn hackrf_device_list() -> *mut HackrfDeviceList;
    pub fn hackrf_device_list_free(list: *mut HackrfDeviceList);
    pub fn hackrf_device_list_open(
        list: *mut HackrfDeviceList,
        index: c_int,
        device: *mut *mut c_void,
    ) -> c_int;
    pub fn hackrf_version_string_read(
        device: *mut c_void,
        version: *mut c_char,
        length: u8,
    ) -> c_int;
    pub fn hackrf_is_streaming(device: *mut c_void) -> c_int;
    pub fn hackrf_set_sample_rate(device: *mut c_void, freq_hz: f64) -> c_int;
    pub fn hackrf_set_baseband_filter_bandwidth(device: *mut c_void, bandwidth_hz: u32) -> c_int;
    pub fn hackrf_set_freq(device: *mut c_void, freq_hz: u64) -> c_int;
    pub fn hackrf_set_amp_enable(device: *mut c_void, value: u8) -> c_int;
    pub fn hackrf_start_rx(
        device: *mut c_void,
        callback: HackrfTransferCallback,
        user_param: *mut c_void,
    ) -> c_int;
    pub fn hackrf_stop_rx(device: *mut c_void) -> c_int;
    pub fn hackrf_set_lna_gain(device: *mut c_void, value: u32) -> c_int;
    pub fn hackrf_set_vga_gain(device: *mut c_void, value: u32) -> c_int;
    pub fn hackrf_board_partid_serialno_read(
        device: *mut c_void,
        value: *mut ReadPartidSerialno,
    ) -> c_int;
    pub fn hackrf_board_id_read(device: *mut c_void, value: *mut u8) -> c_int;
    pub fn hackrf_board_id_name(id: u8) -> *const c_char;
    pub fn hackrf_error_name(errcode: c_int) -> *const c_char;
    pub fn hackrf_board_rev_read(device: *mut c_void, value: *mut u8) -> c_int;
    pub fn hackrf_usb_api_version_read(device: *mut c_void, version: *mut u16) -> c_int;
}
