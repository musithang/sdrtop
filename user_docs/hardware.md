# Supported Hardware

← [Back](README.md)

---

Every radio has its own page. The green ones were tested on a radio on my desk;
the SoapySDR row was written from the API, and [says so out loud](soapysdr.md).

| Device | Range | Rates | Status |
|---|---|---|---|
| [HackRF One](hackrf.md) | 1 MHz to 6 GHz | 2 to 20 Msps | ✅ Every diagnostic, the reference radio |
| [PortaPack H4M](hackrf.md#portapack-h4m) | As the HackRF | As the HackRF | ✅ In HackRF mode |
| [RTL-SDR](rtl-sdr.md) | 24 MHz to 1.766 GHz (R820T / R828D), 52 MHz to 2.2 GHz (E4000) | 0.9 to 3.2 Msps | ✅ One tuner gain plus AGC |
| [Airspy R2](airspy-r2.md) | 24 MHz to 1.8 GHz | 2.5 or 10 Msps | ✅ Measured gain tables, Airspy's curves |
| [Airspy HF+ Discovery](airspy-hf.md) | 9 kHz to 31 MHz, 60 to 260 MHz | 192 to 912 kS/s | ✅ Its digital gain drawn as digital |
| [SDRplay RSP1B](sdrplay.md) | 1 kHz to 2 GHz | 62.5 kS/s to 10 Msps | ✅ Measured gain per band, SDRplay-style curves |
| [tinySA / Ultra / Ultra+](tinysa.md) | By model | Power traces, not IQ | ✅ Spectrum, waterfall and sweeps in dBm (ZS405) |
| [Anything SoapySDR drives](soapysdr.md) | The driver's | The driver's | 🧪 Beta: written from the API, not from owning one |

Every lab bench, the demodulator and the sweep scanner work the same on all the
IQ radios. What a radio does differently (its gain stages, its ceiling, what the
screen leaves out because it would mean nothing) is on its page.

**Where it runs:** Linux, the Raspberry Pi, and macOS unofficially. See
[Host platforms](platforms.md).

## What's coming

| Device | Status |
|---|---|
| Airspy Mini | Planned, needs hardware to test |
| HackRF Pro | Planned, needs hardware to test |
| LimeSDR, bladeRF, PlutoSDR, other SDRplay RSPs | Should already work [through SoapySDR](soapysdr.md). Nobody has told me yet |

Native support means owning and testing the radio, so the list moves at exactly
the speed of a hobby budget. If you'd like to move it along, the project has a
[GitHub Sponsors page](https://github.com/sponsors/musithang).

---

← [Back](README.md)
