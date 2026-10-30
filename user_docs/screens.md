# What You See on Screen

← [Back](README.md)

This page is the tour: every panel sdrtop can draw, and what it's telling you. It
stays at the "what am I looking at" level. What each *measurement* means, and how
to act on it, is in [The Lab presets](lab.md).

---

## How the screen is put together

sdrtop is built out of **panels**, and a **preset** is a named arrangement of
them, which this page also calls a **layout** or a **view**. Views are grouped,
and [the menu](#the-menu) is how you move between them. Within a group the
number keys pick a view and `p` steps to the next one. You can define your own
out of any panel on this page, in the [config file](presets.md). Nothing here is
hard-wired to a particular screen.

Panels that show a derived measurement mark their title **[STALE]** the moment RX
stops, so a number that has stopped updating never passes for a live one.

---

## The menu

The first thing sdrtop shows you, and `Esc` from anywhere brings it back.

Along the top, a row of **tiles**: **Resume**, **Radio**, **Lab**,
**Options**, **Keys** and **Quit**. `←` `→` choose one, or press its letter
(`R` `L` `O` `K` `Q`) from anywhere in the menu. Resume is only there when
there is something to resume: the view your last session ended on. On a
first run the menu opens on Lab.

Under Radio and Lab is a **tree**, opened one column at a time, left to
right: `↓` or `Enter` from a tile goes in, `↑` `↓` move in a column, `→` or
`Enter` opens the next column (or the view), `←` goes back, and `←` from the
first column returns to the tiles. `Tab` jumps between the tiles and the
tree. The row the cursor is on is marked with a bar at its left edge, as on
every list in sdrtop, and the rows it came through keep a mark too, so the
whole path is on screen. The line above the columns says the path in words.

| Tile | Under it |
|------|----------|
| **Radio** | Listening to the air. **Pagers › POCSAG** is [the pager](radio-pocsag.md); the rest (FM broadcast, aviation, marine, the ham bands, walkie-talkies) are listed as *later*, to say what the tile will hold |
| **Lab** | Measuring and decoding. **Spectrum** (the general views), **Sweep**, **Receiver** (the four benches), **Bluetooth › 2.4 GHz** (Overview, Low Energy, Classic, Long range), **Wi-Fi › 2.4 GHz** (Channel; Room, Network and Transmitter later, and 5 GHz) and **Compact** (large type, for a small window) |
| **Options** | The radio's own settings, where it has any: a tinySA's, the Airspy R2's bias tee and packing, the HF+ Discovery's Library DSP |
| **Keys** | Every key, the group you are in first: each panel's focus letter and what it adds, then the general keys |
| **Quit** | What quitting saves, and `Enter` quits. Plain `q` quits from anywhere, as on the deck |

To the right of the columns is the **card**: what the row under the cursor
is, in plain words. For a view, its name and the key that opens it, what
you will see on it, the line it would show you right now (`●` its receiver
running, `○` not), **where it listens** on a ruler from 1 MHz to 7 GHz with
your radio's own reach drawn solid, and **what opens**, a small map of its
panels drawn from the view's own layout rather than from a picture someone
kept up to date by hand.

**A view your radio cannot do is still listed.** It is greyed, and the card
says why in a sentence, for example that this radio tunes 24 to 1800 MHz and
Bluetooth lives at 2.4 GHz. `Enter` does nothing on it. A menu where things
quietly vanish is a menu you stop trusting; one that says "not on this
radio, and here is why" saves you the ten minutes of looking for it.

Views come in **groups** (Spectrum, Receiver, Low Energy, Classic, ...), and
each group numbers its own views from `1`. The number beside a view is the
key that opens it while the cursor is in that group, and the same number
works on the deck once you are there: the footer shows it, and `p` steps to
the next view in the group. This guide writes them as group then number, so
`Receiver 2` is the Receiver group's second view, Gain & noise.

Nothing behind the menu is disturbed while it is open. The radio keeps streaming,
and the header keeps the tuned frequency on screen, so opening the menu never
hides the one number you were watching.

---

## The main views

### Tuning desk · `Spectrum 1`

The default view, and the cockpit. A left **instrument rail** that
gathers everything a poweruser glances at, with the spectrum and waterfall bonded
to its right. From top to bottom:

- **Frequency hero**: the tuned frequency in big segmented digits, the actively
  tuned digit lit.
- **S-meter**: a classic analog signal-strength bar (S1 to S9+60) with a green to
  amber to red gradient and a faint peak-hold pip, sitting under the band and
  sample-rate line. It moves like a real meter's needle rather than like a
  strobe: up within 10 ms, so a short burst still shows, and down over 300 ms,
  so a gap does not drop it to the floor and back. The pip is the true peak,
  the highest reading of the last second. The figure beside it (marked `~`,
  for damped) is the bar's own level, refreshed four times a second so you can
  read it. With nothing to read, the meter keeps its place and says
  `no reading`. All of it is display only: every measurement elsewhere uses
  the raw reading.
- **HUNT · MONITOR · BENCH tabs**: the mode strip. The lead card below it follows
  what you're doing. Tuning surfaces the strongest carriers (Hunt), idling shows a
  calm watch headline (Monitor), changing gain shows front-end health (Bench). It
  auto-follows your actions; in rail focus, `Tab` pins one.
- **Recall slots**: saved frequencies (`M` to store, `1` `2` `3` to jump), each
  with a little activity pip when that frequency has a signal on screen right now.
- **SIGNAL**: SNR · PWR · NF · SAT, each as a braille **oscilloscope trace** of
  its recent history beside the live value and a trend arrow.
- **GAIN**: the front-end boost, then **one ⅛-block bar per gain stage the radio
  actually has**, named the way the driver names them, then the total and the
  clip headroom. Not a fixed LNA/VGA pair: a radio with three gain elements gets
  three rows, and a radio with one gets one. The Bench card's CHAIN verdict reads
  that headroom: "optimal" while the peak sits in the window the auto-gain also
  leaves alone, "hot" above it, "low" when there is a lot of gain going unused,
  and "clipping" once samples are actually hitting the rails.
- **STREAM**: drops, buffer fill, USB throughput, and a one-line log foot.

Press `c` to focus the rail: `←`/`→` tune, `1` `2` `3` recall, `M` save, `L` for
the full log overlay. A gain stage picked with `,` / `.` (a key every layout
has) lights up its name in the GAIN card, and the arrows move that
stage alone, by its own step, leaving the others where they are. Step past either
end of the list and you are back to driving the whole chain.

### Spectrum

A live graph of signal strength across the frequency range you're tuned to. The
horizontal axis is frequency, the vertical axis is signal strength in dBFS, where
0 is the maximum the ADC can represent. Stronger signals appear higher up.

- The bright line is the live signal.
- The dimmer line behind it shows the peak levels seen so far (peak hold).
- The dashed line near the bottom is the noise floor, which is what "silence"
  looks like for your radio in current conditions.

Band labels (FM, Aviation, Marine and so on) appear at the top when relevant
frequencies are in view, and any [markers](tips-and-tricks.md) you've placed show
as labelled vertical lines.

In focus mode (`e`) you get a cursor, tuning, zoom on both axes, markers, and
`d` to switch the trace between braille, filled and scatter styles.

### Waterfall

A scrolling history of the spectrum. Each new row is one moment in time, scrolling
downward, with color running from dark (weak) to bright (strong). This is where
time-domain behaviour shows up: a signal that appears and disappears, a carrier
that drifts, interference that comes and goes on a schedule.

In focus mode (`l`) you can scroll back through the history, stretch the time
window by averaging frames together, zoom the frequency span, and cycle the color
palette with `p`.

When the spectrum sits directly above the waterfall, the two **bond** into a
single instrument sharing one frequency ruler between them, rather than facing
each other across two borders. Zooming one zooms both, because they are drawing
the same axis.

### Signal strip

A single bar with eight live readings, sized to sit at the bottom of a layout:

- **SNR**: signal-to-noise ratio. Higher is cleaner.
- **PWR**: channel power in dBFS.
- **NF**: estimated noise floor in dBFS.
- **SAT**: ADC saturation percentage, the share of samples pinned to the
  converter's rails. The colour means the same thing on every screen: green under
  1 %, amber to 5 %, red above. Red means turn the gain down.
- **DROP**: sample drops per second. If this is non-zero, USB can't keep up.
- **BUF**: receive buffer fill percentage. A leading indicator: if this climbs
  toward 100 %, drops are coming.
- **IQ**: IQ amplitude imbalance in dB. Small values (under ±1 dB) are normal.
- **RBW**: resolution bandwidth, the frequency resolution of the current FFT.

---

## The lab benches

The four views in **Lab › Receiver**, plus the band sweep next door in
**Lab › Sweep**, are the measurement side of sdrtop. Each one puts three or four panels
together to answer a single question. This section says what each panel *is*;
[The Lab presets](lab.md) explains the readings.

### The measurement banner and the marker bar

Two thin bars wrap every lab preset, and they're easy to mistake for decoration.

The **banner** across the top names which bench you're on, then gives you five
fields: `REF` the reference level, `AVG` the averaging depth, `CAL` whether a
reference trace is captured, `MKR` how many markers are placed, and at the far
right whether the stream is live. A field that isn't set yet shows a dash. Focus
the banner with `b` and all of those become controls rather than a read-out.

The **marker bar** along the bottom reads out your placed markers and the `Δ`
between two of them, in both frequency and level. On the signal bench it also
carries the occupied bandwidth and a quality verdict; on the RF bench it reads the
ADC window instead (clip ceiling, peak, headroom, noise, SNR).

### Gain & noise · `Receiver 2`

Three panels that tell the whole receive chain as one story: level climbs stage by
stage, the gap between signal and noise is the SNR set at the antenna, and gain
only positions that gap inside the ADC window. It cannot widen it.

- **RF Diagnostics** *(left, focus `d`)*: the chain as numbers. Gain lineup,
  staging bars with optimal-target ticks, per-stage and Friis-total noise figure,
  sensitivity (MDS), and a plain-language verdict. `A` auto-stages the gain.
- **Gain-Staging Level Diagram** *(centre)*: the same lineup as a picture, the
  signal as a staircase up the chain, a step for each stage's gain, and the
  converter's window at the right: clip, signal, floor and the distances
  between them.
- **ADC Loading** *(right)*: the bit ladder, how much of the converter the
  samples use, one column a bit; the staging verdict, the loading read-out (peak,
  rms, crest, bits reached, clips), and a modeled linearity card.

The dBm figures are modeled and relative, not a calibrated wattmeter reading, and
the panel says so itself.

### IQ balance · `Receiver 1`

Three views of the same quadrature question, from three directions.

- **IQ Diagnostics** *(left, focus `i`)*: the DC offset against the signal, gain
  and phase imbalance as analog **null-meters** where the centre tick is perfect
  and the needle shows how far off you are, and the image rejection those two
  predict. Its focus mode is also
  where sdrtop *corrects* the stream rather than just measuring it.
- **IQ Constellation** *(centre)*: recent I/Q pairs as a dot-cloud coloured by
  density, phosphor-scope style, in a square zoomed to the cloud with dBFS rings,
  and a fitted imbalance ellipse over the top. A circle is healthy, an ellipse is
  amplitude imbalance, a tilt is phase imbalance, and an offset cloud is DC offset.
  Beside it, the envelope: how far out the points sit and how much that varies.
  The imbalance and DC numbers are one panel to the left. Yes, it looks like a
  1970s scope. That's the idea.
- **Image Scope** *(right)*: the empirical check on the computed IRR. It finds the
  strongest carrier, finds its mirror image reflected about the LO, and draws the
  gap between them on the mirror's column. A signal sitting on the LO has no image
  to measure, and it says so. The diagnostics panel calculates image rejection from the
  imbalance figures; this one goes and measures a real image.

### Signal quality · `Receiver 4`

- **Signal Characterization** *(left, focus `x`)*: what is that, and how clean is
  it? Modulation class, SNR, channel power, occupied bandwidth, adjacent-channel
  power, spectral shape, and a plain-language verdict.
- **Spectrum and waterfall** *(centre)*: bonded, as the reference picture.
- **FM MPX · Demod** *(right, focus `m`)*: the one panel that actually
  demodulates. Deviation, the MPX baseband with its 19 kHz pilot, stereo
  injection, CTCSS tones, AM depth, and RDS station name, PI, programme type and
  RadioText. There is no audio anywhere in sdrtop, deliberately. This is an
  instrument, not a receiver.

### USB & timing · `Receiver 3`

Whether your computer is keeping up with the radio in real time.

- **Timing Diagnostics** *(left, focus `t`)*: measured versus expected block
  period, jitter, the sample clock's offset in ppm, and a verdict that names its
  own reason. On a radio that pushes blocks at you (HackRF, RTL-SDR, the Airspys) it
  measures how late the worst blocks come against the radio's slack (a slice of
  one block, or a driver's queue where it keeps one); on one you pull from
  (SoapySDR) there is no deadline to miss, so it measures how much of the read
  loop went on waiting instead.
- **Interval Strip Chart** *(centre)*: every point is one real block, plotted by
  how far its arrival drifted from the expected interval. Late deliveries climb,
  early ones dip. A host hiccup is something you watch happen rather than infer.
- **Hardware Vitals** *(right, focus `v`)*: drops, ADC saturation, CPU and RAM as
  60-second trends, USB errors, configured versus measured sample rate, buffer
  fill, and uptime. Every one with a sparkline.

### Band sweep · `Sweep 1`

- **Sweep** *(body, focus `g`)*: your radio sees only as much spectrum at once as
  the sample rate covers. This scans a wider band by retuning across it, measuring
  briefly at each step and stitching the results into one curve with band-plan
  labels. `Enter` on the cursor tunes straight to that frequency.
- **Signal Metrics** *(right, focus `n`)*: a compact read-out of peak-to-noise,
  channel power, noise floor and occupied bandwidth for wherever you're parked.
- **Sweep strip** *(bottom)*: the sweep's own status bar. Band, progress, cycle
  info, and the cursor read-out with its band-plan name.

---

## Compact views

**Lab › Compact** in the menu. The idea is that sdrtop shouldn't need a full
terminal to be useful: squeezed into a slim tmux split, an SSH session on a phone,
or the small screen of a cyberdeck, the full panels stop being readable. Each
compact view strips one concern down to a single glance, and each adapts to the
width it's given, staying readable from an 80×24 SSH session down to a 40-column
framebuffer.

Four views, on `1` to `4` inside the group:

| Key | View | Built for |
|-----|------|-----------|
| `Compact 1` | **Essentials** | The four field questions at once: where am I, what's the signal, is it healthy, what's the gain |
| `Compact 2` | **Level at a glance** | Aiming an antenna. A large SNR bar with a trend arrow, plus channel power, noise floor, occupied BW and RBW |
| `Compact 3` | **Gain chain** | Setting gain fast on arrival. Wide LNA/VGA bars, prominent ADC utilisation, a gain-advisor verdict, with NF and MDS for context |
| `Compact 4` | **Link health** | Long unattended captures. Drop, saturation and buffer sparklines, CPU and RAM, USB throughput, sample-rate accuracy, a summary verdict and a session timer |

There used to be a fifth, the compact sweep, reached by cycling one step past
the last. It now lives under **Sweep** as `Sweep 2`, Small sweep, next to the
full-size sweep it is a small version of. Grouping by what a view is for beats grouping by
how small it is.

The looks here are still cooking. The idea is solid, the pixels are a work in
progress.

---

## Observer mode

If another app (GNU Radio, SDR++, `hackrf_transfer`) already holds your radio,
sdrtop can't control it, but it doesn't fall over either. It switches to observer
mode and shows what the operating system will tell it about the radio and the app
holding it. There's no spectrum, no waterfall and no tuning in this mode. What it
shows, and how to get the radio back, is in
[Advanced Features](advanced.md#observer-mode-when-another-app-owns-the-radio).

---

## Panels you can add yourself

Every panel sdrtop draws can be placed in a
[custom layout](presets.md). These are the ones the tour
above hasn't already covered: the structural pieces every layout is built from,
plus one measurement panel that has no home preset of its own.

| Panel | What it draws |
|-------|---------------|
| `header` | The full "Radio" block: frequency, band, sample rate, LNA/VGA/AMP and stream status |
| `header_slim` | The thin version used by the Tuning desk, where the frequency lives in the rail instead |
| `system_resources` | sdrtop's own CPU and memory use |
| `iq_histogram` | IQ amplitude distribution across 32 bins with a Low/Mid/Clip breakdown and PAPR |
| `signal_strip` | The eight-reading bar described above |
| `log` | The scrollable message log, with a severity lamp in the gutter |
| `footer` | Key hints, one set per group: the radio keys (with the gain stages your radio actually reports), the group's own keys (under Bluetooth, survey or lock and the address mode with their current state, export and the reference), the group's views with the current one marked, and the way out. Focus letters are not repeated here: every panel names its own in its title. While a panel is focused, its keys instead. `Tab` hides it |

---

## Layouts

The twenty-eight built-in layouts, by group. The key is the number to press while
you are in that group, which is what the menu shows beside each name.

The menu calls each by its title rather than its preset name, so `command_rail`
reads as **Tuning desk** and `main` as **Classic screen**. The preset name is the
one you use in a [config file](presets.md); the title is only what it is called on
screen. No two views share a title, and none is called what its group is.

**Lab › Spectrum**

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | Tuning desk | `command_rail` | The instrument rail + bonded spectrum/waterfall (the default) |
| `2` | Full spectrum | `spectrum` | Spectrum, with header and log |
| `3` | Full waterfall | `waterfall` | Waterfall, with header and log |
| `4` | Spectrum + waterfall | `spectrum_waterfall` | Both, bonded |
| `5` | Classic screen | `main` | Spectrum, waterfall, signal strip and log under a full header |

**Lab › Sweep**

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | Band sweep | `lab_sweep` | Sweep scanner · signal metrics · sweep strip |
| `2` | Small sweep | `micro_sweep` | The same scan as a compact field view |

**Lab › Receiver**

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | IQ balance | `lab_iq` | IQ diagnostics · constellation · image scope |
| `2` | Gain & noise | `lab_rf` | RF diagnostics · level diagram · ADC loading |
| `3` | USB & timing | `lab_timing` | Timing diagnostics · interval strip chart · hardware vitals |
| `4` | Signal quality | `lab_signal` | Signal characterization · spectrum/waterfall · FM demod |

**Lab › Bluetooth › 2.4 GHz › Overview**

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | Radio check | `net` | Band capability: what this radio can reach and receive |
| `2` | Bluetooth survey | `net_survey` | Occupancy over the coexistence heatmap, bonded · decode health |

**Lab › Bluetooth › 2.4 GHz › Low Energy**

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | LE devices | `net_census` | The census of transmitters, their clocks and the selected one's detail |
| `2` | Advertising | `net_ble` | BLE advertising packets · packet detail |
| `3` | One LE link | `net_connection` | One BLE connection, event by event |

**Lab › Bluetooth › 2.4 GHz › Classic**

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | Piconets nearby | `net_bt` | Classic Bluetooth hops (where and when) · the piconet roster |
| `2` | Packets | `net_piconet` | One piconet's packets |
| `3` | Both ends | `net_bench` | Each end of one piconet, side by side |

**Lab › Bluetooth › 2.4 GHz › Long range**

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | Long-range devices | `net_coded` | LE Coded advertising packets · packet detail |

**Lab › Wi-Fi › 2.4 GHz › Channel**

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | Channel survey | `wifi_survey` | Thirteen columns, thirteen columns, each channel's busy share beside its frames' airtime by format · least busy, or why none can be named yet |
| `2` | Frames | `wifi_frames` | A capture at a time: the river of its air · its frames in time order, with their format, signalling, gap, airtime, offset and SNR · frames a second by format, and the selected frame field by field |
| `3` | Airtime | `wifi_airtime` | A column a second, the busy share beside the frames' airtime by format · the ledger, a row a format and rate · the share of the time with its 1 sigma, and the rhythm of the gaps between frames |

**Lab › Compact**

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | Essentials | `micro_main` | Where am I, what's the signal, is it healthy, what's the gain |
| `2` | Level at a glance | `micro_signal` | A large SNR bar, for aiming an antenna |
| `3` | Gain chain | `micro_gain` | The gain stages and the ADC, for setting gain fast |
| `4` | Link health | `micro_health` | Drops, saturation, buffer and CPU, for long unattended runs |

**Radio › Pagers › POCSAG** ([the pager](radio-pocsag.md))

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | Pager | `radio_pocsag` | A POCSAG pager, as one looks: the page on its dot-matrix display, its address and what it took repairing, the buttons with their keys, the link along the bottom · every page received · the shown page as received, its physics and its decoding |

**Radio › Digital modes › FT8**

| Key | Title | Preset | What's in it |
|-----|-------|--------|--------------|
| `1` | World | `radio_ft8` | FT8 on a map of the world around you, every station where it said it was, the land lit by the sun · the minute's cycles and the clock as the stations heard judge it · the band as a river · the decodes cycle by cycle · the chosen station's card |

`p` steps to the next layout in whichever group you are in, and wraps at the
end. A thirtieth preset, `observer`, is marked hidden, so it has no group and no
key: sdrtop loads it by itself when [another app owns the
radio](#observer-mode).

`main` had no key at all until the groups arrived. The general views had four
digits between them and there were five of them, so one had to lose, and it lost
quietly: the docs said "`p` reaches it" and left it there. Groups give each
family its own nine digits, which is more than any of them needs.

A preset's *name* matters in one place: anything starting with `lab_` renders in
**instrument mode**. If you're writing your own layout, that and the other two
arrangements with behaviour attached are covered in
[Configuration](presets.md), along with how to give it a group and a number of
its own.

---

The **lab presets** have their own detailed walkthrough:
**[The Lab Presets](lab.md)**.
