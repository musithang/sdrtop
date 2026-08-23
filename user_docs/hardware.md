# Supported Hardware

← [Back](README.md)

---

## What works today

| Device | Status |
|--------|--------|
| HackRF One | Fully supported: spectrum, waterfall, every diagnostic |
| RTL-SDR (R820T / R828D / E4000) | Fully supported: the whole spectrum, waterfall and lab stack, with a single tuner gain plus AGC |
| PortaPack H4M (Mayhem) | In development: telemetry panel over USB serial |

sdrtop is built and tested on real hardware. Support is only added after physical
testing, never guessed from documentation alone. Datasheets have been known to
fib; an oscilloscope rarely does.

### What differs between the two

The UI adapts rather than showing you fields that can't mean anything:

| | HackRF One | RTL-SDR |
|---|---|---|
| Frequency range | 1 MHz to 6 GHz | By tuner: 24 MHz to 1.766 GHz on R820T / R828D, 52 MHz to 2.2 GHz on E4000 |
| Sample rate | 2 to 20 Msps | 0.9 to 3.2 Msps |
| Gain stages | LNA (0–40, step 8) and VGA (0–62, step 2) | One tuner gain, stepping a table the device reports |
| Front-end boost | RF amplifier (`a`) | Tuner AGC (`a`) |
| Baseband filter | Tunable, follows the sample rate | None, so the panel reads N/A |
| Friis noise figure | Computed per stage | N/A: the dongle doesn't expose per-stage gains |
| Sample format | Signed 8-bit | Unsigned 8-bit, biased at 127.5 |

Everything else, including every lab bench, the demodulator and the sweep scanner,
works the same on both.

> **RTL clones vary.** Different tuners, different gain tables, different quirks,
> and no single person owns them all. If yours behaves oddly, please
> [open an issue](../../../issues) with the tuner type (`rtl_test -t` prints it)
> and what you saw. That's how "works on the units we've tried" becomes "works,
> full stop". The [troubleshooting page](troubleshooting.md) covers the common
> RTL-specific traps, starting with the kernel's DVB driver claiming the dongle
> before sdrtop can.

---

## Host platforms

| Platform | Status |
|----------|--------|
| x86-64 Linux | Fully supported |
| Raspberry Pi (Pi 2 and newer, 64-bit Raspberry Pi OS Bookworm) | Supported, with lower sample rates on older Pis |
| ARM / Android (Termux) | Builds and runs; needs a root-capable USB stack to reach the device |

sdrtop needs **libhackrf 2023.01.1 or newer**, which is what ships in Raspberry Pi
OS Bookworm and Ubuntu 24.04. Older distributions need it built from source. It
also links **librtlsdr** (`librtlsdr-dev` on Debian and Ubuntu, `rtl-sdr` on
Arch), and both are needed at build time regardless of which radio you own.

Linux only, and that's not an oversight: the whole thing is built on libusb
behaviour and `/sys` that don't have direct equivalents elsewhere.

---

## What's coming

| Device | Status | Notes |
|--------|--------|-------|
| Airspy Mini | Planned | Needs hardware to test |
| Airspy HF+ Discovery | Planned | Needs hardware to test |
| HackRF Pro | Planned | Needs hardware to test |
| LimeSDR / bladeRF / SDRplay / PlutoSDR | Planned | Wide range of devices, likely via SoapySDR |

New device support means physically owning and testing the hardware, and
development here runs on a HackRF One and a PortaPack H4M. That's the whole
bottleneck: the list moves at exactly the speed of a hobby budget.

If you'd like to move it along, the wishlist and the Ko-fi link live in the
[project README](../README.md#supported-hardware).
