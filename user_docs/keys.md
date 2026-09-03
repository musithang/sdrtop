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

## The menu

`Esc` opens the menu. It is also the first thing you see when sdrtop starts.

The menu is one screen in two columns. On the left are the **sections**, which
are the four families of layout, plus two panes underneath a dotted rule: the
**Keys** reference and **Options**. On the right is whatever the left column has
selected.

| Key | In the menu |
|-----|-------------|
| `Tab` / `Shift+Tab` | Next or previous row in the left column: the sections, then Keys, then Options |
| `←` / `→` | The same thing |
| `↑` / `↓` | Move through the layouts on the right, or scroll the Keys reference |
| `1` to `9` | Open the layout with that number, in this section |
| `Enter` | Open the highlighted layout |
| `Esc` | Close the menu and go back to what you were looking at |
| `q` | Quit and save |

The menu opens with the cursor already on the layout you are using, so `Enter`
puts you straight back. That is also how the first screen of a session works:
sdrtop remembers the layout you quit from, and the menu opens on it.

**Numbers belong to a section.** `2` is the RF bench inside Lab and the spectrum
inside Command Rail, and each section starts again at `1`. This is what keeps the
keyboard small: four families of layout, nine keys, instead of one long row of
numbers you have to memorise.

Because a bare number is ambiguous on its own, this guide writes them as
**section then number**:

| Written | Means |
|---------|-------|
| `Lab 2` | The RF bench, `2` while the Lab section is active |
| `Sweep 1` | The band sweep |
| `Micro 3` | The gain field view |

On screen you never have to do that translation: the menu shows the number beside
each layout, and the footer shows the numbers for the section you are in.

---

## General

| Key | What it does |
|-----|-------------|
| `Space` | Start or stop receiving |
| `f` | Type a new center frequency (in MHz) |
| `s` | Type a new sample rate (HackRF 2–20 MHz · RTL-SDR 0.9–3.2 MHz) |
| `r` | Reset all settings to defaults |
| `a` | Toggle the front end boost: RF amplifier (HackRF) / tuner AGC (RTL-SDR). Absent on a device that reports neither |
| `w` | Pause or resume the waterfall |
| `h` | Freeze the spectrum (hold the current frame behind the live one) |
| a letter | Focus the panel whose title highlights it. `e` `l` `c` `i` `d` `t` `v` `x` `m` `n` `g` `b`, all listed [below](#focus-modes) |
| `1` to `9` | The nth layout **of the section you are in** |
| `p` | Next layout in the same section, wrapping at the end |
| `Esc` | Leave panel focus, or open the menu when nothing is focused |
| `Tab` | Show or hide the footer bar |
| `q` | Quit and save settings |

`q` **saves**. Quitting is how your frequency, gains, markers and sweep band
persist to the [config file](config.md); `Ctrl+C` exits without saving anything.

`p` stays inside the section. It used to walk every preset in the app in
alphabetical order, which meant leaving the benches for a micro view halfway
through and no way to predict what came next. Cycling within Lab is a thing you
might actually want to do.

`Esc` steps out one level at a time: out of a focused panel first, and only then
into the menu. So a focused panel is never one keystroke away from a full screen
you did not ask for.

If something goes wrong and the screen tells you nothing, look in
`~/.config/sdrtop/sdrtop.log`. sdrtop redirects its own error output there for the
whole session, because the alternate screen has no room for a stack trace. See
[Troubleshooting](troubleshooting.md).

---

## Gain

| Key | What it does |
|-----|-------------|
| `↑` / `↓` | Gain up or down: HackRF LNA (±8 dB) / RTL-SDR tuner (next table step) / SoapySDR the whole chain |
| `[` / `]` | VGA gain up or down by 2 dB (HackRF only) |

On a **HackRF**, LNA (Low Noise Amplifier) is the first gain stage, how much you
amplify before the signal reaches the chip, and VGA (Variable Gain Amplifier) is
the second stage, fine-tuning the level further in. A good starting point: LNA 24,
VGA 30.

On an **RTL-SDR** there's a single tuner gain that steps through a fixed table of
values (the `↑`/`↓` keys walk it), and no VGA, so `[`/`]` simply do nothing.
Instead of a VGA you have tuner **AGC**, toggled with `a`.

On a device reached through [SoapySDR](hardware.md#soapysdr-the-honest-version),
`↑`/`↓` move the **whole chain**, and sdrtop decides where the gain goes: it
fills the front stage first, up to its ceiling, then the next one. That is the
arrangement with the best noise figure, and it is not the one most drivers pick
for you. Whether `a` exists at all still depends on the driver: if it reports no
automatic gain mode, the key is not offered and the panels leave the row out
rather than showing you an `OFF` you cannot change.

**To set one stage on its own, on any device**, focus the Command Rail with `c`,
pick the stage with `,` and `.`, then use `↑`/`↓`. That works on a radio with
three gain elements as well as on a HackRF, and it is the only way to reach the
third one. See [Command Rail focus mode](#command-rail-focus-mode).

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

Press `c` to drive the Command Rail, the instrument rail in the default
`Command Rail 1` layout.

| Key | What it does |
|-----|-------------|
| `←` / `→` | Tune the center frequency by one step (auto-switches the mode strip to Hunt) |
| `,` / `.` | Pick which gain stage the arrows drive, or step past the ends for the whole chain |
| `↑` / `↓` | Move the picked stage by its own step. With no stage picked, the usual gain keys |
| `1` / `2` / `3` | Jump to recall slot 1, 2 or 3 |
| `M` | Save the current tuning to the next recall slot |
| `Tab` | Cycle the HUNT · MONITOR · BENCH mode manually (otherwise it auto-follows your actions) |
| `L` | Toggle the full-log overlay |
| `Esc` | Close the log overlay if open, otherwise exit focus mode |

**While the rail is focused, `1` `2` `3` are recall slots and not layouts.** A
focused panel is offered every key first, and the rail claims those three. The
rest of the digits fall through as usual, so `4` and `5` still switch layout from
inside rail focus. `Esc` gives the digits back.

### Setting one stage at a time

`,` and `.` walk the gain rows in the rail, in the order your device reports
them, and the selected row's name lights up. `↑` and `↓` then move **that stage
alone**, by that stage's own step: 8 dB on a HackRF LNA, 2 dB on its VGA, one
table entry on an RTL-SDR tuner, and whatever the driver says everywhere else.
Nothing is redistributed. That is the entire point of the mode.

"The whole chain" is a real stop on the ring rather than a thing you have to
remember `Esc` for. Keep pressing `.` past the last stage and the selection comes
back off, and `↑`/`↓` go back to being the ordinary gain keys.

Leaving focus with `Esc` also puts the selection back to the whole chain. It does
**not** put the gains back: undoing a setting you deliberately made, on the way
out, would be the surprising half of that.

---

## Lab panel focus modes

| Key | Panel | Where it lives | What it adds |
|-----|-------|----------------|--------------|
| `b` | Measurement banner | every lab preset | `↑↓` reference level · `[ ]` trace averaging · `C` capture or clear the reference trace · `R` clear the reference level |
| `i` | **I**Q Diagnostics | `Lab 1` | `D` DC-block · `C` auto-cal · `F` freeze the constellation · `M` pin the carrier/image markers |
| `d` | RF **D**iagnostics | `Lab 2` | `A` auto-gain · `K` measure the noise step · `⎵` or `F` freeze the histogram and level diagram |
| `t` | **T**iming Diagnostics | `Lab 3` | `R` reset the session jitter peak · `C` clear the jitter and throughput history |
| `v` | Hardware **V**itals | `Lab 3` | `R` reset the session drop counter · `C` clear the trend sparklines |
| `x` | Signal Characterization | `Lab 4` | `C` log a snapshot of the modulation, SNR, occupied bandwidth and ACPR |
| `m` | FM **M**PX · Demod | `Lab 4` | `Space` demod on/off · `←/→` move the channel ±25 kHz · `P` snap to the strongest carrier · `0` re-centre · `T` force WFM / NFM / AM or auto · `C` log a snapshot |
| `n` | Signal Metrics | `Sweep 1` | `C` log a snapshot |
| `g` | Sweep | `Sweep 1` | `←/→` cursor · `S` / `E` set start / end frequency · `M` peak or mean curve · `+/-` dwell time · `C` log a snapshot · `Enter` tune to the cursor frequency |

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

### The noise step (`K` in RF Diagnostics)

`K` runs a short measurement instead of reporting one. sdrtop walks the front
gain stage across its settings, waits at each for the display to settle, and
records what the noise floor did. Six settings takes about five seconds on a
HackRF.

What comes back is the **knee**: the lowest gain from which the noise floor
follows the gain, which is the lowest gain at which your radio's front end, and
not its converter, decides how faint a signal you can hear. Below the knee you
are throwing sensitivity away. Above it you are only spending headroom.

It needs RX running, and it takes the chain over while it runs: the auto-track
latch drops, because auto-gain exists to undo exactly what the sweep is doing.
Press `K` again to stop early. However you leave it, by finishing, by stopping,
by stopping RX or by quitting the app, the stage goes back where it was. Stopping
early reports nothing at all: an interrupted measurement has no answer.

See [the RF bench](lab.md#the-noise-step-k) for what the reading says and,
just as importantly, what it does not.

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

- If you forget a key, `Esc` and then `Tab` to **Keys** shows the whole general
  list without leaving the app. It is generated from the same table the app
  dispatches on, so it cannot drift out of date the way the old `?` overlay did.
- Gain settings, frequency, markers, sweep band, trace style and waterfall palette
  are all saved when you quit with `q`. You can also edit them directly in the
  [config file](config.md).
- No key does anything irreversible. The worst outcome of pressing an unfamiliar
  one is that you learn something.
