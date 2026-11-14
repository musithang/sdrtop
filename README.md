<p align="center">
  <img src="user_docs/pics/sdrtop_logo.png" alt="sdrtop logo" width="75">
</p>

<h1 align="center">sdrtop</h1>

<p align="center">
  <b>A bench instrument for software defined radios, living in your terminal.</b><br>
  <sub>Spectrum, waterfall, and the measurements a plot cannot give you.</sub>
</p>

<p align="center">
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/rust-1.88%2B-orange?logo=rust&logoColor=white" alt="Rust 1.88+"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue" alt="GPL-3.0-or-later"></a>
  <a href="user_docs/platforms.md"><img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&logoColor=black" alt="Linux"></a>
  <a href="user_docs/platforms.md#macos-unofficially"><img src="https://img.shields.io/badge/macOS-unofficial-000000?logo=apple&logoColor=white" alt="macOS, unofficial"></a>
  <img src="https://img.shields.io/badge/stage-early%20development-red" alt="Early development">
</p>

<p align="center">
  <a title="This tool is Tool of The Week on Terminal Trove, The $HOME of all things in the terminal" href="https://terminaltrove.com/"><img src="https://cdn.terminaltrove.com/media/badges/tool_of_the_week/png/terminal_trove_tool_of_the_week_green_on_dark_grey_bg.png" alt="Terminal Trove Tool of The Week" width="150" /></a>
</p>

<p align="center">
  <sub>This project is supported by</sub><br>
  <a href="https://airspy.com/"><b>Airspy</b></a> &nbsp;·&nbsp; <a href="https://www.sdrplay.com/"><b>SDRplay</b></a> &nbsp;·&nbsp; <a href="https://opensourcesdrlab.com/"><b>OpenSourceSDRLab</b></a>
</p>

<p align="center">
  <b>Tested on real hardware</b><br>
  <a href="user_docs/hackrf.md"><img src="https://img.shields.io/badge/HackRF%20One-brightgreen" alt="HackRF One"></a>
  <a href="user_docs/rtl-sdr.md"><img src="https://img.shields.io/badge/RTL--SDR-brightgreen" alt="RTL-SDR"></a>
  <a href="user_docs/airspy-r2.md"><img src="https://img.shields.io/badge/Airspy%20R2-brightgreen" alt="Airspy R2"></a>
  <a href="user_docs/airspy-hf.md"><img src="https://img.shields.io/badge/Airspy%20HF%2B%20Discovery-brightgreen" alt="Airspy HF+ Discovery"></a>
  <a href="user_docs/sdrplay.md"><img src="https://img.shields.io/badge/SDRplay%20RSP1B-brightgreen" alt="SDRplay RSP1B"></a>
  <a href="user_docs/hackrf.md#portapack-h4m"><img src="https://img.shields.io/badge/PortaPack%20H4M-brightgreen" alt="PortaPack H4M"></a>
  <a href="user_docs/tinysa.md"><img src="https://img.shields.io/badge/tinySA%20Ultra-brightgreen" alt="tinySA Ultra"></a>
</p>

<p align="center">
  <b>Via SoapySDR</b> <img src="https://img.shields.io/badge/BETA-orange" alt="Beta"><br>
  <a href="user_docs/soapysdr.md"><img src="https://img.shields.io/badge/SoapySDR-0.8%20%7C%200.9-informational" alt="SoapySDR 0.8 | 0.9"></a>
  <a href="user_docs/soapysdr.md"><img src="https://img.shields.io/badge/Airspy%20Mini-lightgrey" alt="Airspy Mini"></a>
  <a href="user_docs/soapysdr.md"><img src="https://img.shields.io/badge/SDRplay%20RSP-lightgrey" alt="SDRplay RSP"></a>
  <a href="user_docs/soapysdr.md"><img src="https://img.shields.io/badge/PlutoSDR-lightgrey" alt="PlutoSDR"></a>
  <a href="user_docs/soapysdr.md"><img src="https://img.shields.io/badge/LimeSDR-lightgrey" alt="LimeSDR"></a>
  <a href="user_docs/soapysdr.md"><img src="https://img.shields.io/badge/bladeRF-lightgrey" alt="bladeRF"></a>
  <a href="user_docs/soapysdr.md"><img src="https://img.shields.io/badge/USRP-lightgrey" alt="USRP"></a>
  <br>
  <sub>Grey means <b>written from the API, not from owning one</b>. It should work. Nobody has told me either way yet, which is exactly as reassuring as it sounds. <a href="user_docs/soapysdr.md">The honest version</a>.</sub>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#supported-hardware">Hardware</a> ·
  <a href="user_docs/README.md">Full guide</a> ·
  <a href="CHANGELOG.md">Changelog</a>
</p>

**Hey there! This is my take on a terminal monitor for SDR hardware.** Raw, real-time diagnostics from your radio, with a spectrum, a waterfall and a set of bench instruments, in a tmux pane, an SSH session, or the postage-stamp screen of a cyberdeck. I set out to print a few numbers. There is now a Friis noise figure model in here. I'm not entirely sure how that happened.

It's a hobby project, and honestly, I made it for *you* ❤️. Beat on it, open issues, send ideas and pull requests. This is an open table, not my private garage.

https://github.com/user-attachments/assets/af278500-24ff-4d98-8129-b421fd513a49

<sub>It's a terminal app, so brace yourself for the visual spectacle of monospace text in color. The only special effects are honest dBFS numbers.</sub>

> [!IMPORTANT]
> **Early development.** New features keep coming, each one measured against the specification and then polished until it reads at a glance. Known issues: plenty 😄 If something looks broken, it's either a bug or an undocumented feature. Flip a coin, then open an issue.

**Piconet follow** · classic Bluetooth<br>
<sub>One piconet, found by the access code at the start of every packet and read packet by packet. [How it works](user_docs/net-classic.md)</sub>

https://github.com/user-attachments/assets/a9c554c0-bb78-4ac9-9970-b569650d4d08

**RDS demodulation** · FM broadcast<br>
<sub>Station name, programme type and RadioText, read off the 57 kHz subcarrier. No audio, on purpose. [How it works](user_docs/demodulator.md#rds-in-two-halves)</sub>

https://github.com/user-attachments/assets/0586cbb9-a0e7-48e0-81b5-664c7c9cfd5a

---

## What it does

- **Spectrum and waterfall**, plus the Command Rail cockpit: frequency, S-meter, SNR, noise figure and saturation on live traces.
- **Four lab benches** for what a plot can't answer: quadrature, front-end staging, timing, and what *is* that signal. [Details](user_docs/lab.md)
- **A demodulator that reads radios instead of playing them**: FM deviation, pilot, stereo, CTCSS, AM depth, RDS. No audio, on purpose. [Details](user_docs/demodulator.md)
- **NET: the 2.4 GHz band, measured.** Bluetooth (BLE, LE Coded, classic) and Wi-Fi, decoded and held against the specification. Never transmits, never joins. [Details](user_docs/net.md)
- **IQ correction, band sweeps, SigMF recording, observer mode** for a radio another app holds, and every lab panel marked **[STALE]** the moment its data stops being live.

> Because the only thing worse than no data is confidently wrong data.

Everything else, every key and every config field, is in the **[full user guide](user_docs/README.md)**.

---

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/musithang/sdrtop/main/packaging/install.sh | sh
```

The installer takes the prebuilt binary, or compiles from source when that can't run on your machine. Add your radio's runtime with `sh -s -- --hackrf`, `--rtlsdr`, `--airspy`, `--airspyhf` or `--soapy`; `--sdrplay` checks SDRplay's API and service. Piping a script into `sh` means running code you haven't read, so read it first: [`packaging/install.sh`](packaging/install.sh). I'd want to.

**On Arch**, from the [AUR](https://aur.archlinux.org/packages?K=sdrtop):

```sh
yay -S sdrtop-bin    # or sdrtop, or sdrtop-git
```

**With cargo**, anywhere with Rust 1.88+ and a C compiler:

```sh
cargo install sdrtop --locked
```

No SDR library is needed to build; each radio's library is loaded at runtime, and a missing one disables only its own backend. Every flag, every distribution and the tarball by hand: **[Getting started](user_docs/getting-started.md)**.

**First run:** sdrtop opens on its menu. `Enter` takes a layout, `Space` starts receiving, `Esc` brings the menu back, `q` quits and saves. Every key: [Keyboard shortcuts](user_docs/keys.md).

---

## Supported hardware

| Device | Status |
|---|---|
| [HackRF One](user_docs/hackrf.md), [PortaPack H4M](user_docs/hackrf.md#portapack-h4m) | ✅ Tested |
| [RTL-SDR](user_docs/rtl-sdr.md) | ✅ Tested |
| [Airspy R2](user_docs/airspy-r2.md) | ✅ Tested |
| [Airspy HF+ Discovery](user_docs/airspy-hf.md) | ✅ Tested |
| [SDRplay RSP1B](user_docs/sdrplay.md) | ✅ Tested |
| [tinySA / Ultra / Ultra+](user_docs/tinysa.md) | ✅ Spectrum (ZS405 tested) |
| [Anything with a SoapySDR driver](user_docs/soapysdr.md) | 🧪 Beta, written from the API |
| PlutoSky R2 (AD9361) | 🔲 On my desk, native backend in the queue |

Native support lands only after testing on a real radio. The RSP1B came from [SDRplay](https://www.sdrplay.com/), the R2 and the HF+ Discovery from [Airspy](https://airspy.com/), and the PlutoSky R2 from [OpenSourceSDRLab](https://opensourcesdrlab.com/). Thank you all. How the radios differ: **[Supported hardware](user_docs/hardware.md)**.

---

<details>
<summary>📡 <i>(pst… you scrolled this far, might as well)</i></summary>

<br>

```
                .
               /=\
          (    |#|    )
         ((    |#|    ))
        (((    |#|    )))
         ((    |#|    ))
          (    |#|    )
               |#|
              /|#|\
             / |#| \
            /  |#|  \
           /___|#|___\
          /    |#|    \
         '''''''''''''''
```

Yep, it's a radio tower. I'm a simple man, I see free time, I make ASCII art.

**73 de sdrtop**: ham-speak for "catch you later." 📻

</details>

---

**[Credits](CREDITS.md)** · **[Contributing](CONTRIBUTING.md)** · **[License](LICENSE)**: GPL-3.0-or-later
