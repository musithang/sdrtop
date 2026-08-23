# sdrtop

[![Rust](https://img.shields.io/badge/rust-stable-orange?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-GPL--3.0-blue)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-linux-lightgrey?logo=linux&logoColor=white)]()
[![Development Stage](https://img.shields.io/badge/stage-early%20development-red)]()

[![HackRF One](https://img.shields.io/badge/hardware-HackRF%20One-brightgreen)](https://greatscottgadgets.com/hackrf/one/)
[![RTL-SDR](https://img.shields.io/badge/hardware-RTL--SDR-green)](https://www.rtl-sdr.com/)
[![PortaPack](https://img.shields.io/badge/hardware-PortaPack%20H4M-blueviolet)](https://github.com/portapack-mayhem/mayhem-firmware)

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#keys">Keys</a> ·
  <a href="#config">Config</a> ·
  <a href="#supported-hardware">Hardware</a>
</p>

**Hey there! This is my take on a terminal monitor for SDR hardware.** I wanted something that could hunt down every bit of diagnostic data from your radio and stream it straight to your terminal.

I didn't want to cut corners, so this definitely isn't just a lazy hardware info tool clone. It delivers raw, real-time metrics (spectrum, waterfall, ADC health, gain chain) right inside the terminal. It's lightweight, distraction-free, and fits perfectly into a tmux pane, an SSH session, or the custom screen of your cyberdeck.

It's a hobby project built in my spare time, and honestly, I made it for *you* ❤️. Use it however you like, beat on it, and don't be shy: open issues, dig through the code, and if you've got a good idea, send it my way as a pull request or just a message. This is an open table, not my private garage.

> [!IMPORTANT]
> **Project status: early development.**
>
> * **Hardware:** the **HackRF One** and the **RTL-SDR** (R820T / R828D / E4000) are both fully supported, in normal RX and in observer mode. RTL clones vary wildly, though, and nobody owns all of them, so if yours behaves oddly please [open an issue](../../issues).
> * **Software:** the **interactive TUI is feature-complete** (spectrum, waterfall, the IQ / RF / timing / signal / sweep lab presets, and the micro field views). The focus now is **polishing the UI, sharpening the radio math, and fixing bugs**, not piling on features. The most recent pass went through the Lab Signal bench reading by reading asking "is this number actually true?", and several were not.
> * **Known issues:** plenty 😄 If something looks broken, it's either a bug or an undocumented feature. Flip a coin, then open an issue.

### 📖 Documentation

**[→ Full user guide](user_docs/README.md)**: everything below, in depth. Or jump straight to:

| | | |
|---|---|---|
| [Getting started](user_docs/getting-started.md): install & run | [Keyboard shortcuts](user_docs/keys.md): every key | [What's on screen](user_docs/screens.md): panels explained |
| [The Lab presets](user_docs/lab.md): the bench-engineer views | [Configuration](user_docs/config.md): config.toml & custom layouts | [Advanced features](user_docs/advanced.md): workflows & limits |
| [Tips & tricks](user_docs/tips-and-tricks.md): gain, markers, workflows | [Troubleshooting](user_docs/troubleshooting.md): when things go sideways | [Supported hardware](user_docs/hardware.md): what works today |
| [Themes](user_docs/themes.md): the six palettes | [What's new](user_docs/whats-new.md): the checkpoint log | |

---

## Gallery

<p align="center">
  <a href="user_docs/pics/hackrf/video.mp4">
    <img src="user_docs/pics/hackrf/command_rail.png" width="100%" alt="sdrtop in motion: click to watch the demo video">
  </a>
  <br>
  <sub>▶ click the screenshot to play the demo video</sub>
</p>

*It's a terminal app, so brace yourself for the visual spectacle of monospace text in color. The only special effects are honest dBFS numbers.*

Screenshots, split by device. More to come. Got a clean capture on your hardware? Drop it in [`user_docs/pics/`](user_docs/pics/) and send a PR (RTL-SDR shots from different tuners especially welcome).

<details open>
  <summary><b>📻 HackRF One</b>: spectrum, waterfall &amp; lab presets</summary>
  <br>
  <table>
    <tr>
      <td width="50%"><img src="user_docs/pics/hackrf/command_rail.png" alt="HackRF: Command Rail cockpit"></td>
      <td width="50%"><img src="user_docs/pics/hackrf/spectrum.png" alt="HackRF: spectrum & waterfall"></td>
    </tr>
    <tr>
      <td width="50%"><img src="user_docs/pics/hackrf/lab_iq.png" alt="HackRF: IQ diagnostics lab"></td>
      <td width="50%"><img src="user_docs/pics/hackrf/lab_rf.png" alt="HackRF: RF chain lab"></td>
    </tr>
    <tr>
      <td width="100%" colspan="2"><img src="user_docs/pics/hackrf/lab_timing.png" alt="HackRF: timing lab"></td>
    </tr>
  </table>
</details>

<details>
  <summary><b>📡 RTL-SDR</b>: spectrum, waterfall &amp; observer mode</summary>
  <br>
  <table>
    <tr>
      <td width="50%"><img src="user_docs/pics/rtlsdr/rtl-sdr1.png" alt="RTL-SDR: spectrum & waterfall"></td>
      <td width="50%"><img src="user_docs/pics/rtlsdr/rtl-sdr2.png" alt="RTL-SDR: observer mode"></td>
    </tr>
  </table>
</details>

---

## What it shows

Everything your radio knows about itself, in real time, without leaving the terminal.

### The Command Rail: the default cockpit (`1`)
The view sdrtop opens on. A slim header plus a left **instrument rail** that packs what a poweruser reads at a glance: a big segmented **frequency hero**, an analog **S-meter**, the **HUNT · MONITOR · BENCH** mode tabs whose lead card follows what you're doing, **recall slots** with live activity pips, and a SIGNAL zone where SNR · PWR · NF · SAT each ride their own little braille oscilloscope trace beside the value. Gain and stream health round it out, and the bonded spectrum + waterfall fill the rest. Press `c` to drive it, `←/→` to tune. All dials, no autopilot. It's a radio, not a self-driving car.

<details>
  <summary><b>🛰️ Command Rail</b>: the BENCH &amp; HUNT mode cards</summary>
  <br>
  <table>
    <tr>
      <td width="50%"><img src="user_docs/pics/annotate-2026-06-22_22-42-55.png" alt="HackRF: bench"></td>
      <td width="50%"><img src="user_docs/pics/annotate-2026-06-22_22-43-06.png" alt="HackRF: hunt"></td>
    </tr>
  </table>
</details>

<p align="center"><sub>· · ·</sub></p>

### Live spectrum & waterfall
- **Spectrum analyzer**: FFT with EMA smoothing, peak hold, noise floor tracking, dBFS axis, zoom, band-plan overlay, and persistent frequency markers
- **Waterfall**: scrolling spectrogram in truecolor / 256-color / 16-color, with adjustable color scale, history scroll-back, and frame averaging for longer time windows
- **Focus modes**: press the highlighted letter in a panel's title to take it over. `e` spectrum, `l` waterfall, `c` the Command Rail, plus cursor read-outs, holds, and markers without ever touching the mouse

<p align="center"><sub>· · ·</sub></p>

### Bench-engineer measurements (the Lab presets)
- **Demodulator**: not for listening. There is no audio anywhere in sdrtop, and that's the point. This one opens the channel up and reports what a spectrum plot structurally cannot: **FM deviation** (peak and RMS, measured about the carrier, so a mistuned radio confesses to being mistuned instead of faking modulation), the **MPX baseband** with its 19 kHz pilot and stereo injection percentage, **CTCSS** tones for narrowband voice, **AM depth** with positive and negative peaks kept apart, and **RDS**: station name, PI code, programme type and RadioText, decoded off the 57 kHz subcarrier, accents and all
- **Signal characterization**: what *is* that, and how clean is it? Modulation class, SNR, channel power, 99 % **occupied bandwidth** (ITU-R SM.328, measured over the carrier rather than the captured span), **ACPR** against the neighbouring channels at the right spacing for the mode, and a noise floor given both per bin and as a **density** in dBFS/Hz, so the same receiver reads as the same receiver whatever the sample rate
- **RF chain**: tuned frequency with wavelength (λ, λ/4 for cutting antennas), a visual gain chain, estimated **noise figure** (Friis), **minimum detectable signal** (MDS) in dBm, an ADC-utilisation gauge, and a gain advisor that tells you when you're starving or clipping the front end
- **IQ diagnostics**: DC offset, amplitude/phase imbalance and **image rejection ratio** (IRR), drawn as analog **null-meters** (centre is ideal, the needle shows the deviation). Paired with a **persistence constellation**, a phosphor-style I/Q cloud coloured by density with a fitted imbalance ellipse whose stretch reads amplitude imbalance and whose tilt reads phase imbalance, and an **image scope** that measures a carrier against its own mirror to check the computed IRR against a real one
- **IQ correction**: not just measurement. A keypress subtracts the live DC estimate from the stream, another captures a quadrature correction and applies it, so you can watch the DC spike and the mirror images actually go away
- **Measurement banner**: the spectrum-analyser workflow on every lab preset. Set a **reference level**, dial in **trace averaging** to pull a signal out of the noise, capture a **reference trace**, and read the marker bar's delta against it
- **IQ histogram**: ADC amplitude distribution with a Low/Mid/Clip breakdown and **PAPR** (crest factor) that fingerprints the signal type at a glance
- **Timing**: USB transfer cadence, throughput, and jitter with a quality verdict and session peak tracking
- **Hardware vitals**: drops, ADC saturation, sdrtop's own CPU/RAM, USB errors, configured-vs-measured sample rate, and buffer fill, every one with a trend sparkline

<p align="center"><sub>· · ·</sub></p>

### Scanning & field views
- **Frequency sweep**: scan a band wider than one window can show. sdrtop retunes across it, stitches the result into a single curve with band-plan labels, and lets you press `Enter` on a peak to tune straight to it
- **Micro field views**: the deliberately tiny mode (`0`). The idea is that sdrtop shouldn't need a full terminal to be useful. When it's squeezed into a slim tmux split, an SSH session on a phone, or the postage-stamp screen of a cyberdeck, the full panels stop being readable, so the micro views strip each concern down to a single glance (overview · signal · gain · health · sweep) and let you cycle between them. One number that matters, big enough to read across the room. *(Heads up: the looks are still cooking. The idea's solid, the pixels are a work in progress.)*
- **Signal strip**: one live bar with the essentials. P/NF · channel power · noise floor · ADC saturation · drops · buffer fill · IQ imbalance · RBW
- **Observer mode**: if another app already holds the radio, sdrtop shows device identity, the owning process, and USB stats instead of falling over, then reclaims the device when it's free

<p align="center"><sub>· · ·</sub></p>

### Make it yours
- **Six themes**: `sdr` · `nord` · `dracula` · `gruvbox` · `catppuccin` · `solarized`
- **Layout presets**: general + specialised lab layouts. Switch on the fly with the number keys, cycle with `p`, or define your own in the config out of any panel sdrtop draws

> Every lab panel marks itself **[STALE]** the moment RX stops, so a frozen number is never mistaken for a live one. Because the only thing worse than no data is confidently wrong data.

---

## Quick start

**Requirements:** Linux · HackRF One *or* RTL-SDR · `libhackrf` + `librtlsdr` + `pkg-config` · a recent Rust stable (1.78+)

Both libraries are needed at build time even if you only own one radio; at runtime sdrtop is happy with whichever you plug in.

### Arch

```sh
sudo pacman -S hackrf rtl-sdr pkgconf rust
```

### Debian / Ubuntu

```sh
sudo apt install libhackrf-dev librtlsdr-dev pkg-config
```

Other distributions are covered in [Getting started](user_docs/getting-started.md). You also need Rust; if you don't have it yet:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Distro Rust packages are often too old to read this repo's lockfile. If the build stops with something about `lock file version 4`, that's the one; rustup fixes it.

### Then build

```sh
cargo build --release
./target/release/sdrtop
```

Press `Space` to start receiving. Press `?` for the key reference. Press `q` to quit and save.

---

## Keys

| Key        | Action                         |
| ---------- | ------------------------------ |
| `Space`    | Start / stop RX                |
| `↑` / `↓` | Primary gain: HackRF LNA ±8 dB, RTL-SDR tuner step |
| `[` / `]`  | VGA gain ±2 dB (HackRF only)   |
| `a`        | Toggle RF amplifier / tuner AGC |
| `f`        | Enter frequency (MHz)          |
| `s`        | Enter sample rate (HackRF 2–20 MHz · RTL-SDR 0.9–3.2 MHz) |
| `r`        | Reset all settings to defaults |
| `w`        | Pause / resume waterfall       |
| `h`        | Hold / unhold spectrum frame   |
| `c` / `e` / `l` | Focus the Command Rail / spectrum / waterfall |
| `i` / `d` / `t` / `v` / `x` / `m` / `n` / `g` / `b` | Focus a lab panel: IQ · RF · timing · vitals · signal · demod · metrics · sweep · measurement banner |
| `1`–`4`    | Layout presets: Command Rail · spectrum · waterfall · both |
| `5`–`9`    | Lab presets: IQ · RF · timing · signal · sweep |
| `0`        | Micro field-mode view (compact; cycles overview → signal → gain → health → sweep) |
| `p`        | Cycle presets                  |
| `Tab`      | Toggle footer bar              |
| `?`        | Help overlay                   |
| `q`        | Quit and save config           |

> Capitals work everywhere: `C` and `c` do the same thing, so you never have to think about whether Shift is down. The only place case matters is when you're *typing*, like a marker name.

> **On an RTL-SDR** the gain keys adapt to the hardware: `↑`/`↓` step the tuner's single discrete gain table and `a` toggles tuner **AGC** (there's no separate VGA stage, so `[`/`]` sit out). The UI relabels itself accordingly, no muscle memory to relearn. With more than one radio plugged in, a picker appears at launch; pin one with `--device hackrf|rtlsdr`.

Full reference, including every focus mode: **[Keyboard shortcuts](user_docs/keys.md)**.

---

## Config

Everything is saved automatically to `~/.config/sdrtop/config.toml` when you quit, and hand-editing is safe: a missing or broken file just falls back to defaults. Go ahead, mangle it; the parser has seen worse.

```toml
[radio]
frequency_hz = 92800000      # tuned center frequency
sample_rate  = 2000000.0     # HackRF 2–20 MHz · RTL-SDR 0.9–3.2 MHz
lna_gain     = 24            # HackRF LNA / RTL-SDR tuner gain
vga_gain     = 30            # HackRF only (ignored on RTL-SDR)
amp_enabled  = false         # HackRF RF amp / RTL-SDR tuner AGC

[display]
active_preset      = "command_rail"
waterfall_max_rows = 64

# Spectrum markers persist here, one block each
[[display.spectrum_markers]]
freq_hz = 92800000
label   = "FM Radio"

[sweep]
start_hz = 400000000         # frequency scanner: band start
stop_hz  = 500000000         # band end
dwell_ms = 200               # measure time per step (50–2000)

[theme]
base = "nord"
# optional per-field overrides:
# border_accent = "#88c0d0"
# value_hi      = "#ebcb8b"
```

**Themes:** `sdr` (default) · `nord` · `dracula` · `gruvbox` · `catppuccin` · `solarized`. See [Themes](user_docs/themes.md).

**Custom layouts:** define your own `[presets.*]` blocks and they merge with the built-ins, surviving every save. Full reference in [Configuration](user_docs/config.md#custom-layout-presets).

---

## Supported hardware

| Device                                 | Status            | Notes                                     |
| -------------------------------------- | ----------------- | ----------------------------------------- |
| HackRF One                             | ✅ Full support    | All diagnostics, gain stages, ADC metrics |
| RTL-SDR (R820T, E4000, R828D)          | ✅ Full support    | Single tuner gain + AGC; no VGA, no BB filter, no Friis NF |
| PortaPack H4M (Mayhem)                 | 🔧 In development | Telemetry panel via CDC/ACM serial        |
| Airspy Mini / Airspy HF+               | 🔲 Planned        | Needs hardware                            |
| HackRF Pro                             | 🔲 Planned        | Needs hardware                            |
| LimeSDR / bladeRF / SDRplay / PlutoSDR | 🔲 Planned        | Needs hardware                            |

> Hardware support is added only after physical testing on real devices. No guessing from datasheets. (Translation: the list moves at exactly the speed of my hobby budget.)

### The hardware wishlist

Every device on the planned list needs to physically exist on my desk before it gets a backend, and development here runs on a HackRF One and a PortaPack H4M. If you use `sdrtop` and want your device supported sooner, contributions go straight into buying it. Every radio that arrives gets a proper backend: tested on real hardware, documented, shipped.

| Device               | Why it matters                                                     | Price |
| -------------------- | ------------------------------------------------------------------ | ----- |
| RTL-SDR Blog V4      | The backend is in; a unit here would let me test the clone variants | ~€25  |
| Airspy Mini          | Clean 24–1700 MHz, popular with hams and scanner hobbyists          | ~€80  |
| Airspy HF+ Discovery | Best budget HF receiver, dedicated listener community               | ~€150 |
| LimeSDR Mini 2.0     | Full-duplex, wide range, opens up SoapySDR for dozens of devices    | ~€160 |

No pressure, but if this scratches an itch for you, this is where it goes.

[![Ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/mustang6139)

---

## Roadmap

### Right now: polish over features
The feature set is in. So the whole focus has shifted to **making what's already here genuinely good**:

- [ ] **UI polish**: layout, spacing, color, readability, and the small edge cases that make a TUI feel hand-built instead of merely functional
- [ ] **Micro view redesign**: the field views (`0`) do their job, but the layout deserves a rethink. Bigger, calmer, easier to read at a glance on a tiny screen
- [ ] **Sharper radio math**: auditing and refining the derived measurements (NF, MDS, IRR, PAPR, sample-rate accuracy, timing) so the numbers are not just present but *trustworthy*
- [ ] **Bug fixes**: hunting down the rough edges before piling on anything new

No shiny new features until this list feels done. Quality arc, not a feature sprint. ✨

### Just landed 🎉
- [x] **A measurement audit of the Lab Signal bench**: every reading taken apart and asked whether it was true. Occupied bandwidth was measuring the captured span instead of the signal, and that one wrong number was quietly feeding four others. RDS now survives a dropped block instead of throwing away seconds of decoded text, reads accented characters, and stops claiming a station name after you've retuned away from it
- [x] **FM demodulator with RDS**: filed under sharper radio math rather than new shiny, because it exists to make the numbers say more, not to add a screen. The `lab_signal` bench (`8`, focus `m`) demodulates the channel: deviation, stereo pilot and injection, CTCSS, AM depth, and **RDS** station name, PTY and RadioText. Still no audio, deliberately. It reads radios, it doesn't play them
- [x] **RTL-SDR support**: R820T / R828D / E4000, the most common dongle on Earth, behind a clean device-abstraction layer. Normal RX and observer mode both

### Hardware pipeline
- [ ] Airspy Mini / Airspy HF+ Discovery
- [ ] HackRF Pro
- [ ] LimeSDR / bladeRF / SDRplay / PlutoSDR via SoapySDR

### Later (once the polish arc is home)
- [x] Frequency scanner mode: the `lab_sweep` / `micro_sweep` scanner ✅ *done*
- [ ] Signal recording to file
- [ ] In-app config editing (no hand-editing TOML)

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

**[Credits](CREDITS.md)**
