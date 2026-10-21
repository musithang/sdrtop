// SPDX-License-Identifier: GPL-3.0-or-later

#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdatomic.h>

// This layout follows hackrf_device_list_t in libhackrf 2024.02.1
struct hackrf_device_list {
    char **serial_numbers;
    int *usb_board_ids;
    int *usb_device_index;
    int devicecount;
    void **usb_devices;
    int usb_devicecount;
};

static int mode;
static _Atomic int calls[5];
static char *serials[] = {"0000000000000000123456789abcdef0"};

void fixture_mode(int value) { mode = value; }
int fixture_calls(int slot) { return calls[slot]; }
size_t fixture_list_layout(int field) {
    switch (field) {
        case 0: return sizeof(struct hackrf_device_list);
        case 1: return offsetof(struct hackrf_device_list, usb_device_index);
        case 2: return offsetof(struct hackrf_device_list, devicecount);
        case 3: return offsetof(struct hackrf_device_list, usb_devices);
        case 4: return offsetof(struct hackrf_device_list, usb_devicecount);
        default: abort();
    }
}

int hackrf_init(void) {
    calls[0]++;
    return mode == 1 ? -1000 : 0;
}
int hackrf_exit(void) { calls[1]++; return 0; }
struct hackrf_device_list *hackrf_device_list(void) {
    if (mode == 2) return NULL;
    struct hackrf_device_list *list = calloc(1, sizeof(*list));
    list->serial_numbers = serials;
    list->devicecount = mode == 3 ? 0 : 1;
    // Unrelated USB devices must not contribute to HackRF enumeration
    list->usb_devicecount = 9;
    return list;
}
void hackrf_device_list_free(struct hackrf_device_list *list) {
    calls[2]++;
    free(list);
}
int hackrf_device_list_open(struct hackrf_device_list *list, int index, void **device) {
    if (mode == 4) return -6;
    *device = mode == 5 ? NULL : malloc(1);
    return 0;
}
int hackrf_close(void *device) { calls[3]++; free(device); return 0; }
const char *hackrf_error_name(int code) { return "fixture error"; }
const char *hackrf_board_id_name(int id) { return "Fixture HackRF"; }

#define OPTIONAL(name, args) int name args { return -1005; }
OPTIONAL(hackrf_version_string_read, (void *device, char *version, uint8_t length))
OPTIONAL(hackrf_board_partid_serialno_read, (void *device, void *value))
OPTIONAL(hackrf_board_id_read, (void *device, uint8_t *value))
OPTIONAL(hackrf_board_rev_read, (void *device, uint8_t *value))
#ifndef OMIT_LAST_SYMBOL
OPTIONAL(hackrf_usb_api_version_read, (void *device, uint16_t *value))
#endif

#define CONTROL(name, args) int name args { return mode == 6 ? -2 : 0; }
CONTROL(hackrf_set_sample_rate, (void *device, double rate))
CONTROL(hackrf_set_baseband_filter_bandwidth, (void *device, uint32_t bandwidth))
CONTROL(hackrf_set_freq, (void *device, uint64_t freq))
CONTROL(hackrf_set_amp_enable, (void *device, uint8_t value))
CONTROL(hackrf_set_lna_gain, (void *device, uint32_t value))
CONTROL(hackrf_set_vga_gain, (void *device, uint32_t value))
CONTROL(hackrf_start_rx, (void *device, int (*callback)(void *), void *ctx))
int hackrf_is_streaming(void *device) { return 0; }

// hackrf_m0_state in hackrf.h, field for field.
struct hackrf_m0_state {
    uint16_t requested_mode, request_flag;
    uint32_t active_mode, m0_count, m4_count, num_shortfalls, longest_shortfall,
        shortfall_limit, threshold, next_mode, error;
};
#ifndef OMIT_M0_STATE
int hackrf_get_m0_state(void *device, struct hackrf_m0_state *state) {
    if (mode == 7) return -1000;
    state->num_shortfalls = 3;
    state->longest_shortfall = 4096;
    return 0;
}
#endif
size_t hackrf_get_transfer_buffer_size(void *device) { return 262144; }
uint32_t hackrf_get_transfer_queue_depth(void *device) { return 4; }
int hackrf_stop_rx(void *device) { calls[4]++; return 0; }

uint32_t rtlsdr_get_device_count(void) { return 1; }
const char *rtlsdr_get_device_name(uint32_t index) { return "Fixture RTL-SDR"; }
int rtlsdr_get_device_usb_strings(uint32_t index, char *manufacturer, char *product, char *serial) {
    calls[0]++;
    strcpy(manufacturer, "Fixture");
    strcpy(product, "RTL-SDR");
    strcpy(serial, "00000001");
    return 0;
}
int rtlsdr_open(void **device, uint32_t index) {
    if (mode == 4) return -6;
    *device = mode == 5 ? NULL : malloc(1);
    return 0;
}
int rtlsdr_close(void *device) { calls[3]++; free(device); return 0; }
uint32_t rtlsdr_get_sample_rate(void *device) { return 2399999; }
int rtlsdr_get_tuner_type(void *device) { return 5; }
int rtlsdr_get_tuner_gains(void *device, int *gains) {
    if (gains) { gains[0] = 0; gains[1] = 197; gains[2] = 496; }
    return 3;
}
CONTROL(rtlsdr_set_center_freq, (void *device, uint32_t freq))
CONTROL(rtlsdr_set_sample_rate, (void *device, uint32_t rate))
CONTROL(rtlsdr_set_tuner_gain_mode, (void *device, int manual))
CONTROL(rtlsdr_set_tuner_gain, (void *device, int gain))
CONTROL(rtlsdr_reset_buffer, (void *device))
CONTROL(rtlsdr_read_async, (void *device, void (*callback)(unsigned char *, uint32_t, void *), void *ctx, uint32_t num, uint32_t len))
#ifndef OMIT_LAST_SYMBOL
int rtlsdr_cancel_async(void *device) { calls[4]++; return 0; }
#endif

// libairspy 1.0.10. Modes 20 and up are this library's: 20 lists Airspy Mini
// rates, 21 refuses to open (AIRSPY_ERROR_BUSY), 22 finds no device, 23 fails
// every setting (AIRSPY_ERROR_LIBUSB).
int airspy_list_devices(uint64_t *serials, int count) {
    if (mode == 22) return 0;
    if (serials && count >= 1) serials[0] = 0x637862DC2E1182D7ULL;
    return 1;
}
int airspy_open_sn(void **device, uint64_t serial) {
    if (mode == 21) return -6;
    *device = malloc(1);
    return 0;
}
int airspy_close(void *device) { calls[3]++; free(device); return 0; }
// What the backend asked libairspy for last, by field (see fixture_airspy_state):
// 0 sample type, 1 bias tee, 2 LNA AGC, 3 mixer AGC, 4 LNA, 5 mixer, 6 VGA,
// 7 frequency in kHz, 8 sample rate argument. -1 until set.
static int airspy_state[9] = {-1, -1, -1, -1, -1, -1, -1, -1, -1};
int fixture_airspy_state(int field) { return airspy_state[field]; }
typedef struct { uint32_t part_id[2]; uint32_t serial_no[4]; } airspy_read_partid_serialno_t;
typedef struct { uint32_t major_version; uint32_t minor_version; uint32_t revision; } airspy_lib_version_t;
size_t fixture_airspy_layout(int field) {
    switch (field) {
        case 0: return sizeof(airspy_read_partid_serialno_t);
        case 1: return offsetof(airspy_read_partid_serialno_t, serial_no);
        case 2: return sizeof(airspy_lib_version_t);
        default: abort();
    }
}
void airspy_lib_version(airspy_lib_version_t *v) {
    v->major_version = 1; v->minor_version = 0; v->revision = 10;
}
int airspy_board_partid_serialno_read(void *device, airspy_read_partid_serialno_t *p) {
    p->part_id[0] = 0x6906002B; p->part_id[1] = 0x00000030;
    p->serial_no[0] = 0; p->serial_no[1] = 0;
    p->serial_no[2] = 0x637862DC; p->serial_no[3] = 0x2E1182D7;
    return 0;
}
const char *airspy_error_name(int code) { return code == -6 ? "AIRSPY_ERROR_BUSY" : "AIRSPY_ERROR_OTHER"; }
#define AIRSPY_SET(name, field, type) \
    int name(void *device, type value) { \
        if (mode == 23) return -1000; \
        airspy_state[field] = (int)value; return 0; }
AIRSPY_SET(airspy_set_sample_type, 0, int)
AIRSPY_SET(airspy_set_rf_bias, 1, uint8_t)
AIRSPY_SET(airspy_set_lna_agc, 2, uint8_t)
AIRSPY_SET(airspy_set_mixer_agc, 3, uint8_t)
AIRSPY_SET(airspy_set_lna_gain, 4, uint8_t)
AIRSPY_SET(airspy_set_mixer_gain, 5, uint8_t)
AIRSPY_SET(airspy_set_vga_gain, 6, uint8_t)
int airspy_set_freq(void *device, uint32_t freq_hz) {
    if (mode == 23) return -1000;
    airspy_state[7] = (int)(freq_hz / 1000); return 0;
}
AIRSPY_SET(airspy_set_samplerate, 8, uint32_t)
int airspy_board_id_read(void *device, uint8_t *value) { *value = 0; return 0; }
const char *airspy_board_id_name(int id) { return id == 0 ? "AIRSPY" : "AIRSPY BOARD ID INVALID"; }
int airspy_version_string_read(void *device, char *version, uint8_t length) {
    const char *v = "AirSpy NOS v1.0.0-rc10-0-g946184a 2016-09-19";
    strncpy(version, v, length);
    version[length - 1] = 0;
    return 0;
}
#ifndef OMIT_LAST_SYMBOL
int airspy_get_samplerates(void *device, uint32_t *buffer, uint32_t len) {
    static const uint32_t r2[] = {10000000, 2500000};
    static const uint32_t mini[] = {6000000, 3000000};
    const uint32_t *rates = mode == 20 ? mini : r2;
    if (len == 0) { buffer[0] = 2; return 0; }
    if (len > 2) return -2;
    memcpy(buffer, rates, len * sizeof(uint32_t));
    return 0;
}
#endif

