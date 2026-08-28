use libc::{c_char, c_int, c_uchar, c_void};

/// `void (*)(unsigned char *buf, uint32_t len, void *ctx)` - the async read sink.
pub type RtlSdrReadAsyncCb = extern "C" fn(*mut c_uchar, u32, *mut c_void);

// librtlsdr's `rtlsdr_dev_t` is opaque; carry it as `*mut c_void`.
extern "C" {
    pub fn rtlsdr_get_device_count() -> u32;
    pub fn rtlsdr_get_device_name(index: u32) -> *const c_char;
    pub fn rtlsdr_get_device_usb_strings(
        index: u32,
        manufact: *mut c_char,
        product: *mut c_char,
        serial: *mut c_char,
    ) -> c_int;

    pub fn rtlsdr_open(dev: *mut *mut c_void, index: u32) -> c_int;
    pub fn rtlsdr_close(dev: *mut c_void) -> c_int;

    pub fn rtlsdr_set_center_freq(dev: *mut c_void, freq: u32) -> c_int;
    pub fn rtlsdr_set_sample_rate(dev: *mut c_void, rate: u32) -> c_int;

    /// tuner type: 0 unknown, 1 E4000, 2 FC0012, 3 FC0013, 4 FC2580, 5 R820T, 6 R828D.
    pub fn rtlsdr_get_tuner_type(dev: *mut c_void) -> c_int;
    /// With a null buffer, returns the number of gains; otherwise fills `gains`
    /// (in tenths of a dB) and returns the count.
    pub fn rtlsdr_get_tuner_gains(dev: *mut c_void, gains: *mut c_int) -> c_int;
    /// `manual`: 1 = manual gain, 0 = tuner AGC.
    pub fn rtlsdr_set_tuner_gain_mode(dev: *mut c_void, manual: c_int) -> c_int;
    /// `gain` in tenths of a dB (e.g. 197 = 19.7 dB), must be one of the table values.
    pub fn rtlsdr_set_tuner_gain(dev: *mut c_void, gain: c_int) -> c_int;

    pub fn rtlsdr_reset_buffer(dev: *mut c_void) -> c_int;
    pub fn rtlsdr_read_async(
        dev: *mut c_void,
        cb: RtlSdrReadAsyncCb,
        ctx: *mut c_void,
        buf_num: u32,
        buf_len: u32,
    ) -> c_int;
    pub fn rtlsdr_cancel_async(dev: *mut c_void) -> c_int;
}
