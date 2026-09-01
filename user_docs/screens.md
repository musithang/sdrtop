# What You See on Screen

← [Back](README.md)

This page is the tour: every panel sdrtop can draw, and what it's telling you. It
stays at the "what am I looking at" level. What each *measurement* means, and how
to act on it, is in [The Lab presets](lab.md).

---

## How the screen is put together

sdrtop is built out of **panels**, and a **preset** is a named arrangement of
them, which this page also calls a **layout**. Layouts are grouped into four
**sections**, and the menu is how you move between them. Within a section the
number keys pick a layout and `p` steps to the next one. You can define your own
out of any panel on this page, in the [config file](presets.md). Nothing here is
hard-wired to a particular screen.

Panels that show a derived measurement mark their title **[STALE]** the moment RX
stops, so a number that has stopped updating never passes for a live one.

---

## The menu

The first thing sdrtop shows you, and `Esc` from anywhere brings it back.

It is one screen in two columns. The left column lists the four **sections**, and
below a dotted rule two panes that are not sections: **Keys** and **Options**. The
right column shows whatever is selected on the left.

| Section | Holds |
|---------|-------|
| **Command Rail** | The five general views: the cockpit, spectrum, waterfall, both bonded, and the classic layout |
| **Lab** | The four measurement benches |
| **Sweep** | The band sweep, full size and compact |
| **Micro** | The four field views for a small screen |

Each layout shows its **number** beside its name, and that number is the key that
opens it while you are in that section. The number is the same one the footer
shows you on the deck, because both read the same table. There is no second copy
to go stale.

**Keys** is the general key reference, scrollable with `↑` / `↓`. It replaces the
old `?` overlay, and unlike that overlay it is checked against the app's own key
dispatch, so it cannot promise a key that does not exist.

**Options** is empty on purpose so far, and says so when you open it.

The menu opens with the cursor on the layout you are already using, so `Enter`
puts you back without looking anything up. At startup that is your last session's
layout, which makes the first `Enter` of the day a resume.

Nothing behind the menu is disturbed while it is open. The radio keeps streaming,
and the header keeps the tuned frequency on screen, so opening the menu never
hides the one number you were watching.

---

## The main views

### Command Rail

The view sdrtop opens on (`Command Rail 1`). A left **instrument rail** that
gathers everything a poweruser glances at, with the spectrum and waterfall bonded
to its right. From top to bottom:

- **Frequency hero**: the tuned frequency in big segmented digits, the actively
  tuned digit lit.
- **S-meter**: a classic analog signal-strength bar (S1 to S9+60) with a green to
  amber to red gradient and a faint peak-hold pip, sitting under the band and
  sample-rate line.
- **HUNT · MONITOR · BENCH tabs**: the mode strip. The lead card below it follows
  what you're doing. Tuning surfaces the strongest carriers (Hunt), idling shows a
  calm watch headline (Monitor), changing gain shows front-end health (Bench). It
  auto-follows your actions; in rail focus, `Tab` pins one.
- **Recall slots**: saved frequencies (`M` to store, `1` `2` `3` to jump), each
  with a little activity pip when that frequency has a signal on screen right now.
- **SIGNAL**: SNR · PWR · NF · SAT, each as a braille **oscilloscope trace** of
  its recent history beside the live value and a trend arrow.
- **GAIN**: AMP, LNA and VGA as ⅛-block bars, plus total gain and clip headroom.
  The Bench card's CHAIN verdict reads that headroom: "optimal" while the peak
  sits in the window the auto-gain also leaves alone, "hot" above it, "low" when
  there is a lot of gain going unused, and "clipping" once samples are actually
  hitting the rails.
- **STREAM**: drops, buffer fill, USB throughput, and a one-line log foot.

Press `c` to focus the rail: `←`/`→` tune, `1` `2` `3` recall, `M` save, `L` for
the full log overlay.

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

The four layouts in the **Lab** section, plus the band sweep next door in
**Sweep**, are the measurement side of sdrtop. Each one puts three or four panels
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

### RF front-end bench · `Lab 2`

Three panels that tell the whole receive chain as one story: level climbs stage by
stage, the gap between signal and noise is the SNR set at the antenna, and gain
only positions that gap inside the ADC window. It cannot widen it.

- **RF Diagnostics** *(left, focus `d`)*: the chain as numbers. Gain lineup,
  staging bars with optimal-target ticks, per-stage and Friis-total noise figure,
  sensitivity (MDS), and a plain-language verdict. `A` auto-stages the gain.
- **Gain-Staging Level Diagram** *(centre)*: the same lineup as a picture, two
  traces climbing the stage axis. The vertical gap between them is the SNR being
  carried up the chain.
- **ADC Loading** *(right)*: a signed sample-histogram bell, the loading read-out
  (peak, rms, crest, effective bits, clip events), and a modeled linearity card.

The dBm figures are modeled and relative, not a calibrated wattmeter reading, and
the panel says so itself.

### IQ bench · `Lab 1`

Three views of the same quadrature question, from three directions.

- **IQ Diagnostics** *(left, focus `i`)*: DC offset, amplitude and phase
  imbalance, and IRR, each drawn as an analog **null-meter** where the centre tick
  is perfect and the needle shows how far off you are. Its focus mode is also
  where sdrtop *corrects* the stream rather than just measuring it.
- **IQ Constellation** *(centre)*: recent I/Q pairs as a dot-cloud coloured by
  density, phosphor-scope style, with a fitted imbalance ellipse over the top. A
  circle is healthy, an ellipse is amplitude imbalance, a tilt is phase imbalance,
  and an offset cloud is DC offset. No numbers here on purpose; they're one panel
  to the left. Yes, it looks like a 1970s scope. That's the idea.
- **Image Scope** *(right)*: the empirical check on the computed IRR. It finds the
  strongest carrier, finds its mirror image reflected about the LO, and shows the
  gap between them. The diagnostics panel calculates image rejection from the
  imbalance figures; this one goes and measures a real image.

### Signal bench · `Lab 4`

- **Signal Characterization** *(left, focus `x`)*: what is that, and how clean is
  it? Modulation class, SNR, channel power, occupied bandwidth, adjacent-channel
  power, spectral shape, and a plain-language verdict.
- **Spectrum and waterfall** *(centre)*: bonded, as the reference picture.
- **FM MPX · Demod** *(right, focus `m`)*: the one panel that actually
  demodulates. Deviation, the MPX baseband with its 19 kHz pilot, stereo
  injection, CTCSS tones, AM depth, and RDS station name, PI, programme type and
  RadioText. There is no audio anywhere in sdrtop, deliberately. This is an
  instrument, not a receiver.

### Timing bench · `Lab 3`

Whether your computer is keeping up with the radio in real time.

- **Timing Diagnostics** *(left, focus `t`)*: measured versus expected callback
  period, host clock drift in ppm, jitter, deviation percentiles against a
  deadline budget that scales with the sample rate, and a verdict.
- **Callback Interval Strip Chart** *(centre)*: every point is one real USB
  callback, plotted by how far its arrival drifted from the expected interval.
  Late deliveries climb, early ones dip. A host hiccup is something you watch
  happen rather than infer.
- **Hardware Vitals** *(right, focus `v`)*: drops, ADC saturation, CPU and RAM as
  60-second trends, USB errors, configured versus measured sample rate, buffer
  fill, and uptime. Every one with a sparkline.

### Sweep · `Sweep 1`

- **Sweep** *(body, focus `g`)*: your radio sees only as much spectrum at once as
  the sample rate covers. This scans a wider band by retuning across it, measuring
  briefly at each step and stitching the results into one curve with band-plan
  labels. `Enter` on the cursor tunes straight to that frequency.
- **Signal Metrics** *(right, focus `n`)*: a compact read-out of peak-to-noise,
  channel power, noise floor and occupied bandwidth for wherever you're parked.
- **Sweep strip** *(bottom)*: the sweep's own status bar. Band, progress, cycle
  info, and the cursor read-out with its band-plan name.

---

## Micro field views

The **Micro** section of the menu. The idea is that sdrtop shouldn't need a full
terminal to be useful: squeezed into a slim tmux split, an SSH session on a phone,
or the small screen of a cyberdeck, the full panels stop being readable. Each
micro view strips one concern down to a single glance, and each adapts to the
width it's given, staying readable from an 80×24 SSH session down to a 40-column
framebuffer.

Four views, on `1` to `4` inside the section:

| Key | View | Built for |
|-----|------|-----------|
| `Micro 1` | **Overview** | The four field questions at once: where am I, what's the signal, is it healthy, what's the gain |
| `Micro 2` | **Signal** | Aiming an antenna. A large SNR bar with a trend arrow, plus channel power, noise floor, occupied BW and RBW |
| `Micro 3` | **Gain** | Setting gain fast on arrival. Wide LNA/VGA bars, prominent ADC utilisation, a gain-advisor verdict, with NF and MDS for context |
| `Micro 4` | **Health** | Long unattended captures. Drop, saturation and buffer sparklines, CPU and RAM, USB throughput, sample-rate accuracy, a summary verdict and a session timer |

There used to be a fifth, the compact sweep, reached by cycling one step past
Health. It now lives in the **Sweep** section as `Sweep 2`, next to the full-size
sweep it is a small version of. Grouping by what a view is for beats grouping by
how small it is.

The looks here are still cooking. The idea is solid, the pixels are a work in
progress.

---

## Observer mode

If another app (GNU Radio, SDR++, `hackrf_transfer`) already holds your radio,
sdrtop can't control it, but it doesn't fall over either. It switches to observer
mode and shows what the operating system will tell it: device identity, which
process is holding the radio, USB statistics, and its own CPU and RAM.

There's no spectrum, no waterfall and no tuning in this mode, and the config is
not saved on quit, since there's nothing new to save. When the other app lets go,
sdrtop picks the radio back up automatically. See
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
| `header_slim` | The thin version used by the Command Rail, where the frequency lives in the rail instead |
| `system_resources` | sdrtop's own CPU and memory use |
| `iq_histogram` | IQ amplitude distribution across 32 bins with a Low/Mid/Clip breakdown and PAPR |
| `signal_strip` | The eight-reading bar described above |
| `log` | The scrollable message log, with a severity lamp in the gutter |
| `footer` | Key hints for the current mode. `Tab` hides it |

---

## Layouts

The fifteen built-in layouts, by section. The key is the number to press while
that section is active, which is what the menu shows beside each name.

The menu labels them with a short title rather than the preset name, so
`command_rail` reads as **Rail** and `main` as **Classic**. The preset name is the
one you use in a [config file](presets.md); the title is only what it is called on
screen.

**Command Rail**

| Key | Preset | What's in it |
|-----|--------|--------------|
| `1` | `command_rail` | Command Rail + bonded spectrum/waterfall (the default) |
| `2` | `spectrum` | Spectrum, with header and log |
| `3` | `waterfall` | Waterfall, with header and log |
| `4` | `spectrum_waterfall` | Both, bonded |
| `5` | `main` | Spectrum, waterfall, signal strip and log under a full header |

**Lab**

| Key | Preset | What's in it |
|-----|--------|--------------|
| `1` | `lab_iq` | IQ diagnostics · constellation · image scope |
| `2` | `lab_rf` | RF diagnostics · level diagram · ADC loading |
| `3` | `lab_timing` | Timing diagnostics · callback strip chart · hardware vitals |
| `4` | `lab_signal` | Signal characterization · spectrum/waterfall · FM demod |

**Sweep**

| Key | Preset | What's in it |
|-----|--------|--------------|
| `1` | `lab_sweep` | Sweep scanner · signal metrics · sweep strip |
| `2` | `micro_sweep` | The same scan as a compact field view |

**Micro**

| Key | Preset | What's in it |
|-----|--------|--------------|
| `1` | `micro_main` | Overview |
| `2` | `micro_signal` | Signal |
| `3` | `micro_gain` | Gain |
| `4` | `micro_health` | Health |

`p` steps to the next layout in whichever section you are in, and wraps at the
end. A sixteenth preset, `observer`, is marked hidden, so it has no section and no
key: sdrtop loads it by itself when [another app owns the
radio](#observer-mode).

`main` had no key at all until the sections arrived. The general views had four
digits between them and there were five of them, so one had to lose, and it lost
quietly: the docs said "`p` reaches it" and left it there. Sections give each
family its own nine digits, which is more than any of them needs.

A preset's *name* matters in one place: anything starting with `lab_` renders in
**instrument mode**. If you're writing your own layout, that and the other two
arrangements with behaviour attached are covered in
[Configuration](presets.md), along with how to give it a section and a number of
its own.

---

The **lab presets** have their own detailed walkthrough:
**[The Lab Presets](lab.md)**.
