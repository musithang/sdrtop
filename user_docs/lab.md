# The Lab Presets

← [Back](README.md)

sdrtop's **lab presets** are the bench-engineer views. Instead of just a live
spectrum, they surface the measurements sdrtop can derive about your receiver's
*signal quality* and *hardware health*. They're built for setting up a clean
capture and for watching for trouble during a long run.

Four benches under **Lab › Receiver**, plus the band sweep under **Lab ›
Sweep**. The key is the number to press while you are in that group:

| Key | View | The question it answers |
|-----|--------|--------------------------|
| `Receiver 1` | **IQ balance** | Is the quadrature clean, and can I make it cleaner? |
| `Receiver 2` | **Gain & noise** | Is the front end staged properly for what I'm chasing? |
| `Receiver 3` | **USB & timing** | Is my computer keeping up with the radio? |
| `Receiver 4` | **Signal quality** | What is that signal, and what's inside it? |
| `Sweep 1` | **Band sweep** | What's out there across a band too wide to see at once? |

[What you see on screen](screens.md) says which panel is which and where it sits.
This page is about what the readings *mean* and what to do about them.

Every panel turns its border and title **[STALE]** when RX is not streaming, so
you always know whether you're looking at live data or a frozen snapshot. Panels
with extra controls announce it with a highlighted letter in the title; the full
list is in [Keyboard Shortcuts](keys.md#lab-panel-focus-modes).

---

## The measurement banner

The thin strip across the top of every lab preset is a control panel, not a
label. Focus it with `b`. It brings three habits from a bench spectrum analyser,
and they're the difference between reading numbers and actually measuring
something.

### Reference level (`↑` / `↓`, `R` to clear)

Draws a horizontal line across the spectrum at a level you choose, so "above the
line" and "below the line" become something you see rather than something you work
out. It starts at −10 dBFS, moves 1 dB per press, and runs from 0 down to −120.

Use it as a threshold you've decided on: set it at the level a signal has to reach
to be worth chasing, then anything poking above the line is a candidate.

### Trace averaging (`[` / `]`)

Smooths the spectrum across successive FFT frames, from 1 (no smoothing) up to 16.
The default is 5.

This works because noise and signal behave differently under averaging. Noise is
random, so successive frames disagree about it and averaging pulls it down toward
its mean. A real carrier is in the same bin at the same level every frame, so it
stays exactly where it is. Turn averaging up and the floor gets visibly flatter
while the signal doesn't move, which is how you find something a single frame
buries.

The cost is reaction time. At 16 the display is smooth and calm and about a
second behind reality, which is wrong for watching a burst and right for measuring
a steady carrier. Drop it back to 1 when you need to see something happen.

### Reference trace (`C` to capture, `C` again to clear)

Captures the current spectrum and keeps it on screen as a ghost behind the live
trace. This is the before/after tool: capture, change one thing (an antenna, a
filter, a length of coax, a gain setting), and read the difference directly off
the screen instead of trying to remember what the floor looked like a minute ago.

Averaging first, then capture, gives the cleanest comparison. Two averaged traces
differ by what you changed; two single frames differ by that plus a lot of noise.

### The marker bar

The strip along the bottom is the read-out half of the same idea. It shows your
placed markers with frequency and level, and the **Δ** between two of them in both
axes at once, which is the fastest way to answer "how far apart, and how much
weaker?" without arithmetic.

Each bench adds its own field. Signal quality carries occupied bandwidth and a quality
verdict, Gain & noise reads the ADC window (clip ceiling, peak, headroom, the noise floor
the FFT measured with the bandwidth it was measured in, SNR), and IQ balance shows how
far the image sits under the carrier, where the image scope can say (a dash where
it cannot). The clip ceiling is 0 dBFS on most radios; one whose tuner clips
before its converter shows that lower figure and names the part, and headroom
counts from there. A radio whose own AGC holds the level below full scale (the
Airspy HF+'s digital AGC does) shows `HOLD` instead of `CLIP`, as a span
(`-14 to -12`) where the radio holds at different levels depending on what came
before, with headroom counted to the lower one, and a peak that reaches it reads
`HELD`: nothing was lost, the radio quietly turned its own gain
down, and from there on the level is no longer the antenna's. Polite, but not
a measurement.

> **If you build your own preset**, note that the reference line and the ghost
> trace are drawn only in **instrument mode**, which sdrtop turns on for presets
> whose name begins with `lab_`. The banner keys work anywhere, and averaging
> affects the spectrum everywhere, but the two overlays need the name.

---

## RF Front-End Bench · *Gain & noise (`Receiver 2`)*

Three panels that read the whole receive chain as one story. The thesis they
teach: **level climbs stage by stage; the gap between signal and noise is the SNR
set at the antenna; gain only positions that gap in the ADC window.** It never
improves it. Each panel restates one face of that.

> **A note on the levels.** No radio here is power-calibrated, so the lineup's
> levels are **dBFS at the converter**, walked back from the *measured* ADC level
> through the *known* stage gains. That is why the antenna reads something like
> `-75 dBFS` rather than a dBm figure it has no right to: exactly right for
> staging decisions, and not a wattmeter reading. Likewise the linearity figures
> below are datasheet-anchored estimates, not lab measurements. Both are labelled
> as such in the panel.

### RF Diagnostics *(focus `d`)*

- **Gain lineup**: the signal level after each stage (ANT, LNA, MIX, VGA, ADC),
  with each stage's gain in the middle column. You can watch the signal climb by
  each stage's gain and land at the measured ADC level. On a radio sdrtop has no
  noise model for, the stages are the ones the driver names, with the gains they
  are actually set to.
- **Gain staging**: one gradient bar per gain stage the device has, each `n / max`
  against that stage's own ceiling, each with a `┊` tick marking the *optimal*
  target. The `opt` line reads whether you're at the optimum, or names the value
  it wants for every stage.
- **Noise figure**: each stage's own NF as a bar, and the Friis **system total**
  beneath. The system total can sit *below* the worst single stage, because the
  LNA's gain suppresses the noise of everything after it. That's the whole point
  of leading with a low-noise amplifier.
- **Sensitivity**: **MDS** (Minimum Detectable Signal,
  `−174 dBm/Hz + 10·log₁₀(BW) + NF`) plus a noise-floor trend sparkline with its
  ±dB/60s spread. Narrowing the BB filter or lowering the NF improves (lowers) the
  MDS.
- **Noise step**: what the `K` measurement found, when you've run one. Described
  [below](#the-noise-step-k).
- **Verdict**: a plain-language read of the staging (`WELL-STAGED`, `HOT`,
  `CLIPPING`, `HELD`, `UNDER-UTILISED` and so on) with the action chips beside it.

**Two of those blocks need numbers no driver reports**, and only those two. Noise
figure and MDS are computed from each stage's own noise figure, which sdrtop
knows for the HackRF and for nothing else. On any other radio those two blocks are
replaced by one line saying which fact is missing and why, and the rest of the
bench, which is measured rather than modelled, stays exactly as it is. The bench
used to go blank instead: three lines on a twenty row panel.

### Gain-Staging Level Diagram

The lineup drawn as a picture. The chain itself is the x-axis, `ANT ─┤LNA├─┤MIX├─┤VGA├─▶ ADC`,
with each stage's gain written under it, and the signal is a **staircase** on
it: the level holds between two stages and steps up (or down, for the HackRF's
mixer) at each one. Every step is a stage's gain, every tread is the level at
that point, written just above it. The y-axis is dBFS, the converter's own
scale, with the signal walked back from the peak the converter measured. Nothing
here is calibrated to the antenna, so the antenna's figure is a level relative
to full scale, not a power in dBm.

At the right edge sits the **converter's window**, the one place where every
line is measured:

- where this radio clips (red): 0 dBFS at the ADC on a HackRF, about −3 dBFS at
  the tuner's output on an [Airspy R2](hardware.md#airspy-r2), which flattens
  the signal before the converter's own rails;
- the signal (`●`), and the **headroom** between it and the clip;
- the **floor** the FFT measured, with the bandwidth it was measured in in the
  legend, and how far the signal sits **over the floor**. A signal under it
  says it is under it, in amber, rather than showing a negative distance.

Solid lines are measured, a dashed one is modelled. On the HackRF, whose stages
come with noise figures, the noise is walked back from the measured floor
through them (`┄`): watch the gap between it and the signal shrink at the first
stage and then stay put, because gain moves the SNR up the chain but never
improves it. **On a radio whose stages don't come with noise figures** (an
Airspy R2, anything through SoapySDR) that walk would have to be made up, so it
isn't drawn, and the panel says so on its last line.

A stage whose gain isn't a decibel figure right now, outside the band it was
measured in or while the AGC is driving it, reads as its step or `AGC` under the
axis, and the staircase starts after it rather than guessing what came before.
[Freeze the bench](#auto-gain-and-freeze) and the whole picture is held, every
stage and the floor included.

### ADC Loading

How much of the converter the signal actually uses:

- **The bit ladder**: one column for every 6 dB below full scale, which is one
  bit of the converter, the least significant at the left and full scale at the
  right. Each is as tall as its share of the samples, with the percentage on top.
  A HackRF on broadcast FM at a modest gain can turn out to be using three of its
  eight bits, which is the kind of thing a ladder says at a glance and a bell
  never did.
- **The axis** is dBFS, the same scale as every other level on the bench. The
  green stretch is where the staging verdict wants the peak, `▲` is the peak and
  `△` the rms. On a radio that clips before its converter (an
  [Airspy R2](hardware.md#airspy-r2), at about −3 dBFS in its tuner) a red line
  runs through the ladder where it clips, so a band past it is plainly past it.
- **The verdict** and how far the peak sits from where the radio clips, then the
  **loading**: `peak` in dBFS and in the converter's counts, `rms` and the
  **crest** factor, both per axis because the converter digitises I and Q
  separately (a constant envelope reads 3 dB), how many **bits** the peak reaches,
  and the **clips** counted at the converter's rails. A clip in a tuner touches no
  rail, so on the R2 a line under it says the counter cannot see it.
- **Linearity** *(modeled)*: P1dB headroom, IIP3 / IMD3, and SFDR against the
  honest 8-bit ceiling (`6.02·8 + 1.76 ≈ 50 dB`). These need a two-tone source to
  measure for real; here they're gain-adjusted datasheet estimates for guidance,
  shown only on the HackRF they describe.

### Auto-gain and freeze

Focus RF Diagnostics with `d`, then:

- **`A`, auto-gain.** When the chain is off-optimal, one press jumps LNA and VGA
  to the staging target (signal about 8 dB under the clip ceiling, so around
  −8 dBFS on most radios, no clip), filling LNA first to
  protect the noise figure. Once you're already at the optimum, pressing `A` again
  **latches a continuous auto-track** that re-nudges the gain when the level
  drifts. Press once more to unlatch. Touching the gain manually drops the latch
  immediately, so it never fights you.
- **`⎵` or `F`, freeze.** Holds the histogram and level diagram on a snapshot so
  you can study them while RX keeps running. Both panels show `[FRZ]` in their
  title. Press again to go live.

### The noise step (`K`)

Everything else on this bench reports. This one **measures**.

Press `K` with RX running and sdrtop walks the front gain stage across its
settings, waits at each for the display to settle, averages the noise floor
there, and moves on. Six settings takes about five seconds on a HackRF. While it
runs the block shows which stage it is on and how far along it is.

What comes back looks like this:

```
├╴ NOISE STEP ╶────────────────── measured at 92.800 MHz
 knee  LNA 24 dB and up                       0.80 dB/dB
 floor ▁▁▂▃▅█                             -88 → -66 dBFS

 Under 24 dB the converter sets the floor, not the RF.
 Not a noise figure: that needs a known source.
```

**The knee is the number to read.** Below it, adding gain barely moves the noise
floor, because what you are looking at is the converter's own noise and you are
simply not driving it hard enough. Above it, every dB of gain lifts the floor by
about a dB, because now you are amplifying real noise from the front end and the
antenna along with everything else. The knee is the boundary: the lowest gain at
which your radio, and not its ADC, decides how faint a signal you can hear.

Under the knee you are throwing sensitivity away. Over it you are only spending
headroom. That is the whole of it.

The figure beside the knee is the slope measured **above** it. It should be near
1.0. If it is well under, the front end has not fully taken over even at the top
of the stage's range.

**The frequency in the heading is part of the reading.** The knee moves with the
band, because a quiet band has less noise coming in and needs more gain before
the front end clears the converter. On one HackRF it sat at LNA 24 dB on a busy
FM channel and at 32 dB on a quiet stretch of UHF, on the same afternoon and the
same antenna. So the block always names where the measurement was taken, and a
reading left on screen after you retune is telling you about somewhere else.

**What it is not.** It is not a noise figure. A noise figure says how much noise
your receiver adds in absolute terms, and getting one means putting a *known*
source at the input, which sdrtop has no way to do and no way to ask you for. The
knee tells you where the converter stops limiting you. It says nothing about how
good the front end is once it takes over. The two sit next to each other on the
panel on a HackRF precisely so you can read the modelled figure and the measured
knee as the different things they are.

Stopping early with `K` reports nothing at all. Two points do define a line, and
printing that line as the result of a measurement you interrupted would be the
instrument answering a question it was not allowed to finish asking.

However you leave the sweep, by finishing, by stopping it, by stopping RX or by
quitting sdrtop, the stage goes back where it was. Quitting mid-sweep also saves
the gain you chose rather than the step it happened to be parked on.

---

## IQ Bench · *IQ balance (`Receiver 1`)*

Everything here is about **quadrature**: whether the I and Q channels your radio
produces are really equal in amplitude and really 90° apart. When they're not,
every signal grows a mirror image on the opposite side of centre, and a spectrum
with fake signals in it is worse than a noisy one.

Three panels ask the same question three ways, and then the focus mode lets you
do something about the answer.

### IQ Diagnostics *(focus `i`)*

Three blocks and a verdict, top to bottom.

**DC at the LO.** A DC offset is a tone at 0 Hz, right in the middle of your
spectrum, and what decides whether it matters is how loud it is *against the
signal*. The same offset is a small spike under a strong station and the loudest
thing in the capture under a weak one: a HackRF on FM at a modest gain can carry
its DC 12 dB over everything it receives. So the block gives the DC's level in
dBFS, how far over or under the signal's rms that is, a scale showing it
(green well under, amber closing in, red within 10 dB or over), and the I and Q
split as a percentage of full scale.

**Quadrature.** Two analog **null-meters**, where the centre tick is perfect and
the needle deflects by how far off you are:

- **gain**: whether I and Q carry the same power.
- **phase**: whether I and Q are exactly 90° apart.

Either one puts a mirror image of every signal on the opposite side of the LO, and
right under them is how far down that image should be, the **IRR** those two
predict, as a red-to-green bar with its 60-second trend. 30 dB or more is good;
below 20 dB the images become a problem. It is a prediction from the imbalance
figures; the [Image-Rejection Scope](#image-rejection-scope) is where an image is
measured.

**The verdict** says the one thing that matters most, in plain words, and the chips
under it show which corrections are on, with how long ago the auto-cal was taken.
The DC counts as high when it is over 2 % of full scale *or* within 10 dB of the
signal.

### IQ Constellation

Where the diagnostics give you numbers, the constellation gives you *shape*, and
shape is often faster to read.

- A **circle** centred on the origin means healthy quadrature.
- An **ellipse** means amplitude imbalance (I and Q at different levels).
- A **tilt** means phase imbalance (I and Q not 90° apart).
- The cloud's **offset** from centre is the DC offset, marked with a crosshair.
- A cloud pushed **flat against the edge** is clipping.

The picture is **square**, so a circle is drawn as a circle and an ellipse means
the hardware, not the terminal's cells. It is **zoomed to the cloud**: the outer
ring sits about 5 dB above the signal's rms, on a 3 dB step, and the rings inside
it are 6 dB apart, each labelled in dBFS. Full scale is drawn in red when it is in
view, and so is the clip of a radio that clips before its converter (an
[Airspy R2](hardware.md#airspy-r2) at about −3 dBFS, dashed). The zoom follows a
growing signal at once but waits a second before zooming in on a fading one, so
the picture does not flicker between two scales, and it holds still while the
cloud is frozen with `F`. A weak signal on an 8-bit radio, zoomed this far, shows
the converter's grid of counts: that is the quantisation, not a fault.

The cloud is coloured by **point density**, a phosphor-scope look where sparse
edges are cool blue and the dense core glows orange. A measured **imbalance
ellipse** is fitted over it: its axis ratio is the amplitude imbalance and its tilt
the phase imbalance, the same two faults the diagnostics quantify, drawn straight
onto the cloud. The numbers for those, and for the DC offset, are one panel to the
left and nowhere else.

Beside the square is the one thing only this panel measures, the **envelope**:
how far from the centre the points sit. A radial profile from the centre out to the
outer ring, the envelope's rms in dBFS, and its **spread**: 0 % is a perfect ring
(a constant envelope, like FM), about 52 % is Gaussian noise, and a clipped signal
piles up at the outer edge.

### Image-Rejection Scope

The empirical check on everything above. The other two panels *calculate* image
rejection from the measured imbalance figures. This one goes and looks at a real
image.

It finds the strongest carrier in the frame (`▼`) and the bin that mirrors it
about the centre, the LO (`▽`), and draws the gap between them where it is: the
carrier's level is carried across to the mirror's column as a dashed tick, and the
drop from there down to the mirror is the image rejection, written beside it. The
chart's scale follows the capture, from just under the noise floor to just over
the strongest bin. Under it, the carrier and the mirror with their frequencies and
levels, and one verdict:

- **IMAGE 32 dB DOWN**: the mirror stands clear of the floor, so the image is
  measured. The measured counterpart of the computed IRR next door.
- **IMAGE ≥ 17 dB DOWN**: the mirror is the noise floor, so the real image is
  somewhere under it and the figure is a lower bound. A weak carrier can only ever
  give you this; a stronger one, or more gain, turns it into a measurement.
- **MIRROR 5 dB DOWN**: within 6 dB of the carrier, which no working front end
  lets an image do. It is far more likely a signal of its own sitting there, so
  there is no image figure.
- **NO IMAGE TO MEASURE**: the carrier is part of a signal that runs across the LO,
  say a station you tuned to, so the "mirror" is the signal's own other half. Tune
  so the signal sits off to one side, a megahertz or two, and its image appears on
  the other.

When this and the computed IRR disagree, trust this one for "what will actually
appear in my spectrum" and the computed IRR for "how imbalanced is the hardware".
They're answering slightly different questions. The DC offset is IQ Diagnostics'
to report; here it is simply the LO's column.

Two more things stop it lying to you. It ignores a small guard band around centre,
so the DC spike can never be mistaken for a carrier. And on the automatic path it
requires the strongest bin to stand at least 10 dB clear of the noise floor;
below that there is no carrier, only noise, and reporting the loudest noise bin as
a carrier would produce an alarming suppression figure about nothing. Place a
marker or pin one with `M` and that gate is bypassed, because an operator
deliberately probing a weak signal outranks the heuristic.

### Correcting, not just measuring

This is the part that's easy to miss: two of the focus keys change the *samples*,
not the display.

- **`D`, DC-block.** Subtracts the live DC estimate from the stream. That
  permanent spike at your centre frequency is the front end's own DC offset, not a
  signal. Turn this on and watch it drop. Worth having on whenever the DC spike is
  interfering with a measurement near centre, which includes the demodulator on
  the signal bench.
- **`C`, auto-cal.** Measures the amplitude and phase imbalance in the current
  sample window and applies the inverse correction from there on. Mirror images
  fade and IRR improves. It's a one-shot snapshot that stays fixed until you press
  `C` again to clear it.

  Because it estimates from whatever is in the window at that moment, capture it
  with a decent signal present rather than on an empty band. The natural workflow
  is: tune to a strong clean carrier, look at the image scope, press `C`, watch
  the image drop, then go back to what you were doing with the correction still
  applied.
- **`F`** freezes the constellation cloud so you can study a shape while RX keeps
  running.
- **`M`** pins the carrier and image markers instead of letting them auto-track,
  so they stay on the signal you chose rather than on whatever is loudest right
  now.

Both corrections are display-session state, not hardware settings, and neither is
saved. A fresh launch is an uncorrected radio, which is the honest default.

---

## IQ Amplitude Distribution · *optional panel (`iq_histogram`)*

> Not in any built-in preset any more; the image scope took its slot in IQ balance.
> Add `iq_histogram` to a [custom layout](presets.md) if you
> want the exact Low/Mid/Clip percentages.

A histogram of incoming sample amplitudes across 32 bins, log-scaled vertically so
both rare strong peaks and the bulk of weak samples are visible at once. The
colour zones are dim (low amplitude, ADC under-utilised), green (healthy) and red
(approaching clipping), and below the chart a numeric breakdown gives the exact
percentages so you can set gain precisely.

**PAPR**, the Peak-to-Average Power Ratio (crest factor) in dB, is estimated from
the distribution. It's a quick fingerprint of *what kind* of signal you're looking
at:

| PAPR | Likely signal |
|------|---------------|
| under 3 dB | CW / FM (constant envelope) |
| 3–8 dB | AM / mixed |
| 8–15 dB | wideband / spread-spectrum |
| over 15 dB | bursty / impulsive |

---

## Signal Characterization · *Signal quality (`Receiver 4`)*

Where the demod panel opposite opens the channel up and reads what is inside it,
this one answers the question you ask first: what *is* that, and how clean is it?
The channel figures come from the same FFT frames the spectrum beside it draws,
taken four times a second: often enough to follow the signal, slowly enough that a
number can actually be read before it changes. Press `x` to focus it, then `C` to
write the current readings to the log.

**The carrier figures wait for a carrier.** Occupied bandwidth, the adjacent
channels and the verdict describe a carrier, and in the gap between two bursts
there is none: measured there, the adjacent channels are noise against noise at
about 0 dB, and the occupied bandwidth is noise spread over the whole span. So
these come only from frames with a carrier in them and are held through a gap of
up to a second; after a second of nothing they are dashes and the verdict says
`NO SIGNAL`. While they are being held the verdict card says for how long
(`held 0.8 s`). The verdict also holds a second before it changes its mind, and a
carrier heard for less than that reads `◌ CARRIER HEARD` while it is judged. The
card keeps one height whatever it says, so nothing above it moves. A
walkie-talkie keying up does not get a new diagnosis every time it takes a breath.

**Radio headline.** Peak-to-noise in dB with a status lamp, and the classifier's
guess at the modulation (`WFM`, `NFM`, `AM`). Green at 20 dB or better, amber down
to 10, red below that. The guess is told apart by occupied bandwidth, which
scatters from one burst to the next. Wider than any one broadcast can be (334 kHz,
Carson's rule with the widest subcarrier), there is no guess at all and the verdict
says `MANY SIGNALS`: on a crowded HF band the measurement walks from one station to
the next, and a whole band is not a very wide FM transmitter, however much it would
like to be. A guess shows once it has held for a second,
a different one has to hold for two to replace it, and the badge stays two seconds
after the carrier goes.

**Signal metrics.**

- **Channel power**, the total power in the occupied channel. Unlike a single bin,
  this does not change when you change the sample rate. It is the same reading as
  the S-meter on the Command Rail, through the same meter movement and marked `~`
  the same way, so the two never disagree.
- **Peak**, the strongest real bin near centre and its frequency. "Real" because
  the front end's own LO leakage sits exactly at centre and is usually the tallest
  thing there; naming it would report the tuned frequency as a station every time
  the channel is quiet. The search also stays near centre, so this row cannot
  wander off and name somebody else's transmitter.
- **Noise floor**, in dBFS per bin, with the same figure as a **density** in
  dBFS/Hz next to it. The per-bin number is the one that matches where the noise
  sits on the trace, but it is not a property of your radio: it rises with the bin
  width, so it changes when you change the sample rate and says as much about the
  analyser as about the receiver. The density divides that out. Measured on one
  station at two rates, the floor read −81.1 dBFS at 2 Msps and −73.8 at 10 Msps,
  which looks alarming and is entirely the wider bin. As densities the same two
  readings are −112.8 and −112.5 dBFS/Hz: the same radio, correctly reported as
  the same radio.
- **Occupied BW**, the 99 % occupied bandwidth (ITU-R SM.328), measured over the
  carrier rather than the whole captured span. See the note below.
- **Peak hold**, the highest level seen since the trace was last reset.

**Adjacent channel (ACPR).** How far down the neighbouring channels sit relative
to this one, one bar per side, more fill meaning closer to the carrier and so
worse. The ratio is averaged over half a second, up and down alike: measured on
the noise in the neighbouring bands it scatters a few dB from frame to frame even
on a rock-steady carrier, and a bar that chased every scatter would mostly be
showing you the noise. Averaged in power, that still springs up in well under a
second and settles back gently. The digits beside it change four times a second.
When the carrier goes the bar holds for the same second the other carrier figures
do, then its digits turn to a dash and the bar sinks back to empty in the stale
colour over a few seconds: a needle returning to rest, not a reading. The row
names the spacing the bar was measured at, even in the gap after it. The
spacing follows the modulation (±200 kHz for broadcast FM, ±25 kHz for
NFM, ±9 kHz for AM) and the row labels name whichever offset was actually used, so
they cannot drift apart. The bar's floor is sdrtop's own display range, not a
regulatory mask: no limit is being asserted, the bar just shows the measurement.

**Spectral shape.** A 60-second trend of carrier-to-noise, and the crest factor
(peak-to-RMS) of the ADC stream, which is the same honest "constant-envelope
versus peaky" proxy the RF bench shows.

**Verdict.** A plain-language read of the four zones above: modulation, SNR, ACPR
and occupied bandwidth. Rule-based and nothing more. There is no model here and no
demodulation; it is a sentence describing numbers that are already on the panel.

### About occupied bandwidth

**A broadcast station reads narrower than its allocation, and that is correct.**
Broadcast FM is allocated 200 kHz and designed around 180, but a real programme
measures somewhere around 85 to 120 kHz of 99 % occupied bandwidth. The reason is
that the time-averaged spectrum of an FM signal is strongly peaked at the carrier:
the deviation only reaches its extremes on loud passages, so most of the energy
spends most of the time near the middle. The allocation is what the signal may
occupy at its widest; this is what it occupies now. Carson's rule gives the first
number, a spectrum analyser gives the second, and they are not supposed to match.

**It varies with sample rate.** The same station measured 101.6 kHz at 2 Msps and
65.9 kHz at 5 Msps, because a wider span means coarser bins and a different view of
the carrier's skirts. That is worth knowing before you compare two readings: they
are only comparable at the same rate. It also drags the modulation badge with it,
so a broadcast station on a very wide span can classify as `NFM`. If you are using
the demod panel opposite, force the mode with `T` rather than trusting the badge.

---

## FM MPX · Demod · *Signal quality (`Receiver 4`)*

This panel actually demodulates the channel and reads out what is inside it. It is
a **measurement instrument, not a receiver**. There is no audio anywhere in
sdrtop, and this panel exists to tell you things about a transmission that a
spectrum plot cannot: how hard it is deviating, whether it is in stereo, which
subaudible tone opens the squelch, what the station calls itself.

Press `m` to focus it. It only runs while the panel is actually on screen, so it
costs nothing on any other layout. That means it also works in a
[custom preset](presets.md) of your own, as long as you list
`fm_demod` in it.

**What it shows depends on the modulation.** Each mode is shown only what it
actually has, rather than a fixed grid where most rows read as permanently empty:

| Mode | Sections |
|------|----------|
| **WFM** (broadcast) | MPX baseband · Pilot/stereo · Deviation · RDS |
| **NFM** (voice) | Deviation · CTCSS |
| **AM** | Depth · Carrier |

There is no audio section, and there is no audio. A section a mode does not have is
simply absent rather than sitting there empty.

**It holds still on a bursty signal.** A mode shows once it has held for a second,
has to hold for two to replace another and is kept two after the carrier, so one
odd frame from the classifier does not swap the broadcast sections for the NFM
ones. When the signal drops under the detection threshold,
the last readings stay where they are and the headline turns from `DEMOD LOCK`
to `◌ DEMOD HELD`, counting how long they have been held. A second and a
half without a new one and they go, and the headline says why. A figure that is
missing shows as a dim dash on its own row instead of taking the row with it, so
nothing below it moves. A panel that does a little dance every time a walkie-talkie
unkeys is exciting, just not in the way an instrument should be.

- **MPX baseband**: the demodulated composite from 0 to 60 kHz as a braille
  profile, with ticks at 19 k (pilot), 38 k (stereo difference) and 57 k (RDS).
  This is the audio-side spectrum, not the RF one. You are looking *inside* the FM
  channel. The picture is averaged over half a second so it moves smoothly
  instead of jumping with every update; the pilot figures below are measured from
  the spectrum as it is, not from the smoothed picture.
- **Pilot / stereo**: `● STEREO`, `◐ MARGINAL` or `○ MONO`, plus the pilot's own
  deviation and its **injection percentage**. Broadcast practice is 8 to 10 %, so
  a figure well outside that is a transmitter fault rather than a reception
  problem.
- **Deviation**: peak (quasi-peak, with decay) and RMS deviation measured *about
  the carrier*, plus a **Carrier** row giving how far the carrier sits from the
  centre of the demodulated channel. Measuring about the carrier is what makes a
  mistuned radio report its tuning error there instead of inflating the modulation
  figure. The bar is drawn against the mode's nominal limit, ±75 kHz for broadcast
  and ±5 kHz for NFM, and turns amber then red as you approach and exceed it.
- **CTCSS**: the subaudible tone that opens a repeater's squelch, identified from
  the standard 40-tone table, with its deviation and the **margin** it beat its
  nearest rival by. It needs half a second of unbroken audio, so it shows
  `◌ SEARCHING n%` while filling its window. That is not the same thing as
  `○ NO TONE`, and the panel says which one it means.
- **Depth** (AM): modulation depth with positive and negative peaks reported
  separately, because they fail differently. A negative peak approaching 100 %
  pinches the carrier off and splatters.
- **RDS**: see below.

### RDS

For broadcast FM the panel decodes the **RDS** data stream on the 57 kHz
subcarrier, and shows:

- the **Programme Service** name, the 8-character station name,
- **PI**, the station's unique hex identity code,
- **PTY**, the programme type (`Pop Music`, `News`, `Culture` and so on),
- **Traffic** flags beside the programme type, when TP or TA is set,
- **Groups**, which is two numbers: the total accepted on this channel, and after
  it the length of the current unbroken run. `Groups 1400  +1` means fourteen
  hundred groups have been decoded here, but the run in progress is one group long,
  so something keeps interrupting reception. When the two agree, only the total is
  shown.
- **RadioText**, the free 64-character field stations use for now-playing
  information, wrapped across two rows.

**Accented characters come through.** RDS is not ASCII: it uses its own character
set (IEC 62106 table G0), where the accented Latin letters live above the ASCII
range. A Hungarian title reads as a Hungarian title rather than one with holes
punched in it.

The headline distinguishes three states. `● NAME` is the answer. `◌ DECODING`
means bits are arriving and the name is a second or two away. `○ NO RDS` means
that as far as we can tell this station carries none.

A name does not outlive its station. RDS accumulates over seconds, which is
exactly what lets it sit on screen looking confident after reception has stopped,
so the panel ages it: a few seconds without a new group and the headline says how
old it is, and past thirty seconds nothing is shown at all. Retuning drops
everything at once, since none of it describes the new frequency.

Nothing is shown until it has been **confirmed twice**. RDS has only a block CRC
and no error correction beyond it, so a block can pass its check with an undetected
error. Requiring a second identical sighting costs under a second and removes
almost all of the wrong-glyph flicker a naive decoder shows on a weak signal. The
same rule guards the PI code, because a random 26-bit window matches a valid block
pattern often enough that a single hit is not evidence of anything.

RDS needs a decent signal. It rides about 20 dB below the main programme audio, so
a station you can hear perfectly may still decode nothing.

**If the demod says blocks were dropped, believe it.** RDS and CTCSS both need an
unbroken run of samples, and the demod is fed through a small queue that discards
blocks when the machine cannot keep up. A busy host and a station without RDS look
identical otherwise, so the panel counts them and says so, noting that RDS and
CTCSS need a clean run. The line appears only while blocks are actually being lost,
so an old glitch does not leave a warning pinned there. A lower sample rate, or
closing whatever else is competing for the CPU, is the fix.

### Tuning the demod channel

While focused (`m`):

- `Space` toggles the demod on and off.
- `←` / `→` move the demodulated channel ±25 kHz *within* the captured spectrum,
  without retuning the radio.
- `P` snaps the channel onto the strongest carrier in view. The DC spike at centre
  is excluded, or it would snap to the radio's own LO leakage every time.
- `0` re-centres the channel.
- `T` forces the demodulator: auto → WFM → NFM → AM → auto. **You will usually want
  this.** The automatic classifier measures occupied bandwidth across the whole
  captured span, which on a wide span reads as WFM for nearly anything. That is
  fine as a rough badge but too coarse to pick a demodulator. A forced mode is
  marked `✱` so a reading is never mistaken for the classifier's own conclusion.
- `C` writes a snapshot of the current readings to the log.

If the panel warns **"on DC spike"**, the channel is sitting on the front end's own
DC offset and LO leakage, which swamps the phase detector and inflates every
deviation figure. Either enable the DC block (`D` in IQ balance) or walk the channel
off centre with `←` / `→`.

---

## Timing Bench · *USB & timing (`Receiver 3`)*

The question no other bench can answer: is your computer keeping up with the radio
in real time? Samples arrive in steady USB bursts, and your machine has to keep
taking them or the buffer backs up and samples drop.

**There are two ways a radio hands samples over, and this bench measures them
differently.** A HackRF or an RTL-SDR *pushes*: the driver calls you when a block
is ready, and the useful question is whether your code answered in time, so the
panel talks about callbacks and deadlines. A SoapySDR device *pulls*: your code
asks for a block and waits until there is one, and there is no deadline to miss,
because a slow reader just waits less. The useful question there is the opposite
one, how much of each cycle was spent **waiting** rather than working, and that is
what the panel measures instead. The words on screen change with the transport.

Getting this wrong is what made an entirely healthy SoapySDR link report a
permanent USB overload: it was being marked late against a deadline that does not
exist on a pull backend.

### Timing Diagnostics *(focus `t`)*

- **Arrival** (push): a block every so many milliseconds against the period
  your sample rate implies, the jitter around it and its 60-second trend.
- **Late by** (push): how late the worst callbacks came (p95, p99, peak), each a
  bar on one logarithmic scale from 1 % of the budget to ten times it, with the
  time and the multiple written beside it, and a count of the ones past it. The
  **budget** is the radio's slack, and the nameplate says which: a slice of one
  block where the driver holds its USB transfer until the callback returns (the
  HackRF, an RTL-SDR), and the depth of the driver's queue less one block where it
  queues blocks before calling back (an [Airspy R2](hardware.md#airspy-r2):
  libairspy keeps eight, so a callback may come seven blocks late, about 46 ms at
  10 Msps, before anything is lost).
- **Read loop** (pull): the share of the read loop spent working rather than
  waiting. Low is healthy and means there is slack. Climbing toward 100 % means
  the reader has stopped having spare time, and the driver's buffer is about to
  start filling, which is the same warning a late callback gives on the other
  transport.
- **Sample clock**: the rate the radio actually delivers and how many ppm that is
  off the one you set, measured over a long baseline: the one clock figure on the
  bench, also the `CLOCK` in the bars above and below. A steady few ppm is a
  crystal being a crystal, a wandering figure is not. Then the throughput it
  produces, with its trend.
- **The verdict** names its own reason rather than just a grade, so
  `Sample clock is off the configured rate / 182 ppm out, nothing lost` is
  distinguishable at a glance from `Overrun, samples lost`. A clock that is a
  little off and a link that is dropping samples used to read the same.

`R` resets the session jitter peak, `C` clears the history.

### Interval Strip Chart

Every point is one real block, plotted by how far its arrival drifted from the
expected interval. Late deliveries climb, early ones dip, and anything past the
deadline band gets tagged. A host hiccup becomes something you watch happen rather
than something you infer from a counter afterwards. The panel is titled
**Callback Interval** or **Read Interval** to match the transport, and so is its
caption.

This is the panel to have open when you suspect the problem is your computer and
not your radio. A scheduler stall, a CPU frequency step, another process waking up
on a timer: they all have a shape here, and the shape repeats.

### Hardware Vitals *(focus `v`)*

The supporting cast, all on a 60-second rolling window:

- **Sample drops**: per second plus the session total. Non-zero means something
  upstream is not keeping up, and a line under it says who: the **driver**
  (samples its library or the radio's link lost before handing them over) or
  **sdrtop** (its own intake had no room for a block).
- **ADC saturation**: how often samples hit the ADC ceiling, with the session
  peak.
- **CPU load and RAM**: sdrtop's own use. CPU is a system-wide percentage, so
  100 % means every core is maxed and a healthy figure on a multi-core machine is
  well under it.
- **USB link**: bus throughput, link utilisation against the device's real
  ceiling, and USB errors (zero-length transfers, usually a cable or hub problem).
  Bus throughput is what crossed the link, which is not always what arrived: an
  Airspy R2 packing its samples to 12 bits carries 3 bytes for every 4 its library
  hands over. Through SoapySDR the wire is the driver's secret, so the line says
  **Delivered** instead and counts what reached sdrtop.
  The errors are coloured by recent rate rather than session total, so one old
  glitch doesn't pin the panel red forever.
- **Ring buffer**: peak fill and overrun margin. This is the leading indicator.
  Fill climbing toward the ceiling is the warning that drops are about to start,
  and it arrives before the drop counter does.

`R` resets the session drop counter, `C` clears the sparklines.

---

## Sweep · *Band sweep (`Sweep 1`)*

Your radio sees only as much spectrum at once as the sample rate covers, ±10 MHz
at 20 Msps on a HackRF and rather less on an RTL-SDR. **Band sweep** maps a wider
band by retuning across it: at each step it measures briefly, records the peak and
mean level, then moves on, stitching the results into one curve with frequency on
the x-axis. Known bands are labelled from the band plan, and the cursor reads out
the level and band at any point.

Because a full cycle takes a couple of seconds, sweep is for *finding* a signal,
not watching it. Once you spot one, focus the panel with `g` and press `Enter` to
tune straight to the cursor frequency in normal RX.

While focused, `S` and `E` set the start and end frequency live, `+` and `-`
adjust the dwell, and `M` switches the curve between peak and mean. Peak finds
brief transmissions that a mean would average away; mean gives a stabler picture
of what is continuously present. The band and dwell also live in the config, see
[Configuration](config.md#sweep-scanner).

**Signal Metrics** *(focus `n`)* sits alongside with a compact read-out for
wherever the radio is actually parked: peak-to-noise, channel power, noise floor
and occupied bandwidth. `C` logs a snapshot. It's the same family of numbers as
the signal bench, sized to fit next to a scan.

`micro_sweep` (`Sweep 2`) gives the same scan as a compact field list.

---

## Using the lab presets in practice

A typical setup flow, switching presets as you go:

1. Tune to your target and start RX (`Space`).
2. In **IQ balance (`Receiver 1`)**, watch the **constellation**: adjust LNA and VGA until the
   cloud is a bright, well-filled ring with no red ring in sight (full scale, or
   the radio's clip, only appears when the signal gets near it). Glance at **IQ Diagnostics**:
   a centred needle on each null-meter, an image predicted 30 dB or more down,
   and the DC well under the signal mean clean quadrature. If the DC is in your
   way, press `D`; if the images are, park on a strong carrier and press `C`.
3. In **Gain & noise (`Receiver 2`)**, focus **RF Diagnostics** (`d`) and press `A` to auto-stage
   the gain, then read **NF** and **MDS** to confirm the receiver is sensitive
   enough for what you're chasing. Watch the **ADC Loading** ladder put the peak
   on the green stretch, short of where the radio clips.
4. In **USB & timing (`Receiver 3`)**, confirm the timing verdict is Good or Excellent, and
   that the ring-buffer fill isn't trending upward, before committing to a long
   run.
5. On the bench you're actually working at, set the banner up: averaging up if the
   thing you're chasing is near the floor, and a reference trace captured before
   you change anything.
6. During a long capture, keep **Hardware Vitals** in view. CPU, buffer fill and
   drops together tell you whether the run is sustainable, and buffer fill tells
   you first.

---

← [Back to all screens](screens.md)
