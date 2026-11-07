# HackRF One and PortaPack H4M

← [Supported hardware](hardware.md)

---

The radio sdrtop grew up on. Development runs on a HackRF One and a PortaPack
H4M, so this is the reference: every bench, every diagnostic, and the one radio
where the RF bench models the front end stage by stage, Friis noise figure and
all.

## At a glance

| | |
|---|---|
| Range | 1 MHz to 6 GHz |
| Sample rates | 2 to 20 Msps |
| Gain stages | LNA (0 to 40 dB, step 8) and VGA (0 to 62 dB, step 2) |
| Front-end boost | The RF amplifier (`a`) |
| Baseband filter | Tunable, follows the sample rate |
| Friis noise figure | Computed per stage |
| Clip ceiling | The converter's 0 dBFS |
| Samples | Signed 8-bit |

## PortaPack H4M

A PortaPack running Mayhem, put in HackRF mode, is a HackRF as far as sdrtop is
concerned: everything on this page applies, diagnostics included.

## Installing the library

`sh install.sh --hackrf`, or your distribution's package from
[Getting started](getting-started.md#what-you-need).

HackRF needs libhackrf 2023.01.1+ with all required symbols, including
`hackrf_board_rev_read` and `hackrf_usb_api_version_read`. Linux candidates are
`libhackrf.so.0` and `libhackrf.so`. Firmware metadata reads may fail without
preventing the device from opening.

---

← [Supported hardware](hardware.md)
