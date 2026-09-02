# Supported Hardware

← [Back](README.md)

---

## What works today

| Device | Status |
|--------|--------|
| HackRF One | Fully supported: spectrum, waterfall, every diagnostic |
| RTL-SDR (R820T / R828D / E4000) | Fully supported: the whole spectrum, waterfall and lab stack, with a single tuner gain plus AGC |
| **Anything with a SoapySDR driver** | Supported, **and not yet confirmed on hardware other than a HackRF**. See [below](#soapysdr-the-honest-version) |
| PortaPack H4M (Mayhem) | Fully supported (HackRF mode) |

The first two are built and tested on real hardware. Support for them was only
added after physical testing, never guessed from documentation. Datasheets have
been known to fib; an oscilloscope rarely does.

The third row is a deliberate exception, and it gets its own section rather than
a footnote, because you deserve to know which kind of "supported" you are
looking at.

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

## SoapySDR: the honest version

Here is the thing. sdrtop supports two radios because I own two radios, and I had
a rule: **hardware support lands only after physical testing.** It is a good
rule. It is also why every week somebody says they would try this if it spoke to
their Airspy, and every week the answer is "sorry, I do not have one".

So I broke my own rule, on purpose, in one specific place.

[SoapySDR](https://github.com/pothosware/SoapySDR) is one API with a lot of
radios behind it: Airspy, SDRplay's RSP line, PlutoSDR, LimeSDR, bladeRF, USRP,
and SoapyRemote for a radio on a different machine entirely. sdrtop now speaks
it. **The backend was written from the SoapySDR headers, not from owning the
devices**, which means it is exactly as good as the drivers are honest and as
careful as I could be reading a C header at midnight.

What I did instead of pretending otherwise:

- **Nothing about your radio is hardcoded.** Frequency range, sample rates, gain
  range, whether there is an AGC, whether there is a baseband filter, the sample
  format and how many bits actually mean something in it: sdrtop asks the driver
  for all of it. There is no table of devices in the source, because a table
  would be me guessing about hardware I have never held.
- **What cannot be asked is refused, not invented.** If sdrtop cannot work
  something out, that reading reads as unavailable. It does not fall back to a
  plausible number. A plausible number is the worst thing a measurement tool can
  produce.
- **When a driver disagrees with sdrtop, the log says so by name**, including the
  driver's own error text. That is in `~/.config/sdrtop/sdrtop.log`, and it is
  what makes a bug report actionable instead of a mystery.

### What is actually verified

| Thing | State |
|---|---|
| Loading libSoapySDR, ABI check, enumeration | Verified on a real install |
| Opening a device, reading its capabilities | Verified, on a HackRF through `SoapyHackRF` |
| Streaming, at full sample rate, no drops | Verified, same HackRF, 8 Msps and 10 Msps |
| The two radios above, unaffected | Verified. The native paths did not change |
| **Every other radio** | **Unverified.** The code is right as far as I can reason. That is not the same as right |

So if you have an Airspy, an RSP, a Pluto or a Lime: you are the test. I would
genuinely love an issue either way, working or not. "It works, here is what the
header said" is as useful to me as a crash.

### Using it

Nothing to enable. If libSoapySDR is installed, sdrtop finds it at startup and
your devices appear in the picker next to any HackRF or RTL-SDR. If it is not
installed, sdrtop behaves exactly as it did before, same binary, no complaints.

```sh
# Is your radio visible to SoapySDR at all? Start here, always.
SoapySDRUtil --find

# Everything SoapySDR can see, and nothing else
sdrtop --device soapy

# One driver in particular
sdrtop --device soapy=driver=airspy
```

`SoapySDRUtil --find` is the first thing to run and the first thing to paste into
an issue. If SoapySDR cannot see your radio, sdrtop has no chance, and the
problem is a missing driver module rather than anything in here.

### The gotchas I already hit, so you do not have to

| Symptom | Cause / fix |
|---|---|
| Your **sound card** shows up as an SDR | It is real: `soapysdr-module-audio` presents audio inputs as SDR sources, which is genuinely useful with a soundcard receiver and confusing on a laptop. sdrtop skips the `audio` driver by default. Ask for it with `--device soapy=driver=audio` |
| Your HackRF or RTL-SDR appears **once**, not twice, even with the Soapy module installed | On purpose. sdrtop's own driver for those two knows more about them than the generic path does, so the native one wins. Force the other with `--device soapy` |
| The **RF bench is missing** its noise figure, MDS and linearity card | Also on purpose. Those model a specific front end stage by stage. SoapySDR does not publish that, and inventing it would be worse than leaving it out |
| **No `[A]` boost** on your device | sdrtop asks the driver whether there is an automatic gain mode and only offers the key if there is. A HackRF reached through SoapySDR reports there is not, which surprised me too |
| `[` and `]` do nothing | One overall gain, no second stage. See below |
| "its native sample format is CF32" in the log | sdrtop's pipeline is integer, so it handles `CS8`, `CU8` and `CS16` and refuses the rest by name rather than guessing at a conversion. Open an issue with the driver name |
| The frequency range looks **too optimistic** | It is the driver's number, not mine. `SoapyHackRF` claims 0 to 7.25 GHz where the datasheet says 1 MHz to 6 GHz. sdrtop reports what it is told; the radio will refuse the rest, and the log will say so |

### What a SoapySDR device gets less of

| | Native HackRF / RTL-SDR | Through SoapySDR |
|---|---|---|
| Gain | Named stages, LNA and VGA separately | One overall gain across the device's own range. `↑` / `↓` move it |
| Front-end boost | Always there | Only if the driver reports an automatic gain mode |
| Friis noise figure, MDS | Modelled per stage | Not shown. We do not know the chain |
| Linearity card (IIP3, IMD3, SFDR) | Shown | Not shown. Those are one front end's datasheet |
| ADC bench | 8-bit | Follows the converter the driver reports, so a 12 or 14-bit radio is described as one |
| Everything else | | The same |

Mapping SoapySDR's named gain elements onto sdrtop's LNA and VGA is the obvious
next step, and it is deliberately not in this release: the mapping is different
on every device, and a wrong guess silently drives the wrong stage. That is the
sort of bug that costs somebody an afternoon. It lands when there are enough
reports to aim at.

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

**libSoapySDR is not needed to build and not needed to run.** It is opened at
runtime if it happens to be there, which is why the same binary serves people who
have it and people who have never heard of it. On Debian and Ubuntu that is
`libsoapysdr0.8` plus a driver module such as `soapysdr-module-airspy`, or
`soapysdr-module-all` if you are feeling generous with disk space.

Linux only, and that's not an oversight: the whole thing is built on libusb
behaviour and `/sys` that don't have direct equivalents elsewhere.

---

## What's coming

| Device | Status | Notes |
|--------|--------|-------|
| Airspy Mini | Planned | Needs hardware to test |
| Airspy HF+ Discovery | Planned | Needs hardware to test |
| HackRF Pro | Planned | Needs hardware to test |
| LimeSDR / bladeRF / SDRplay / PlutoSDR | Try SoapySDR | These should already work through the SoapySDR backend. Nobody has told me yet |

**Native** device support means physically owning and testing the hardware, and
development here runs on a HackRF One and a PortaPack H4M. That is the whole
bottleneck: the list moves at exactly the speed of a hobby budget. The SoapySDR
backend exists precisely because that speed was not good enough.

If you'd like to move it along, the wishlist and the Ko-fi link live in the
[project README](../README.md#supported-hardware).
