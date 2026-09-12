# Native SDR ABI checks

All 15 RTL-SDR functions loaded by `src/hardware/native/rtlsdr/ffi.rs` have identical declarations in the three checked development packages.
Each runtime exports all 15 names.
Their argument widths, return widths, tuner values, and callback ABI match the Rust declarations.

## Package evidence

Checked on 2026-09-11 using the `amd64` packages below.
Downloads were extracted with `dpkg-deb -x` into session artifacts.
No packages were installed.

| Distribution | Version | Runtime download | Development download | Runtime SONAME |
| --- | --- | --- | --- | --- |
| Debian 12 Bookworm | `0.6.0-4` | [librtlsdr0](https://deb.debian.org/debian/pool/main/r/rtl-sdr/librtlsdr0_0.6.0-4_amd64.deb) | [librtlsdr-dev](https://deb.debian.org/debian/pool/main/r/rtl-sdr/librtlsdr-dev_0.6.0-4_amd64.deb) | `librtlsdr.so.0` |
| Debian 13 Trixie | `2.0.2-2+b1` | [librtlsdr0](https://deb.debian.org/debian/pool/main/r/rtl-sdr/librtlsdr0_2.0.2-2+b1_amd64.deb) | [librtlsdr-dev](https://deb.debian.org/debian/pool/main/r/rtl-sdr/librtlsdr-dev_2.0.2-2+b1_amd64.deb) | `librtlsdr.so.0` |
| Ubuntu 24.04 Noble | `2.0.1-2build1` | [librtlsdr2](https://archive.ubuntu.com/ubuntu/pool/universe/r/rtl-sdr/librtlsdr2_2.0.1-2build1_amd64.deb) | [librtlsdr-dev](https://archive.ubuntu.com/ubuntu/pool/universe/r/rtl-sdr/librtlsdr-dev_2.0.1-2build1_amd64.deb) | `librtlsdr.so.2` |
| Debian 13 Trixie | `2024.02.1-3` | [libhackrf0](https://deb.debian.org/debian/pool/main/h/hackrf/libhackrf0_2024.02.1-3_amd64.deb) | [libhackrf-dev](https://deb.debian.org/debian/pool/main/h/hackrf/libhackrf-dev_2024.02.1-3_amd64.deb) | `libhackrf.so.0` |

The checks used `usr/include/rtl-sdr.h` and `usr/include/libhackrf/hackrf.h`.
`readelf -d` verified SONAMEs.
`readelf --dyn-syms --wide` verified defined global function exports.
GCC 15.2 compiled C11 `_Static_assert` probes with `-Wall -Wextra -Werror`.
The probes checked function-pointer types, callback types, scalar widths, enum values, and HackRF structure layouts.
Rust signature comparisons and a compiled Rust layout probe also passed.
All checks passed.

## RTL-SDR signatures

Names below have the `rtlsdr_` prefix.
`dev*` means the opaque `rtlsdr_dev_t*`.
`I32` is C `int` / Rust `c_int`.
`U32` is C `uint32_t` / Rust `u32`.
Characters occupy 8 bits.
Pointers occupy 64 bits on the checked `amd64` target.
All functions and callbacks use the C calling convention.

| Function | Arguments in order | Return |
| --- | --- | --- |
| `get_device_count` | none | U32 |
| `get_device_name` | U32 index | `const char*` |
| `get_device_usb_strings` | U32 index, `char*` manufacturer, `char*` product, `char*` serial | I32 |
| `open` | `dev**`, U32 index | I32 |
| `close` | `dev*` | I32 |
| `set_center_freq` | `dev*`, U32 frequency | I32 |
| `set_sample_rate` | `dev*`, U32 rate | I32 |
| `get_sample_rate` | `dev*` | U32 |
| `get_tuner_type` | `dev*` | 32-bit `enum rtlsdr_tuner` |
| `get_tuner_gains` | `dev*`, `int*` gains | I32 |
| `set_tuner_gain_mode` | `dev*`, I32 manual | I32 |
| `set_tuner_gain` | `dev*`, I32 gain | I32 |
| `reset_buffer` | `dev*` | I32 |
| `read_async` | `dev*`, callback, `void*` context, U32 buffer count, U32 buffer length | I32 |
| `cancel_async` | `dev*` | I32 |

`rtlsdr_read_async_cb_t` is `void (*)(unsigned char*, uint32_t, void*)`.
Its buffer contains unsigned 8-bit samples.
Its length is unsigned 32-bit.
Rust's `RtlSdrReadAsyncCb` has the same signature.

`enum rtlsdr_tuner` contains `UNKNOWN=0`, `E4000=1`, `FC0012=2`, `FC0013=3`, `FC2580=4`, `R820T=5`, and `R828D=6`.
Every name has the `RTLSDR_TUNER_` prefix.
The enum occupies four bytes in all three probes.
GCC selects `unsigned int` as its compatible integer type.
Rust's `c_int` has the same return ABI for these values.

## HackRF enum and callback checks

The checked runtime exports all 22 functions loaded by `src/hardware/native/hackrf/ffi.rs`.
Their header signatures match the current Rust ABI.
`hackrf_board_id_name` takes `enum hackrf_board_id` and returns `const char*`.
Its enum argument occupies four bytes.
The runtime disassembly compares the full `%edi` register.
A Rust `u8` argument has the wrong declared width.
`hackrf_board_id_read` separately writes through a `uint8_t*`.

`hackrf_board_id` values are `0` through `4`, `254`, and `255`.
`hackrf_usb_board_id` values are `0x604B`, `0x6089`, `0xCC15`, and `0xFFFF`.
Both enums have the 32-bit call/storage ABI used by Rust's `c_int`.
GCC chooses `unsigned int` for both.
`hackrf_error` is compatible with signed 32-bit `int`.

`hackrf_sample_block_cb_fn` is `int (*)(hackrf_transfer*)`.
`hackrf_transfer` fields are device pointer, `uint8_t*` buffer, two C `int` lengths, RX context pointer, and TX context pointer.
Their `amd64` offsets are `0, 8, 16, 20, 24, 32`.
Size is 40 bytes with alignment 8.
`hackrf_device_list_t` field offsets are `0, 8, 16, 24, 32, 40`.
Size is 48 bytes with alignment 8.
Its USB board array stores four-byte enum elements.
`read_partid_serialno_t` contains `uint32_t[2]` followed by `uint32_t[4]`.
Size is 24 bytes with alignment 4.
These layouts match the Rust `repr(C)` definitions.

## Compatibility boundary

These results cover the listed package builds on `amd64`.
They support loading both RTL-SDR SONAMEs for this symbol subset.
Other architectures and future library builds need their own ABI evidence.

The native APIs have no Soapy-style ABI version gate.
The loader requires every symbol in its declared subset.
Symbol availability alone cannot validate a handwritten function signature.
The former pkg-config checks supplied compiler/linker flags.
The linker resolved names.
Neither checked the manual Rust argument types, return types, callbacks, or struct layouts against C headers.
