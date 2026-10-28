// SPDX-License-Identifier: GPL-3.0-or-later

// For pthreads and nanosleep under -std=c11.
#define _POSIX_C_SOURCE 200809L

#include <math.h>
#include <pthread.h>
#include <time.h>
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
// rates, 21 lists the radio but will not open it, as 1.0.10 answers for one
// another program has claimed (AIRSPY_ERROR_NOT_FOUND), 22 finds no device, 23 fails
// every setting (AIRSPY_ERROR_LIBUSB), 24 streams three blocks and then stops
// by itself, as libairspy does when a USB transfer comes back short.
int airspy_list_devices(uint64_t *serials, int count) {
    if (mode == 22) return 0;
    if (serials && count >= 1) serials[0] = 0x637862DC2E1182D7ULL;
    return 1;
}
int airspy_open_sn(void **device, uint64_t serial) {
    if (mode == 21) return -5;
    *device = malloc(1);
    return 0;
}
int airspy_close(void *device) { calls[3]++; free(device); return 0; }
// What the backend asked libairspy for last, by field (see fixture_airspy_state):
// 0 sample type, 1 bias tee, 2 LNA AGC, 3 mixer AGC, 4 LNA, 5 mixer, 6 VGA,
// 7 frequency in kHz, 8 sample rate argument; 9 streams started, 10 stops
// asked, 11 the sample rate argument in force when the last stream started,
// 12 packing, 13 packing in force when the last stream started. -1 until set.
static int airspy_state[14] = {-1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 0, -1, -1, -1};
int fixture_airspy_state(int field) { return airspy_state[field]; }
typedef struct { uint32_t part_id[2]; uint32_t serial_no[4]; } airspy_read_partid_serialno_t;
typedef struct { uint32_t major_version; uint32_t minor_version; uint32_t revision; } airspy_lib_version_t;
// airspy_transfer_t in airspy.h, field for field.
typedef struct {
    void *device;
    void *ctx;
    void *samples;
    int sample_count;
    uint64_t dropped_samples;
    int sample_type;
} airspy_transfer_t;
size_t fixture_airspy_layout(int field) {
    switch (field) {
        case 0: return sizeof(airspy_read_partid_serialno_t);
        case 1: return offsetof(airspy_read_partid_serialno_t, serial_no);
        case 2: return sizeof(airspy_lib_version_t);
        case 3: return sizeof(airspy_transfer_t);
        case 4: return offsetof(airspy_transfer_t, samples);
        case 5: return offsetof(airspy_transfer_t, sample_count);
        case 6: return offsetof(airspy_transfer_t, dropped_samples);
        case 7: return offsetof(airspy_transfer_t, sample_type);
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
const char *airspy_error_name(int code) {
    return code == -5 ? "AIRSPY_ERROR_NOT_FOUND" : code == -6 ? "AIRSPY_ERROR_BUSY" : "AIRSPY_ERROR_OTHER";
}
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
typedef int (*airspy_cb)(airspy_transfer_t *);
static struct {
    pthread_t thread;
    int running;
    airspy_cb cb;
    void *ctx;
    void *device;
} airspy_stream;
static _Atomic int airspy_streaming, airspy_stop;
// libairspy's consumer thread: three blocks of four pairs, block k's pair i
// carrying I = 100k + i and Q = -(100k + i). Before the second, one buffer
// of the queue was lost: four pairs, counted at that block's size.
static void *airspy_consumer(void *arg) {
    int16_t samples[8];
    for (int k = 0; k < 3; k++) {
        for (int i = 0; i < 4; i++) {
            samples[2 * i] = (int16_t)(100 * k + i);
            samples[2 * i + 1] = (int16_t)-(100 * k + i);
        }
        airspy_transfer_t t = {airspy_stream.device, airspy_stream.ctx, samples, 4,
                               k == 1 ? 4 : 0, 2};
        airspy_stream.cb(&t);
    }
    if (mode == 24) { airspy_streaming = 0; return NULL; }
    while (!airspy_stop) {
        struct timespec pause = {0, 1000000};
        nanosleep(&pause, NULL);
    }
    return NULL;
}
int airspy_start_rx(void *device, airspy_cb cb, void *ctx) {
    if (mode == 23 || airspy_streaming) return -1000;
    airspy_state[9]++;
    airspy_state[11] = airspy_state[8];
    airspy_state[13] = airspy_state[12];
    airspy_stream.cb = cb;
    airspy_stream.ctx = ctx;
    airspy_stream.device = device;
    airspy_stop = 0;
    airspy_streaming = 1;
    airspy_stream.running = pthread_create(&airspy_stream.thread, NULL, airspy_consumer, NULL) == 0;
    return airspy_stream.running ? 0 : -1000;
}
int airspy_stop_rx(void *device) {
    airspy_state[10]++;
    airspy_stop = 1;
    if (airspy_stream.running) { pthread_join(airspy_stream.thread, NULL); airspy_stream.running = 0; }
    airspy_streaming = 0;
    return 0;
}
int airspy_is_streaming(void *device) { return airspy_streaming; }
// libairspy refuses packing while streaming, with AIRSPY_ERROR_BUSY.
int airspy_set_packing(void *device, uint8_t value) {
    if (mode == 23) return -1000;
    if (airspy_streaming) return -6;
    airspy_state[12] = value; return 0;
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


// libairspyhf 1.6.8. Modes 30 and up are this library's: 30 reports a Ranger's
// part id, 31 lists the radio but will not open it, as for one another program
// has claimed (AIRSPYHF_ERROR), 32 finds no device.
int airspyhf_list_devices(uint64_t *serials, int count) {
    if (mode == 32) return 0;
    if (serials && count >= 1) serials[0] = 0xDD528080FF444630ULL;
    return 1;
}
int airspyhf_open_sn(void **device, uint64_t serial) {
    if (mode == 31) return -1;
    *device = malloc(1);
    return 0;
}
int airspyhf_close(void *device) { calls[3]++; free(device); return 0; }
// What the backend asked libairspyhf for last, by field (see
// fixture_airspyhf_state): 0 the length passed to version_string_read, 1 the
// frequency in kHz, 2 the rate argument, 3 library DSP, 4 attenuator index,
// 5 LNA, 6 AGC, 7 AGC threshold, 8 streams started, 9 stops asked, 10 the
// rate in force when the last stream started, 11 tunings asked, 12 library
// DSP when the last stream started. -1 until asked, the counts from 0.
static int airspyhf_state[13] = {-1, -1, -1, -1, -1, -1, -1, -1, 0, 0, -1, 0, -1};
int fixture_airspyhf_state(int field) { return airspyhf_state[field]; }
typedef struct { uint32_t part_id; uint32_t serial_no[4]; } airspyhf_read_partid_serialno_t;
size_t fixture_airspyhf_layout(int field) {
    switch (field) {
        case 0: return sizeof(airspyhf_read_partid_serialno_t);
        case 1: return offsetof(airspyhf_read_partid_serialno_t, serial_no);
        default: abort();
    }
}
// The words the bench's Discovery reports (part id 2), or a Ranger's (3).
int airspyhf_board_partid_serialno_read(void *device, airspyhf_read_partid_serialno_t *p) {
    p->part_id = mode == 30 ? 3 : 2;
    p->serial_no[0] = 0xDD528080; p->serial_no[1] = 0xFF444630;
    p->serial_no[2] = 0x33303520; p->serial_no[3] = 0x34303031;
    return 0;
}
// As 1.6.8 does: length - 1 bytes from a 64-byte buffer, then a NUL. The
// length is recorded, and the copy kept inside the buffer here so that a test
// can say a longer request was made rather than read past it.
int airspyhf_version_string_read(void *device, char *version, uint8_t length) {
    char local[64] = "R5.0.1-CD";
    airspyhf_state[0] = length;
    if (length == 0) return -1;
    memcpy(version, local, length - 1 < 63 ? length - 1 : 63);
    version[length - 1] = 0;
    return 0;
}
typedef struct { uint32_t major_version; uint32_t minor_version; uint32_t revision; } airspyhf_lib_version_t;
void airspyhf_lib_version(airspyhf_lib_version_t *v) {
    v->major_version = 1; v->minor_version = 6; v->revision = 8;
}
// Mode 33: a radio whose flash holds a correction of +250 ppb.
int airspyhf_get_calibration(void *device, int32_t *ppb) {
    *ppb = mode == 33 ? 250 : 0;
    return 0;
}
// Mode 34 fails every setting, as a radio that left the bus would.
#define AIRSPYHF_SET(name, field, type) \
    int name(void *device, type value) { \
        if (mode == 34) return -1; \
        airspyhf_state[field] = (int)value; return 0; }
int airspyhf_set_freq(void *device, uint32_t freq_hz) {
    if (mode == 34) return -1;
    airspyhf_state[11]++;
    airspyhf_state[1] = (int)(freq_hz / 1000); return 0;
}
AIRSPYHF_SET(airspyhf_set_samplerate, 2, uint32_t)
AIRSPYHF_SET(airspyhf_set_lib_dsp, 3, uint8_t)
AIRSPYHF_SET(airspyhf_set_hf_att, 4, uint8_t)
AIRSPYHF_SET(airspyhf_set_hf_lna, 5, uint8_t)
AIRSPYHF_SET(airspyhf_set_hf_agc, 6, uint8_t)
AIRSPYHF_SET(airspyhf_set_hf_agc_threshold, 7, uint8_t)
// The firmware's architectures as the bench's Discovery lists them: 912 and
// 768 kS/s zero-IF, the rest low-IF.
int airspyhf_is_low_if(void *device) {
    return airspyhf_state[2] >= 0 && airspyhf_state[2] <= 650000;
}
// 1.6.8's block.
int airspyhf_get_output_size(void *device) { return 2048; }
// airspyhf_transfer_t in airspyhf.h, field for field.
typedef struct { float re; float im; } airspyhf_complex_float_t;
typedef struct {
    void *device;
    void *ctx;
    airspyhf_complex_float_t *samples;
    int sample_count;
    uint64_t dropped_samples;
} airspyhf_transfer_t;
size_t fixture_airspyhf_transfer_layout(int field) {
    switch (field) {
        case 0: return sizeof(airspyhf_transfer_t);
        case 1: return offsetof(airspyhf_transfer_t, samples);
        case 2: return offsetof(airspyhf_transfer_t, sample_count);
        case 3: return offsetof(airspyhf_transfer_t, dropped_samples);
        default: abort();
    }
}
typedef int (*airspyhf_cb)(airspyhf_transfer_t *);
static struct { pthread_t thread; int running; airspyhf_cb cb; void *ctx; void *device; } airspyhf_stream;
static _Atomic int airspyhf_streaming, airspyhf_stop_flag;
// The firmware's filter gain at the bench Discovery's rates, dB: what the
// library divides the words by (192 kS/s 16 dB, 768 kS/s none).
static int airspyhf_gain_db(void) { return airspyhf_state[2] == 192000 ? 16 : 0; }
// libairspyhf's consumer thread: four blocks of four pairs, block k's pair i
// carrying the words I = 100k + i + 1 and Q = -(100k + i + 1), times the
// filter gain over 32768 as the library computes it in float. With the DSP
// off those are the words exactly; with it on, a quarter of a word is added,
// as the library's correction leaves fractions. Before the third block one
// buffer was lost: four pairs, at that block's size.
static void *airspyhf_consumer(void *arg) {
    airspyhf_complex_float_t samples[4];
    for (int k = 0; k < 4; k++) {
        float gain = (1.0f / 32768) * powf(10.0f, (float)airspyhf_gain_db() * -0.05f);
        float frac = airspyhf_state[3] == 1 ? 0.25f : 0.0f;
        for (int i = 0; i < 4; i++) {
            samples[i].re = ((float)(100 * k + i + 1) + frac) * gain;
            samples[i].im = -((float)(100 * k + i + 1) + frac) * gain;
        }
        airspyhf_transfer_t t = {airspyhf_stream.device, airspyhf_stream.ctx, samples, 4, k == 2 ? 4 : 0};
        airspyhf_stream.cb(&t);
    }
    if (mode == 35) { airspyhf_streaming = 0; return NULL; }
    while (!airspyhf_stop_flag) {
        struct timespec pause = {0, 1000000};
        nanosleep(&pause, NULL);
    }
    return NULL;
}
int airspyhf_start(void *device, airspyhf_cb cb, void *ctx) {
    if (mode == 34 || airspyhf_streaming) return -1;
    airspyhf_state[8]++;
    airspyhf_state[10] = airspyhf_state[2];
    airspyhf_state[12] = airspyhf_state[3];
    airspyhf_stream.cb = cb;
    airspyhf_stream.ctx = ctx;
    airspyhf_stream.device = device;
    airspyhf_stop_flag = 0;
    airspyhf_streaming = 1;
    airspyhf_stream.running = pthread_create(&airspyhf_stream.thread, NULL, airspyhf_consumer, NULL) == 0;
    return airspyhf_stream.running ? 0 : -1;
}
int airspyhf_stop(void *device) {
    airspyhf_state[9]++;
    airspyhf_stop_flag = 1;
    if (airspyhf_stream.running) { pthread_join(airspyhf_stream.thread, NULL); airspyhf_stream.running = 0; }
    airspyhf_streaming = 0;
    return 0;
}
int airspyhf_is_streaming(void *device) { return airspyhf_streaming; }
#ifndef OMIT_LAST_SYMBOL
int airspyhf_get_samplerates(void *device, uint32_t *buffer, uint32_t len) {
    static const uint32_t rates[] = {912000, 768000, 650000, 456000, 384000, 228000, 192000};
    if (len == 0) { buffer[0] = 7; return 0; }
    if (len > 7) return -1;
    memcpy(buffer, rates, len * sizeof(uint32_t));
    return 0;
}
#endif
