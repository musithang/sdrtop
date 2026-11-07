# sdrtop User Guide

Welcome. This is the plain-language guide to using sdrtop.

> **Status:** spectrum, waterfall, the four lab benches, the sweep scanner, the
> compact field views, the pager, FT8, IQ recording and the NET section
> (Bluetooth and Wi-Fi, measured), with the HackRF One, the RTL-SDR, the Airspy
> R2, the Airspy HF+ Discovery, the SDRplay RSP1B and the tinySA verified on
> hardware. Anything with a **SoapySDR** driver works too, written from the API
> rather than from owning the radio, which is a different kind of "supported"
> and [says so out loud](soapysdr.md). The [changelog](../CHANGELOG.md) has what
> changed lately.

**Contents:**
[Start here](#start-here) ·
[Spectrum and the Lab](#spectrum-and-the-lab) ·
[Signals and decoders](#signals-and-decoders) ·
[NET: the 2.4 GHz band](#net-the-24-ghz-band) ·
[Recording](#recording) ·
[Hardware](#hardware) ·
[Making it yours](#making-it-yours) ·
[Help](#help)

---

## I want to...

| ...do this | Go here |
|---|---|
| Install sdrtop and get a picture on screen | [Getting started](getting-started.md) |
| Find a key I forgot | [Keyboard shortcuts](keys.md) |
| Know what a number on screen means | [What you see on screen](screens.md) |
| Set the gain properly | [Tips and tricks](tips-and-tricks.md) |
| Check whether my radio works with sdrtop | [Supported hardware](hardware.md) |
| Fix something that doesn't work | [Troubleshooting](troubleshooting.md) |

---

## Start here

| Page | What's in it |
|---|---|
| [Getting started](getting-started.md) | Install, build, first run |
| [Keyboard shortcuts](keys.md) | Every key, including every focus mode |
| [What you see on screen](screens.md) | Every panel, explained |

## Spectrum and the Lab

| Page | What's in it |
|---|---|
| [The Lab presets](lab.md) | The four benches: what each measurement means and how to act on it |
| [Tips and tricks](tips-and-tricks.md) | Setting gain, pulling weak signals out of the noise, capture checklists |
| [Advanced features](advanced.md) | Multiple radios, observer mode, the session log, what sdrtop deliberately doesn't do |

## Signals and decoders

| Page | What's in it |
|---|---|
| [The demodulator](demodulator.md) | The signal chain behind the FM, RDS, CTCSS and AM readings, written from scratch in Rust |
| [The pager](radio-pocsag.md) | POCSAG pages on a pager's own face, and the link each one came on |
| [FT8](radio-ft8.md) | The world's FT8 stations on a map around you, and the clock as the stations judge it |

## NET: the 2.4 GHz band

Bluetooth and Wi-Fi, decoded and then measured like any other transmitter on
the bench. **[The NET section](net.md)** is the front door; start there.

| Page | What's in it |
|---|---|
| [Reading any NET screen](net-reading.md) | What every NET view shares; read it once and the rest get shorter |
| [What a NET number is worth](net-trust.md) | What each measurement is held against, and how far to trust it |
| [The band](net-band.md) | `Overview 1` and `2`: can this radio do the job, and what is in the band |
| [Taking the data away](net-export.md) | `o`: the files NET writes |

**Bluetooth**

| Page | What's in it |
|---|---|
| [Bluetooth Low Energy](net-le.md) | `Low Energy 1` and `2`: who is advertising, and each packet measured |
| [One LE link](net-le-connection.md) | `Low Energy 3`: one connection, followed from the sidelines |
| [Long-range devices](net-le-coded.md) | `Long range 1`: LE Coded |
| [Piconets nearby](net-classic.md) | `Classic 1`: the classic Bluetooth piconets around you |
| [Packets](net-classic-packets.md) | `Classic 2`: one piconet, packet by packet |
| [Both ends](net-classic-bench.md) | `Classic 3`: each device of a piconet on its own row |

**Wi-Fi**

| Page | What's in it |
|---|---|
| [Channel survey](net-wifi.md) | `Channel 1`: how much of each channel's time is taken |
| [Frames](net-wifi-frames.md) | `Channel 2`: one channel's traffic, frame by frame |
| [Airtime](net-wifi-airtime.md) | `Channel 3`: what a channel's time went on, second by second |

## Recording

| Page | What's in it |
|---|---|
| [Recording the IQ stream](recording.md) | `Ctrl+R`, the SigMF files it writes, and how a recording says what it lost |
| [The sdrtop SigMF extension](sdrtop.sigmf-ext.md) | Every field sdrtop writes beyond SigMF's core, for tools that read them |

## Hardware

**[Supported hardware](hardware.md)** has every radio at a glance. Each one has a
page of its own:

| Radio | What's in it |
|---|---|
| [HackRF One and PortaPack H4M](hackrf.md) | The reference radio, every diagnostic |
| [RTL-SDR](rtl-sdr.md) | One tuner gain plus AGC, and a word on the clones |
| [Airspy R2](airspy-r2.md) | Measured gain tables, Airspy's curves, the tuner's ceiling |
| [Airspy HF+ Discovery](airspy-hf.md) | The radio that manages its own levels |
| [SDRplay RSP1B](sdrplay.md) | The radio behind a service, its measured gain and the mixer trap |
| [tinySA](tinysa.md) | Calibrated power traces, not IQ |
| [SoapySDR](soapysdr.md) | Everything else, and the honest version of what that means |
| [Host platforms](platforms.md) | Linux, the Raspberry Pi, macOS unofficially, and the libraries sdrtop loads |

## Making it yours

| Page | What's in it |
|---|---|
| [Configuration](config.md) | The config file, markers, the sweep band |
| [Layout presets](presets.md) | The twenty-eight built-in layouts, and writing your own |
| [Themes](themes.md) | The six palettes, per-field overrides, and writing your own |

## Help

| Page | What's in it |
|---|---|
| [Troubleshooting](troubleshooting.md) | When it doesn't work |
| [Changelog](../CHANGELOG.md) | What changed, version by version |

---

Each fact lives in exactly one of these pages, and the others link to it. If you
find the same thing explained two different ways, that's a bug in the docs, and
worth an issue as much as a bug in the code.
