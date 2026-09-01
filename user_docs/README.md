# sdrtop User Guide

Welcome. This is the plain-language guide to using sdrtop.

> **Status:** the interactive TUI is feature-complete (spectrum, waterfall, the
> four lab benches, the sweep scanner and the micro field views), and both the
> HackRF One and the RTL-SDR are fully supported. Anything with a **SoapySDR**
> driver now works too, written from the API rather than from owning the radio,
> which is a different kind of "supported" and
> [says so out loud](hardware.md#soapysdr-the-honest-version). The current arc is
> polish: instrument-grade UI, sharper radio math, and bug fixing. See
> [What's New](whats-new.md).

---

## Start here

- **[Getting started](getting-started.md)**: install, build, first run
- **[Keyboard shortcuts](keys.md)**: every key, including every focus mode
- **[What you see on screen](screens.md)**: every panel, explained

## Going deeper

- **[The Lab presets](lab.md)**: what each measurement means and how to act on it
- **[Tips and Tricks](tips-and-tricks.md)**: setting gain, pulling weak signals
  out of the noise, capture checklists
- **[Advanced Features](advanced.md)**: multiple radios, observer mode, the
  session log, what sdrtop deliberately doesn't do
- **[Troubleshooting](troubleshooting.md)**: when it doesn't work

## Setting it up

- **[Configuration](config.md)**: the config file, markers, the sweep band
- **[Layout presets](presets.md)**: the sixteen built-in layouts, and writing
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
