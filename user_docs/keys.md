# Keyboard Shortcuts

← [Back](README.md)

This page is the complete key reference. Every key sdrtop responds to is listed
here, and nowhere else, so there is only ever one version to be wrong.

---

**Capitals work everywhere.** Every key below is listed the way the footer shows
it, and `C` and `c` do the same thing. You never have to think about whether Shift
is down. The one place case still matters is when you are *typing* something, like
a marker name, where what you press is what you get.

---

## General

| Key | What it does |
|-----|-------------|
| `Space` | Start or stop receiving |
| `f` | Type a new center frequency (in MHz) |
| `s` | Type a new sample rate (HackRF 2–20 MHz · RTL-SDR 0.9–3.2 MHz) |
| `r` | Reset all settings to defaults |
| `a` | Toggle the RF amplifier (HackRF) / tuner AGC (RTL-SDR) |
| `w` | Pause or resume the waterfall |
| `h` | Freeze the spectrum (hold the current frame behind the live one) |
| a letter | Focus the panel whose title highlights it. `e` `l` `c` `i` `d` `t` `v` `x` `m` `n` `g` `b`, all listed [below](#focus-modes) |
| `1` / `2` / `3` / `4` | Layout presets: Command Rail (the default) · spectrum · waterfall · both |
| `5` / `6` / `7` / `8` / `9` | Lab presets: IQ · RF · timing · signal · sweep |
| `0` | Micro field views. Press again to cycle: overview → signal → gain → health → sweep |
| `p` | Cycle through every preset, built-in and your own, in alphabetical order |
| `Tab` | Show or hide the footer bar |
| `?` | Show the help overlay |
| `q` | Quit and save settings |

`q` **saves**. Quitting is how your frequency, gains, markers and sweep band
persist to the [config file](config.md); `Ctrl+C` exits without saving anything.

One built-in preset, `main`, has no number key of its own. `p` is the way to
reach it.

If something goes wrong and the screen tells you nothing, look in
`~/.config/sdrtop/sdrtop.log`. sdrtop redirects its own error output there for the
whole session, because the alternate screen has no room for a stack trace. See
[Troubleshooting](troubleshooting.md).

---

## Gain

| Key | What it does |
|-----|-------------|
| `↑` / `↓` | Primary gain up or down: HackRF LNA (±8 dB) / RTL-SDR tuner (next table step) |
| `[` / `]` | VGA gain up or down by 2 dB (HackRF only) |

On a **HackRF**, LNA (Low Noise Amplifier) is the first gain stage, how much you
amplify before the signal reaches the chip, and VGA (Variable Gain Amplifier) is
the second stage, fine-tuning the level further in. A good starting point: LNA 24,
VGA 30.

On an **RTL-SDR** there's a single tuner gain that steps through a fixed table of
values (the `↑`/`↓` keys walk it), and no VGA, so `[`/`]` simply do nothing.
Instead of a VGA you have tuner **AGC**, toggled with `a`.

Either way: if the spectrum is maxed out (everything near 0 dBFS), turn it down.
If it's all noise at the bottom, try turning it up.

---

## Focus modes

A **focus mode** hands one panel the keyboard. Most panels have measurements to
show and nothing to press, but the ones that do have their own controls announce
it with a **highlighted letter in the panel title**: the **I** in "**I**Q
Diagnostics", the **D** in "RF **D**iagnostics". Press that letter to enter.

While a panel is focused its border lights up, the footer lists exactly the keys
that panel adds, and `Esc` leaves. Anything the focused panel does not claim falls
straight through to the general keys above, so you can still change gain, retune,
or switch presets without leaving focus first.

The focus key only works when the panel is actually on screen. If you press one
and nothing happens, the panel it belongs to isn't in the current preset.

---

## Spectrum focus mode

Press `e`.

| Key | What it does |
|-----|-------------|
| `←` / `→` | Tune the center frequency by one step |
| `[` / `]` | Change the tuning step size (1 kHz, 5, 10, 25, 100, 500 kHz, 1, 5, 10 MHz) |
| `↑` / `↓` | Zoom the dBFS axis (expand or compress the signal range shown) |
| `+` / `-` | Frequency zoom, magnifying the centre of the band (`=` also zooms in) |
| `j` / `k` | Move the cursor left or right across the spectrum |
| `m` | Place a named marker at the cursor position |
| `b` | Cycle channel bandwidth on the nearest marker |
| `d` | Cycle the trace style: braille → fill → scatter |
| `h` | Hold / unhold the spectrum frame (freeze a ghost behind the live signal) |
| `Esc` | Exit focus mode |

**Channel bandwidths** on `b`: 6.25, 12.5, 25, 50, 100, 200, 500 kHz, then none.
The cursor has to be near a marker (within four tuning steps) for it to land.

**Frequency zoom is shared.** In the presets where the spectrum and waterfall are
bonded into one instrument, `+` / `-` narrow both together, because they are
drawn against a single frequency ruler. Zooming only one half would be lying about
the other.

The trace style on `d` is saved to your config, so the look you pick is the look
you get next launch.

---

## Waterfall focus mode

Press `l`.

| Key | What it does |
|-----|-------------|
| `↑` / `↓` | Adjust the color scale (show faint or strong signals in more detail) |
| `[` / `]` | Frame averaging: combine multiple frames per row for a longer time window |
| `+` / `-` | Frequency zoom, magnifying the centre of the band (`=` also zooms in) |
| `p` | Cycle the color palette: classic → amber → ice → phosphor |
| `m` | Place or remove a frequency cursor |
| `←` / `→` | Move the cursor frequency when one is placed |
| `j` / `k` | Scroll back and forth through waterfall history |
| `Esc` | Exit focus mode |

`classic` follows whatever [theme](themes.md) you're running; the other three are
fixed gradients that ignore it. Like the spectrum's trace style, your choice is
saved to the config.

---

## Command Rail focus mode

Press `c` to drive the Command Rail, the instrument rail in the default `1`
preset.

| Key | What it does |
|-----|-------------|
| `←` / `→` | Tune the center frequency by one step (auto-switches the mode strip to Hunt) |
| `1` / `2` / `3` | Jump to recall slot 1, 2 or 3 |
| `M` | Save the current tuning to the next recall slot |
| `Tab` | Cycle the HUNT · MONITOR · BENCH mode manually (otherwise it auto-follows your actions) |
| `L` | Toggle the full-log overlay |
| `Esc` | Close the log overlay if open, otherwise exit focus mode |

---

## Lab panel focus modes

| Key | Panel | Where it lives | What it adds |
|-----|-------|----------------|--------------|
| `b` | Measurement banner | every lab preset | `↑↓` reference level · `[ ]` trace averaging · `C` capture or clear the reference trace · `R` clear the reference level |
| `i` | **I**Q Diagnostics | `5` lab_iq | `D` DC-block · `C` auto-cal · `F` freeze the constellation · `M` pin the carrier/image markers |
| `d` | RF **D**iagnostics | `6` lab_rf | `A` auto-gain · `⎵` or `F` freeze the histogram and level diagram |
| `t` | **T**iming Diagnostics | `7` lab_timing | `R` reset the session jitter peak · `C` clear the jitter and throughput history |
| `v` | Hardware **V**itals | `7` lab_timing | `R` reset the session drop counter · `C` clear the trend sparklines |
| `x` | Signal Characterization | `8` lab_signal | `C` log a snapshot of the modulation, SNR, occupied bandwidth and ACPR |
| `m` | FM **M**PX · Demod | `8` lab_signal | `Space` demod on/off · `←/→` move the channel ±25 kHz · `P` snap to the strongest carrier · `0` re-centre · `T` force WFM / NFM / AM or auto · `C` log a snapshot |
| `n` | Signal Metrics | `9` lab_sweep | `C` log a snapshot |
| `g` | Sweep | `9` lab_sweep | `←/→` cursor · `S` / `E` set start / end frequency · `M` peak or mean curve · `+/-` dwell time · `C` log a snapshot · `Enter` tune to the cursor frequency |

The two panels worth a longer word are the ones that change something rather than
just reporting it.

### The measurement banner (`b`)

The strip across the top of every lab preset is not decoration, it's a control
panel, and it brings the three habits of a bench spectrum analyser:

| Key | Control | Range |
|-----|---------|-------|
| `↑` / `↓` | **Reference level**: a horizontal line across the spectrum at the level you pick | 0 to −120 dBFS, 1 dB per press, starts at −10 |
| `R` | Clear the reference level | |
| `[` / `]` | **Trace averaging**: smooths the spectrum across successive FFT frames | 1 (no smoothing) to 16, default 5 |
| `C` | **Reference trace**: capture the current spectrum as a ghost behind the live one, or clear it | |

What each one is *for*, and why averaging finds signals a single frame buries, is
in [The Lab presets](lab.md#the-measurement-banner).

One wrinkle if you build [your own layout](presets.md): the
reference line and the ghost trace are drawn only in **instrument mode**, which
sdrtop turns on for presets whose name begins with `lab_`. The banner and its keys
work anywhere you place it, and averaging affects the spectrum everywhere, but if
you want the two overlays to appear, give your preset a name beginning with
`lab_`.

### IQ Diagnostics (`i`): it corrects, not just measures

`D` and `C` don't change the display, they change the samples.

- **`D`, DC-block** subtracts the live DC estimate from the stream. The permanent
  spike at your centre frequency is the front end's own DC offset, not a signal;
  this takes it out. Turn it on and watch the spike drop.
- **`C`, auto-cal** measures the amplitude and phase imbalance between I and Q in
  the current sample window, then applies the inverse correction to the stream
  from there on. Mirror images fade, IRR improves. It's a one-shot snapshot that
  stays fixed until you press `C` again, which clears it and shows you the
  uncorrected radio. Since it estimates from whatever is in the window at that
  moment, capture it with a decent signal present rather than on an empty band.
- **`F`** freezes the constellation cloud so you can study a shape while RX keeps
  running.
- **`M`** pins the carrier and image markers instead of letting them auto-track,
  which is useful when you want them to stay on the signal you chose rather than
  on whatever is loudest right now.

What each reading means is in [The Lab presets](lab.md).

### Auto-gain (`A` in RF Diagnostics)

One press jumps LNA and VGA to the staging target. Once you're already at the
optimum, pressing `A` again latches a **continuous auto-track** that re-nudges the
gain as the level drifts. It drives the same global gain controls you use by hand
(`↑↓`, `[ ]`, `a`, `r`), so touching any of them drops the latch immediately. It
never fights a manual tweak.

---

## Typing: frequency, sample rate, marker names

Three keys put sdrtop into a text-entry mode: `f` (frequency in MHz), `s` (sample
rate in MHz) and `m` in spectrum focus (marker name). While you're typing, keys
are letters rather than commands, and **this is the one place capitals mean what
they say**. `Enter` confirms, `Esc` cancels.

A marker name left empty gets an automatic label like `[1]`. Input formats and
examples are in [Advanced Features](advanced.md).

---

## Tips

- If you're not sure what a reading means, the `?` overlay shows a quick summary
  while you use the app.
- Gain settings, frequency, markers, sweep band, trace style and waterfall palette
  are all saved when you quit with `q`. You can also edit them directly in the
  [config file](config.md).
- No key does anything irreversible. The worst outcome of pressing an unfamiliar
  one is that you learn something.
