# RTL-SDR

← [Supported hardware](hardware.md)

---

The cheapest way into all of this, and fully supported: the whole spectrum,
waterfall and lab stack. The dongle has one tuner gain and an AGC, so that is
what the screen offers, and the panels that need more than that say so instead
of making something up.

## At a glance

| | |
|---|---|
| Range | By tuner: 24 MHz to 1.766 GHz on R820T / R828D, 52 MHz to 2.2 GHz on E4000 |
| Sample rates | 0.9 to 3.2 Msps |
| Gain | One tuner gain, stepping a table the device reports |
| Front-end boost | The tuner's AGC (`a`) |
| Baseband filter | None, so the panel reads N/A |
| Friis noise figure | N/A: the dongle doesn't expose per-stage gains |
| Clip ceiling | The converter's 0 dBFS |
| Samples | Unsigned 8-bit, biased at 127.5 |

> **RTL clones vary.** Different tuners, different gain tables, different quirks,
> and no single person owns them all. If yours behaves oddly, please
> [open an issue](../../../issues) with the tuner type (`rtl_test -t` prints it)
> and what you saw. That's how "works on the units we've tried" becomes "works,
> full stop". The [troubleshooting page](troubleshooting.md) covers the common
> RTL-specific traps, starting with the kernel's DVB driver claiming the dongle
> before sdrtop can.

## Installing the library

`sh install.sh --rtlsdr`, or your distribution's package from
[Getting started](getting-started.md#what-you-need).

RTL-SDR needs librtlsdr with all required symbols. Linux candidates are
`librtlsdr.so.0`, `librtlsdr.so.2` and `librtlsdr.so`. Runtime packages include
`librtlsdr0` on Debian, `librtlsdr2` on Ubuntu and `rtl-sdr` on Arch.

---

← [Supported hardware](hardware.md)
