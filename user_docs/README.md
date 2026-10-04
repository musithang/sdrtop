# sdrtop User Guide

Welcome. This is the plain-language guide to using sdrtop.

> **Status:** spectrum, waterfall, the four lab benches, the sweep scanner, the
> micro field views, IQ recording and the NET section (BLE, LE Coded and
> classic Bluetooth, measured), with the HackRF One, the RTL-SDR and the tinySA
> verified on hardware. Anything with a **SoapySDR** driver works too, written
> from the API rather than from owning the radio, which is a different kind of
> "supported" and [says so out loud](hardware.md#soapysdr-the-honest-version).
> New features keep coming, and each is followed by its polish: a UI that reads
> at a glance, sharper radio math, bug fixing. See [What's New](whats-new.md).

---

## Start here

- **[Getting started](getting-started.md)**: install, build, first run
- **[Keyboard shortcuts](keys.md)**: every key, including every focus mode
- **[What you see on screen](screens.md)**: every panel, explained

## Going deeper

- **[The Lab presets](lab.md)**: what each measurement means and how to act on it
- **[The NET section](net.md)**: the 2.4 GHz band, BLE, LE Coded and classic Bluetooth:
  what each view answers, and how far each number can be trusted
- **[Recording the IQ stream](recording.md)**: `Ctrl+R`, the SigMF files it
  writes, and how a recording says what it lost
- **[How the demodulator works](demodulator.md)**: the signal chain behind the
  FM, RDS, CTCSS and AM readings, written from scratch in Rust
- **[Tips and Tricks](tips-and-tricks.md)**: setting gain, pulling weak signals
  out of the noise, capture checklists
- **[Advanced Features](advanced.md)**: multiple radios, observer mode, the
  session log, what sdrtop deliberately doesn't do
- **[Troubleshooting](troubleshooting.md)**: when it doesn't work

## Setting it up

- **[Configuration](config.md)**: the config file, markers, the sweep band
- **[Layout presets](presets.md)**: the twenty-one built-in layouts, and writing
  your own
- **[Themes](themes.md)**: the six palettes, per-field overrides, and writing
  your own
- **[Supported hardware](hardware.md)**: what works today, and how the two
  radios differ

## Updates

- **[What's new](whats-new.md)**: the checkpoint log, in plain language

---

Each fact lives in exactly one of these pages, and the others link to it. If you
find the same thing explained two different ways, that's a bug in the docs, and
worth an issue as much as a bug in the code.
