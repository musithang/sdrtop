# What's New

← [Back](README.md)

The story of sdrtop so far — not as a wall of dates, but as **checkpoints**: the big moments where the app levelled up. Each one is condensed to the essentials.

> **Where we are now:** the interactive TUI is feature-complete, RTL-SDR support has landed, and the current arc is **instrument-grade polish**: the **Command Rail** cockpit, the redrawn **Lab IQ**, the rebuilt **Lab RF** bench, the **Lab Timing** real-time bench, and now the **FM MPX · Demod** instrument with RDS are the latest (see Checkpoint 12). The ongoing work is polishing the UI, sharpening the radio math, and squashing bugs. So if something looks off or behaves oddly, that's exactly what we're hunting.

---

## ✅ Checkpoint 1 — It receives
The foundation: talk to the HackRF safely, pull IQ off the wire, and show it.
- Solid USB FFI layer with a clean shutdown on every exit path
- Live **spectrum analyzer** — FFT with peak hold, noise floor, dBFS and frequency axes
- Scrolling **waterfall** — truecolor / 256-color / 16-color, with a graceful fallback on basic terminals

## ✅ Checkpoint 2 — It remembers
sdrtop stopped being forgetful.
- Settings (frequency, gains, sample rate, layout) **persist** across restarts in `~/.config/sdrtop/config.toml`
- Atomic, safe saves; a missing or broken config just falls back to sane defaults
- **Six themes** (`sdr`, `nord`, `dracula`, `gruvbox`, `catppuccin`, `solarized`) and switchable **layout presets**

## ✅ Checkpoint 3 — It diagnoses
The part that makes sdrtop more than a pretty spectrum.
- **Hardware health** — drops, ADC saturation, USB errors, buffer fill, sample-rate accuracy
- **RF chain** — gain stages, frequency + wavelength, estimated **noise figure** and **minimum detectable signal**
- **IQ diagnostics** — DC offset, imbalance, **image rejection ratio**, plus an ADC amplitude **histogram**

## ✅ Checkpoint 4 — It plays nice
Less crashing, more cooperating.
- **Observer mode** — if another app already holds the radio, sdrtop watches what it can instead of falling over, then reclaims it when free
- Live **sample-rate control** (`s`) without restarting
- A big **performance overhaul** — far lower CPU/RAM at 30 fps, smooth even at high sample rates

## ✅ Checkpoint 5 — It analyzes
The spectrum and waterfall grew real tools, driven by a single highlighted **focus** key per panel.
- **Spectrum focus** (`e`) — tune with `←`/`→`, **zoom**, **hold** a ghost frame to compare, a **cursor** read-out, **band-plan** labels, and named **markers** that persist
- **Waterfall focus** (`l`) — adjustable color scale, scroll-back through history, and **frame averaging** to stretch the visible time window

## ✅ Checkpoint 6 — The lab bench
Bench-engineer views for people who care about the numbers, not just the picture.
- **Lab presets** `5`–`8`: IQ · RF · timing · signal
- Derived measurements worth trusting: **NF**, **MDS**, **IRR**, **PAPR**, sample-rate accuracy, and USB **timing/jitter** with a quality verdict
- **Hardware Vitals** now tracks sdrtop's own CPU/RAM with trend graphs
- Every lab panel marks itself **[STALE]** the instant RX stops — a frozen number is never mistaken for a live one

## ✅ Checkpoint 7 — It scans
- **Frequency sweep** (`9`) — scan a band wider than one window can show; sdrtop stitches it into one curve with band-plan labels. Focus with `g`, set the band live with `s` / `e`, and press `Enter` on a peak to tune straight to it
- **Micro field views** (`0`) — deliberately tiny single-glance read-outs (signal · gain · health · sweep) for slim splits, SSH sessions, and cyberdeck screens

## 🔧 Checkpoint 8 — Polish
The feature list is closed for now. This checkpoint is about taste: refining layout and readability, **reworking the micro view's UI**, double-checking every radio calculation, and fixing the rough edges — the groundwork that made the next leap safe to land.

## 📡 Checkpoint 9 — A second radio
sdrtop stopped being a one-device app.
- **RTL-SDR support** (R820T / R828D / E4000) lands alongside the HackRF One, behind a clean `SdrDevice` abstraction layer — the HackRF path is untouched, the RTL path shares the same RX → FFT → UI pipeline
- The UI **adapts to the hardware**: HackRF's LNA/VGA/AMP vs RTL-SDR's single tuner gain + AGC, the right frequency and sample-rate ranges, and N/A where a measurement doesn't apply (no BB filter, no Friis NF)
- Plug in more than one radio and a **device picker** greets you at launch; `--device hackrf|rtlsdr` pins one
- **Status: working, new.** Community-contributed and confirmed on real hardware — normal RX *and* observer mode, with FM reception, tuner gain, AGC and sweep all checked out. The only open question is the zoo of RTL clones, which no single person owns. **So this is where you come in:** run it on yours and [open an issue](../../../issues) with how it went — real-world reports are what make "works" universal.

## 🎛️ Checkpoint 10 — The instrument cockpit
The polish arc grew teeth: the UI started reading like a real radio's front panel, not a table of numbers.
- **Command Rail** (`1`, now the default): a left instrument rail with a big segmented **frequency hero**, an analog **S-meter**, the HUNT·MONITOR·BENCH mode tabs whose lead card follows what you're doing, recall slots with live activity pips, and a **SIGNAL** zone where SNR·PWR·NF·SAT each ride their own braille oscilloscope trace beside the value
- **Lab IQ, reimagined**: IQ diagnostics redrawn as analog **null-meters** (centre is ideal, the needle shows the deviation), paired with a **persistence constellation**, a density-coloured I/Q cloud with a fitted imbalance ellipse whose stretch is amplitude imbalance and whose tilt is phase imbalance
- **Lab RF, rebuilt as a front-end bench** (`6`): three panels that teach one idea — level climbs stage by stage, the signal/noise gap *is* the SNR set at the antenna, and gain only parks that gap in the ADC window. **RF Diagnostics** (gain lineup, staging, Friis noise figure, sensitivity, verdict), a **Gain-Staging Level Diagram** (signal and noise traces climbing ANT▸LNA▸MIX▸VGA▸ADC), and an **ADC Loading** panel (signed-sample histogram bell, loading stats, a modeled linearity card). Focus it with `D` and press `A` to auto-stage the gain, or `⎵` to freeze the bench — the dBm are honestly labelled *modeled / relative*, never a wattmeter
- A shared braille-instrument language (oscilloscope traces, ⅛-block gain bars, gradient fills) applied across the rail, with the radio math left exactly as honest as it always was. No "AI-enhanced" anything; the only thing that learns here is you

## 📻 Checkpoint 12 - It listens (you are here)
Every instrument so far described a signal from the outside: how strong, how wide, how clean. The new **FM MPX · Demod** panel in **Lab Signal** (`8`, focus `m`) opens the channel up and reads what is inside it. There is still no audio anywhere in sdrtop. This is a measurement instrument, and it reports things a spectrum plot cannot.
- **Deviation**: peak and RMS deviation measured *about the carrier*, so a mistuned radio reports its tuning error as offset instead of inflating the modulation figure, with the bar drawn against ±75 kHz for broadcast or ±5 kHz for narrowband
- **MPX baseband**: the demodulated composite from 0 to 60 kHz as a live profile, with the 19 k pilot, 38 k stereo difference and 57 k RDS subcarrier all visible where they live
- **Pilot / stereo**: STEREO, MARGINAL or MONO, plus the pilot's **injection percentage** against the 8 to 10 % broadcast norm, which reads a transmitter's health rather than your reception
- **RDS**: station name, PI code, programme type, traffic flags and **RadioText**, decoded off the 57 kHz subcarrier. Nothing appears until it has been confirmed twice, so a mis-decoded character never flickers on screen and gets corrected in front of you
- **CTCSS** for narrowband FM: the subaudible tone that opens a repeater's squelch, picked out of the standard 40-tone table with the margin it won by, and an honest "searching" state while it fills its half-second window
- **AM depth**: modulation depth with positive and negative peaks reported separately, because a negative peak approaching 100 % pinches the carrier off and splatters
- The demodulated channel tunes **inside** the captured spectrum with `←` / `→`, `P` snaps it to the strongest carrier while ignoring the radio's own LO leakage, and `T` forces the demodulator when the automatic classifier is too coarse to pick one

---

## ⏱️ Checkpoint 11 - Lab Timing
The newest instrument answers a question the others could not: is your computer keeping up with the radio in real time? The radio ships samples in steady USB bursts, one callback at a time, and your machine has to catch every one on schedule or the buffer backs up and samples drop. **Lab Timing** (`7`) watches that handoff and grades it.
- **Timing Diagnostics** (`t`): measured callback period versus the expected period at your sample rate, host clock drift in ppm, jitter, and per-callback deviation percentiles (p95 / p99 / peak) drawn against a **deadline budget** that scales with the rate, plus a late-callback count and a plain verdict (Excellent / Good / Marginal / Poor)
- **Callback Interval Strip Chart**: every point is one real callback, plotted as how far its arrival drifted from the expected interval. Late deliveries climb, early ones dip, and anything past the deadline band gets tagged (▲ late, ▼ early), so a host hiccup is something you watch happen instead of guess at
- **Hardware Vitals** (`v`): the supporting cast. Sample drops, ADC saturation and CPU / RAM as 60 second trends, USB link utilization against the device's real ceiling, ring-buffer overrun headroom, and uptime
- Quieter fixes that rode along: **Lab IQ** now gates carrier / image detection on the noise floor (an idle radio reads "no signal" instead of flagging a noise bin as a carrier), gains clamp to each device's own model on load, the RTL-SDR AGC indicator stays honest after a manual gain nudge, and the spectrum / waterfall cursor respects each radio's true frequency range

---

```
  ┌─[ sdrtop · 2026 ]──────────────────────────────────
  │  $ ./sdrtop --scan-for-hype
  │  > 0 LLMs detected in the signal path
  │  > no neural nets, no "AI-powered" sticker
  │  > just honest FFTs and a person who likes radios
  │  > carry on.
  └────────────────────────────────────────────────────
```

In a year where everything claims to be AI-powered, sdrtop is proudly powered by
math you can check yourself. The dBFS numbers are real, the bugs are mine. 📻

