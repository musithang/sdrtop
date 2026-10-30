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
  <a href="user_docs/hardware.md#host-platforms"><img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&logoColor=black" alt="Linux"></a>
  <a href="user_docs/hardware.md#macos-unofficially"><img src="https://img.shields.io/badge/macOS-unofficial-000000?logo=apple&logoColor=white" alt="macOS, unofficial"></a>
  <img src="https://img.shields.io/badge/stage-early%20development-red" alt="Early development">
</p>

<p align="center">
  <a title="This tool is Tool of The Week on Terminal Trove, The $HOME of all things in the terminal" href="https://terminaltrove.com/"><img src="https://cdn.terminaltrove.com/media/badges/tool_of_the_week/png/terminal_trove_tool_of_the_week_green_on_dark_grey_bg.png" alt="Terminal Trove Tool of The Week" width="150" /></a>
</p>

<p align="center">
  <sub>This project is supported by</sub><br>
  <a href="https://airspy.com/"><b>Airspy</b></a> &nbsp;·&nbsp; <a href="https://www.sdrplay.com/"><b>SDRplay</b></a>
</p>

<p align="center">
  <b>Tested on real hardware</b><br>
  <a href="https://greatscottgadgets.com/hackrf/one/"><img src="https://img.shields.io/badge/HackRF%20One-brightgreen" alt="HackRF One"></a>
  <a href="https://www.rtl-sdr.com/"><img src="https://img.shields.io/badge/RTL--SDR-brightgreen" alt="RTL-SDR"></a>
  <a href="user_docs/hardware.md#airspy-r2"><img src="https://img.shields.io/badge/Airspy%20R2-brightgreen" alt="Airspy R2"></a>
  <a href="user_docs/airspy-hf.md"><img src="https://img.shields.io/badge/Airspy%20HF%2B%20Discovery-brightgreen" alt="Airspy HF+ Discovery"></a>
  <a href="https://github.com/portapack-mayhem/mayhem-firmware"><img src="https://img.shields.io/badge/PortaPack%20H4M-brightgreen" alt="PortaPack H4M"></a>
  <a href="user_docs/hardware.md#tinysa"><img src="https://img.shields.io/badge/tinySA%20Ultra-brightgreen" alt="tinySA Ultra"></a>
</p>

<p align="center">
  <b>Via SoapySDR</b> <img src="https://img.shields.io/badge/BETA-orange" alt="Beta"><br>
  <a href="user_docs/hardware.md#soapysdr-the-honest-version"><img src="https://img.shields.io/badge/SoapySDR-0.8%20%7C%200.9-informational" alt="SoapySDR 0.8 | 0.9"></a>
  <a href="user_docs/hardware.md#soapysdr-the-honest-version"><img src="https://img.shields.io/badge/Airspy%20Mini-lightgrey" alt="Airspy Mini"></a>
  <a href="user_docs/hardware.md#soapysdr-the-honest-version"><img src="https://img.shields.io/badge/SDRplay%20RSP-lightgrey" alt="SDRplay RSP"></a>
  <a href="user_docs/hardware.md#soapysdr-the-honest-version"><img src="https://img.shields.io/badge/PlutoSDR-lightgrey" alt="PlutoSDR"></a>
  <a href="user_docs/hardware.md#soapysdr-the-honest-version"><img src="https://img.shields.io/badge/LimeSDR-lightgrey" alt="LimeSDR"></a>
  <a href="user_docs/hardware.md#soapysdr-the-honest-version"><img src="https://img.shields.io/badge/bladeRF-lightgrey" alt="bladeRF"></a>
  <a href="user_docs/hardware.md#soapysdr-the-honest-version"><img src="https://img.shields.io/badge/USRP-lightgrey" alt="USRP"></a>
  <br>
  <sub>Grey means <b>written from the API, not from owning one</b>. It should work. Nobody has told me either way yet, which is exactly as reassuring as it sounds. <a href="user_docs/hardware.md#soapysdr-the-honest-version">The honest version</a>.</sub>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#keys">Keys</a> ·
  <a href="#config">Config</a> ·
  <a href="#roadmap">Roadmap</a> ·
  <a href="#supported-hardware">Hardware</a> ·
  <a href="CHANGELOG.md">Changelog</a> ·
  <a href="user_docs/README.md">Full guide</a>
</p>

**Hey there! This is my take on a terminal monitor for SDR hardware.** I wanted something that could hunt down every bit of diagnostic data from your radio and stream it straight to your terminal. Not a lazy `*-info` clone: raw, real-time metrics with a spectrum, a waterfall and a set of bench instruments, in a tmux pane, an SSH session, or the postage-stamp screen of a cyberdeck. I set out to print a few numbers. There is now a Friis noise figure model in here. I'm not entirely sure how that happened.

It's a hobby project built in my spare time, and honestly, I made it for *you* ❤️. Use it however you like, beat on it, and don't be shy: open issues, dig through the code, and if you've got a good idea, send it my way as a pull request or just a message. This is an open table, not my private garage.

https://github.com/user-attachments/assets/af278500-24ff-4d98-8129-b421fd513a49

<sub>It's a terminal app, so brace yourself for the visual spectacle of monospace text in color. The only special effects are honest dBFS numbers.</sub>

> [!IMPORTANT]
> **Early development.** New features keep coming, and each one goes the same way: built, measured against the specification, then polished until its screen reads at a glance. Between features the work is sharper radio math and bug fixing.
>
> Known issues: plenty 😄 If something looks broken, it's either a bug or an undocumented feature. Flip a coin, then open an issue.

---

## Gallery

**Piconet follow** · classic Bluetooth<br>
<sub>One piconet, found by the access code at the start of every packet and read packet by packet. [How it works](user_docs/net-classic.md)</sub>

https://github.com/user-attachments/assets/a9c554c0-bb78-4ac9-9970-b569650d4d08

**RDS demodulation** · FM broadcast<br>
<sub>Station name, programme type and RadioText, read off the 57 kHz subcarrier. No audio, on purpose. [How it works](user_docs/demodulator.md#rds-in-two-halves)</sub>

https://github.com/user-attachments/assets/0586cbb9-a0e7-48e0-81b5-664c7c9cfd5a

<sub>The clips are snapshots of the day I recorded them; the screens keep moving.</sub>

<details>
  <summary><b>📻 More screenshots</b>: lab benches, RTL-SDR, the Command Rail cards</summary>
  <br>
  <table>
    <tr>
      <td width="50%"><img src="https://raw.githubusercontent.com/musithang/sdrtop/main/user_docs/pics/hackrf/spectrum.png" alt="HackRF: spectrum & waterfall"></td>
      <td width="50%"><img src="https://raw.githubusercontent.com/musithang/sdrtop/main/user_docs/pics/hackrf/lab_iq.png" alt="HackRF: IQ diagnostics lab"></td>
    </tr>
    <tr>
      <td width="50%"><img src="https://raw.githubusercontent.com/musithang/sdrtop/main/user_docs/pics/hackrf/lab_rf.png" alt="HackRF: RF chain lab"></td>
      <td width="50%"><img src="https://raw.githubusercontent.com/musithang/sdrtop/main/user_docs/pics/hackrf/lab_timing.png" alt="HackRF: timing lab"></td>
    </tr>
    <tr>
      <td width="50%"><img src="https://raw.githubusercontent.com/musithang/sdrtop/main/user_docs/pics/rtlsdr/rtl-sdr1.png" alt="RTL-SDR: spectrum & waterfall"></td>
      <td width="50%"><img src="https://raw.githubusercontent.com/musithang/sdrtop/main/user_docs/pics/rtlsdr/rtl-sdr2.png" alt="RTL-SDR: observer mode"></td>
    </tr>
  </table>
</details>

Got a clean capture on your hardware? Drop it in [`user_docs/pics/`](https://github.com/musithang/sdrtop/tree/main/user_docs/pics) and send a PR. Shots from radios I don't own are the most useful ones there are.

---

## What it does

Everything your radio knows about itself, in real time, without leaving the terminal. It's more than I planned. Every one of these started as "that'd be a nice little readout".

- **The Command Rail**, the default cockpit: a big frequency hero, an analog S-meter, mode cards that follow what you're doing, and SNR, power, noise figure and saturation each on its own live trace. All dials, no autopilot. It's a radio, not a self-driving car.
- **Spectrum and waterfall** with peak hold, noise floor tracking, zoom, a band-plan overlay and markers.
- **Four lab benches** for the questions a plot can't answer: is the quadrature clean, is the front end staged right, is my computer keeping up, and what *is* that signal. [The details](user_docs/lab.md).
- **A demodulator that doesn't play audio**, deliberately: FM deviation, the MPX baseband and its pilot, stereo, CTCSS, AM depth and RDS. It reads radios, it doesn't play them. [How it works](user_docs/demodulator.md).
- **IQ correction**, not just measurement: take the DC spike and the mirror image out of the stream and watch them actually go away.
- **A band sweep** wider than one window, stitched into one curve; `Enter` on a peak tunes straight to it.
- **NET: the 2.4 GHz band, measured.** Bluetooth (BLE, LE Coded and classic) and Wi-Fi, decoded and then treated like any other transmitter on the bench, each number with its uncertainty and against the specification's limits. Connections are followed from the sidelines: sdrtop never transmits and never joins anything. (First thing it taught me: the biggest airtime hog on my channel isn't Wi-Fi 6, it's a 1 Mb/s 802.11b beacon that should have retired around the time flip phones did.) [The details](user_docs/net.md).
- **Recording**: `Ctrl+R` writes the raw IQ stream as SigMF, every lost sample marked where it happened. [The details](user_docs/recording.md).
- **Small screens and shared radios**: micro views that strip each concern down to one big number, and an observer mode that waits politely when another app holds the radio.
- **Focus modes, themes and layouts**: press a panel's highlighted letter and it's yours; six themes; presets you can rearrange or write yourself. Nothing needs a mouse.

Measured the awkward way rather than the easy way. Bandwidth about the carrier, not across whatever span you happened to capture, so a mistuned radio confesses instead of faking a good number. The noise floor as a density, so the same receiver reads as the same receiver whatever the sample rate. And every lab panel marks itself **[STALE]** the moment RX stops, so a frozen number is never mistaken for a live one.

> Because the only thing worse than no data is confidently wrong data.

---

## Documentation

**[→ Full user guide](user_docs/README.md)**. This page is the short version; everything below is covered properly over there.

| | | |
|---|---|---|
| [Getting started](user_docs/getting-started.md): install & run | [Keyboard shortcuts](user_docs/keys.md): every key | [What's on screen](user_docs/screens.md): panels explained |
| [The Lab presets](user_docs/lab.md): the bench-engineer views | [Configuration](user_docs/config.md): config.toml, every field | [Advanced features](user_docs/advanced.md): workflows & limits |
| [Tips & tricks](user_docs/tips-and-tricks.md): gain, markers, workflows | [Troubleshooting](user_docs/troubleshooting.md): when things go sideways | [Supported hardware](user_docs/hardware.md): what works today |
| [Themes](user_docs/themes.md): the six palettes | [Changelog](CHANGELOG.md): version by version | [The demodulator](user_docs/demodulator.md): how it was built |
| [The NET section](user_docs/net.md): Bluetooth and Wi-Fi, measured | [Layout presets](user_docs/presets.md): the built-ins and your own | [Recording](user_docs/recording.md): the IQ stream as SigMF |

---

## Install

**Requirements:** Linux, or a Mac ([unofficially](user_docs/hardware.md#macos-unofficially): built and tested in CI, radios tested by users) · a HackRF One, an RTL-SDR, an Airspy R2, an Airspy HF+ Discovery, a tinySA, or anything SoapySDR speaks to

### The one-liner

```sh
curl -fsSL https://raw.githubusercontent.com/musithang/sdrtop/main/packaging/install.sh | sh
```

The installer tries the prebuilt binary on your machine. It compiles from source if the binary cannot run there. Native SDR libraries are optional runtime dependencies.

Add `--hackrf`, `--rtlsdr`, `--airspy` or `--airspyhf` to install the runtime for your radio. `--soapy` adds SoapySDR and its driver modules. A plain install adds none of these libraries.

<details>
<summary>Every flag it takes, and installing without root</summary>

<br>

```sh
sh install.sh --prefix ~/.local     # install under a directory, no root anywhere
sh install.sh --version v0.6.8      # a specific release instead of the latest
sh install.sh --from-source         # skip the prebuilt binary, always compile
sh install.sh --git                 # compile the main branch, live dangerously
sh install.sh --no-verify           # skip the checksum check (say why first)
sh install.sh --hackrf              # add the HackRF runtime
sh install.sh --rtlsdr              # add the RTL-SDR runtime
sh install.sh --airspy              # add the Airspy R2 runtime
sh install.sh --airspyhf            # add the Airspy HF+ Discovery runtime
sh install.sh --soapy               # add SoapySDR and its driver modules
sh install.sh --deps-only --hackrf  # only the selected runtime, no sdrtop
sh install.sh --uninstall           # remove what a previous run installed
sh install.sh --help                # this list, from the script itself
```

Piped straight into a shell they go after `sh -s --`. `--no-verify` turns off the checksum check on a download, which is the one thing standing between you and a tarball that isn't the one I published, so have a reason. The rest are explained in [Getting started](user_docs/getting-started.md#every-flag-it-takes).

`--deps-only` requires a runtime flag. It never installs build tools.

</details>

Piping a script into `sh` means running code you haven't read. You should read it: [`packaging/install.sh`](packaging/install.sh). I'd want to.

### On Arch, from the AUR

sdrtop is on the [AUR](https://aur.archlinux.org/packages?K=sdrtop), packaged by [Ateles](https://aur.archlinux.org/account/Ateles). Thank you, I owe you a beer. There are three packages. Pick one:

```sh
yay -S sdrtop-bin    # the release tarball, nothing to compile
yay -S sdrtop        # the release tag, compiled on your machine
yay -S sdrtop-git    # the main branch, compiled, live dangerously
```

`paru` or a plain `makepkg -si` does the same job. Each package also installs the man page and the docs.

The radio runtimes are optional dependencies: `hackrf`, `rtl-sdr` and `soapysdr`. The Airspy R2's `airspy` is not on that list yet, so for an R2 it's `sudo pacman -S airspy`. The HF+ Discovery's library is not in the official repos at all: `yay -S airspyhf-git` from the AUR. SoapySDR alone finds no radios: its driver modules are `soapyrtlsdr`, `soapyhackrf`, `soapyairspy` and `soapyplutosdr` in the official repos, and `soapysdrplay3-git` in the AUR for an RSP.

Use the AUR or the one-liner, not both. The one-liner installs to `/usr/local/bin`, which comes before the package's `/usr/bin` on the PATH, so that copy runs even after the package has moved on.

### Or cargo, the boring one that always works

sdrtop is on [crates.io](https://crates.io/crates/sdrtop). Cargo compiles it for your machine.

```sh
sudo apt install build-essential    # Debian / Ubuntu / Mint
sudo pacman -S base-devel           # Arch / Manjaro
sudo dnf install gcc                # Fedora

cargo install sdrtop --locked
```

Building needs Rust 1.88+ and a C compiler/linker. Install Rust with [rustup](https://rustup.rs). No SDR libraries, development headers or pkg-config are required. At runtime, HackRF needs libhackrf 2023.01.1+, RTL-SDR needs librtlsdr and the Airspy R2 needs libairspy. A missing or incompatible library disables only its own backend, so install the runtimes for the radios you have (a tinySA needs none of them).

Then go make coffee: a few minutes on a laptop, considerably more on a Raspberry Pi. It's not frozen, it's just Rust.

### For SoapySDR devices

Beta, see [Hardware](#supported-hardware). Neither of these is needed to build or run sdrtop; it looks for them at startup and shrugs if they aren't there.

```sh
sudo apt install libsoapysdr0.8 soapysdr-tools soapysdr-module-all
SoapySDRUtil --find     # if this can't see your radio, sdrtop can't either
```

That last command is the whole diagnostic. If your radio isn't in that list, the missing piece is a driver module, and no amount of shouting at sdrtop will conjure one up.

<sub><a href="user_docs/getting-started.md#the-prebuilt-tarball-by-hand">Getting started</a> covers the release tarball, checksums and provenance attestation. The tarball needs x86_64 Linux with glibc 2.36+. Runtime loading supports both Debian's <code>librtlsdr.so.0</code> and Ubuntu's <code>librtlsdr.so.2</code>.</sub>

**First run:** sdrtop opens on its menu. `Enter` takes a layout, `Space` starts receiving, `Esc` brings the menu back, `q` quits and saves.

---

## Keys

`Esc` opens the menu: a row of tiles (Radio, Lab, Options, Keys, Quit) with a tree under Radio and Lab. Under Lab, the views are grouped and **each group has its own numbers**: `Spectrum` for the general views, `Sweep` for the band scan, `Receiver` for the benches, `Compact` for the field views, and `Bluetooth` (`Overview`, `Low Energy`, `Classic`, `Long range`) and `Wi-Fi` (`Channel`) for the 2.4 GHz band. Under Radio, the pager. So `2` is Gain & noise inside Receiver and Full spectrum inside Spectrum. Nine keys per group, rather than one long row to memorise. A view your radio cannot do is greyed, with the reason, instead of hidden.

The ones that get you everywhere:

| Key | Action |
|---|---|
| `Esc` | Open the menu, or leave a focused panel |
| `Enter` | Open what the menu's cursor is on |
| `Space` | Start / stop RX |
| `1`–`9` | The nth view **of the group you're in** |
| `↑` / `↓` | Primary gain |
| `f` / `s` | Type a frequency / a sample rate |
| `c` / `e` / `l` | Focus the Command Rail / spectrum / waterfall |
| `Ctrl+R` | Record the IQ stream, or stop |
| `q` | Quit and save |

Capitals work everywhere: `C` and `c` do the same thing, so you never have to think about whether Shift is down. The gain keys relabel themselves per device, and a control your radio doesn't have simply isn't offered rather than sitting there doing nothing.

Forgotten one mid-session? `Esc`, then `Tab` to **Keys**. That reference is generated from the same table the app dispatches on, so it cannot go stale. Full list including every focus mode: **[Keyboard shortcuts](user_docs/keys.md)**.

---

## Config

Saved to `~/.config/sdrtop/config.toml` when you quit. Hand-editing is safe: a missing or broken file just falls back to defaults. Go ahead, mangle it; the parser has seen worse.

Frequency, gains, sample rate, markers, the sweep band, your theme and your layout all live there, plus any layouts of your own. Give a custom preset a `section` and a `slot` and it gets a place in the menu and a number key. Annotated example and every field: **[Configuration](user_docs/config.md)** and **[Layout presets](user_docs/presets.md)**.

**Themes:** `sdr` (default) · `nord` · `dracula` · `gruvbox` · `catppuccin` · `solarized`, with per-field overrides. See [Themes](user_docs/themes.md).

---

## Roadmap

**Features, then polish.** New features keep landing, and when one is done its next step is always the same: polish, and a UI that reads at a glance. Here's where things stand. ✨

### ✅ Done

Native support for the HackRF One, the RTL-SDR, the Airspy R2, the Airspy HF+ Discovery, the PortaPack H4M and the tinySA family (that last one thanks to [@AlCalzone](https://github.com/AlCalzone)). Spectrum, waterfall and the four lab benches. The FM demodulator with RDS. IQ correction, band sweeps, observer mode and SigMF recording. Bluetooth decoded and measured against the Core Specification, and the first Wi-Fi views. Release by release, it's all in the [changelog](CHANGELOG.md).

### 🔧 In progress

- **Wi-Fi**: the channel survey and the Frames view are in. Next up is who and what is actually spending a channel's airtime
- **SoapySDR feature parity**: every panel and every measurement working through a SoapySDR device the way it does on a HackRF. Most of it is there; what's left needs datasheets nobody publishes

### 🔜 Next

- A native backend for the radio still waiting on my desk: the SDRplay RSP1B
- **Digital signals done properly**, the way Bluetooth is now: LoRa, DMR, GSM, ADS-B, AIS. Each gets its own screens. Still no audio, still just data
- Editing the config from inside the app. Until then: `$EDITOR` and a little courage

---

## Supported hardware

| Device | Status | Notes |
|---|---|---|
| HackRF One | ✅ Full support | All diagnostics, gain stages, ADC metrics |
| PortaPack H4M (Mayhem) | ✅ Full support | HackRF mode: all HackRF diagnostics apply |
| RTL-SDR (R820T, E4000, R828D) | ✅ Full support | Single tuner gain + AGC; no VGA, no BB filter, no Friis NF |
| Airspy R2 | ✅ Full support | Natively: measured LNA, mixer and VGA tables, Airspy's Linearity and Sensitivity curves on the knob, tuner AGC; no BB filter, no Friis NF. [Details](user_docs/hardware.md#airspy-r2) |
| Airspy HF+ Discovery | ✅ Full support | Natively: both bands and the gap between them, all seven rates, ATT and LNA drawn as the digital gain they are, the radio's hold at about −12 dBFS; no BB filter, no Friis NF. [Details](user_docs/airspy-hf.md) |
| tinySA / Ultra / Ultra+ | ✅ Spectrum support | ZS405 verified; calibrated dBm spectrum, waterfall and native band sweeps |
| **Anything with a SoapySDR driver** | 🧪 **Beta** | Airspy Mini, SDRplay, Pluto, Lime, bladeRF, USRP, SoapyRemote. Unconfirmed on anything but a HackRF and an Airspy R2 |
| SDRplay RSP1B | 🔲 Planned | On my desk; native backend in the queue |
| HackRF Pro | 🔲 Planned | Needs hardware |

**Hardware from the people who make it:** the RSP1B came from [SDRplay](https://www.sdrplay.com/), the R2 and the HF+ Discovery from [Airspy](https://airspy.com/). Thank you both.

Native support is added only after physical testing on real devices. No guessing from datasheets. Translation: that list moves at exactly the speed of my hobby budget.

**SoapySDR is the deliberate exception**, because "I don't own one" was a bad answer to give every week, and buying one of everything is not a plan, it's a fantasy with a shipping address. Nothing about your radio is hardcoded: sdrtop asks the driver for the frequency range, the gain range, whether there's an AGC, the sample format and how many of its bits mean anything. What it can't ask, it refuses rather than invents. Verified against a HackRF through `SoapyHackRF`: the loader, enumeration, opening, capabilities and streaming. Unverified: literally every other radio. [The honest version](user_docs/hardware.md#soapysdr-the-honest-version) has the full reckoning, including the gotchas I already hit.

It took me a while to stop wincing at that. Before this, every row above was green and "tested on real hardware" applied to everything I shipped, which is another way of saying it separated nothing. The grey badges are what make the green ones mean something, and a rule nobody has ever had a reason to test is not a rule, it's a habit. This one now has one marked edge on it, on purpose.

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
