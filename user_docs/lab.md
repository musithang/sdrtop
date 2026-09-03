# The Lab Presets

← [Back](README.md)

sdrtop's **lab presets** are the bench-engineer views. Instead of just a live
spectrum, they surface the measurements sdrtop can derive about your receiver's
*signal quality* and *hardware health*. They're built for setting up a clean
capture and for watching for trouble during a long run.

Four benches in the **Lab** section, plus the band sweep in **Sweep**. The key
is the number to press while that section is active:

| Key | Preset | The question it answers |
|-----|--------|--------------------------|
| `Lab 1` | **Lab IQ** | Is the quadrature clean, and can I make it cleaner? |
| `Lab 2` | **Lab RF** | Is the front end staged properly for what I'm chasing? |
| `Lab 3` | **Lab Timing** | Is my computer keeping up with the radio? |
| `Lab 4` | **Lab Signal** | What is that signal, and what's inside it? |
| `Sweep 1` | **Lab Sweep** | What's out there across a band too wide to see at once? |

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

Each bench adds its own field. Lab Signal carries occupied bandwidth and a quality
verdict, Lab RF reads the ADC window (clip ceiling, peak, headroom, noise, SNR),
and Lab IQ shows the measured carrier-to-image suppression.

> **If you build your own preset**, note that the reference line and the ghost
> trace are drawn only in **instrument mode**, which sdrtop turns on for presets
> whose name begins with `lab_`. The banner keys work anywhere, and averaging
> affects the spectrum everywhere, but the two overlays need the name.

---

## RF Front-End Bench · *Lab RF (`Lab 2`)*

Three panels that read the whole receive chain as one story. The thesis they
teach: **level climbs stage by stage; the gap between signal and noise is the SNR
set at the antenna; gain only positions that gap in the ADC window.** It never
improves it. Each panel restates one face of that.

> **A note on the levels.** The HackRF is not power-calibrated, so the dBm figures
> here are *modeled and relative*: the lineup is back-computed from the *measured*
> ADC level through the *known* stage gains, anchored to a documented
> `0 dBFS = 0 dBm` reference. They're exactly right for staging decisions, and
> they're not a wattmeter reading. Likewise the linearity figures below are
> datasheet-anchored estimates, not lab measurements. Both are labelled as such in
> the panel.

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
  `CLIPPING`, `UNDER-UTILISED` and so on) with the action chips beside it.

**Two of those blocks need numbers no driver reports**, and only those two. Noise
figure and MDS are computed from each stage's own noise figure, which sdrtop
knows for the HackRF and for nothing else. On any other radio those two blocks are
replaced by one line saying which fact is missing and why, and the rest of the
bench, which is measured rather than modelled, stays exactly as it is. The bench
used to go blank instead: three lines on a twenty row panel.

### Gain-Staging Level Diagram

The lineup drawn as a picture: two traces climbing the stage axis, **signal**
(filled) and **noise floor** (line). The vertical gap between them is the SNR.
Reading it left to right shows that gap being *carried up* the chain and parked
inside the ADC window, never widened. Dashed reference lines mark the ADC clip
ceiling and the 8-bit floor, and the band between the traces is shaded as
**usable dynamic range**. If the noise ever climbs above the signal, that band is
flagged **buried** rather than left blank, because an empty gap and a negative one
are very different situations.

### ADC Loading

How hard the 8-bit ADC is actually being driven:

- **Signed sample histogram**: a centred bell from −FS to +FS. A healthy signal
  fills the middle without piling up on the rails; the rails turn amber, then red,
  as clipping appears. A lopsided bell reveals a DC offset.
- **Headroom** bar: clip headroom in dB, with the optimal tick.
- **Loading**: `peak` and `rms` in dBFS and ADC counts, **crest** factor,
  **effective bits** (ENOB), and the **clip-event** count for the window.
- **Linearity** *(modeled)*: P1dB headroom, IIP3 / IMD3, and SFDR against the
  honest 8-bit ceiling (`6.02·8 + 1.76 ≈ 50 dB`). These need a two-tone source to
  measure for real; here they're gain-adjusted datasheet estimates for guidance.

### Auto-gain and freeze

Focus RF Diagnostics with `d`, then:

- **`A`, auto-gain.** When the chain is off-optimal, one press jumps LNA and VGA
  to the staging target (signal around −8 dBFS, no clip), filling LNA first to
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

## IQ Bench · *Lab IQ (`Lab 1`)*

Everything here is about **quadrature**: whether the I and Q channels your radio
produces are really equal in amplitude and really 90° apart. When they're not,
every signal grows a mirror image on the opposite side of centre, and a spectrum
with fake signals in it is worse than a noisy one.

Three panels ask the same question three ways, and then the focus mode lets you
do something about the answer.

### IQ Diagnostics *(focus `i`)*

Each *deviation-from-ideal* is drawn as an analog **null-meter**: a centre tick is
"perfect", and a coloured needle deflects left or right by how far off you are,
with the span between centre and needle filled. A glance reads the state; the
number beside it reads the exact value.

- **DC I / DC Q**: how far each channel is offset from zero, with a combined **DC
  magnitude** quality bar. A high DC offset puts a fixed tone right in the middle
  of your spectrum.
- **DC spike**: how tall that centre-frequency spike is, in dBFS. Green below
  −40 dBFS.
- **Amp imbalance**: whether I and Q carry the same power. A mismatch creates
  mirror images of signals on the opposite side of centre.
- **Phase imbalance**: whether I and Q are exactly 90° apart. Also causes
  mirroring.
- **IRR**: **Image Rejection Ratio** in dB, as a red-to-green quality bar. This is
  the key quadrature-quality figure. It tells you how far *below* every real
  signal its mirror image appears. 30 dB or more is good, the images are faint;
  below 20 dB and they become a problem.

A contextual hint at the bottom summarises whether anything needs attention,
colour-matched to severity.

### IQ Constellation

Where the diagnostics give you numbers, the constellation gives you *shape*, and
shape is often faster to read.

- A **circle** centred on the origin means healthy quadrature.
- An **ellipse** means amplitude imbalance (I and Q at different levels).
- A **tilt** means phase imbalance (I and Q not 90° apart).
- The cloud's **offset** from centre is the DC offset, marked with a crosshair.

The cloud is coloured by **point density**, a phosphor-scope look where sparse
edges are cool blue and the dense core glows orange, so you can see where the
signal's energy actually concentrates. A measured **imbalance ellipse** is fitted
over it: its axis ratio is the amplitude imbalance and its tilt is the phase
imbalance, the same two faults the diagnostics quantify, drawn straight onto the
cloud. No live numbers sit here on purpose; they're one panel to the left.

### Image-Rejection Scope

The empirical check on everything above. The other two panels *calculate* image
rejection from the measured imbalance figures. This one goes and looks at a real
image.

It finds the strongest carrier in the frame, finds the bin that mirrors it about
the centre (the LO), and reports:

- **CARRIER**: its frequency and level.
- **IMAGE**: the level of the mirror at the reflected frequency.
- **DC spike**: the residual at centre, which is the I/Q offset rather than any
  signal.
- **image supp.**: the gap between carrier and image in dB, which is the measured
  counterpart of the computed IRR next door.

When the two disagree, trust this one for "what will actually appear in my
spectrum" and the computed IRR for "how imbalanced is the hardware". They're
answering slightly different questions.

Two things stop it lying to you. It ignores a small guard band around centre, so
the DC spike can never be mistaken for a carrier. And on the automatic path it
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

> Not in any built-in preset any more; the image scope took its slot in Lab IQ.
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

## Signal Characterization · *Lab Signal (`Lab 4`)*

Where the demod panel opposite opens the channel up and reads what is inside it,
this one answers the question you ask first: what *is* that, and how clean is it?
Everything here comes from the same FFT frame the spectrum beside it draws, so the
two always agree. Press `x` to focus it, then `C` to write the current readings to
the log.

**Radio headline.** Peak-to-noise in dB with a status lamp, and the classifier's
guess at the modulation (`WFM`, `NFM`, `AM`). Green at 20 dB or better, amber down
to 10, red below that.

**Signal metrics.**

- **Channel power**, the total power in the occupied channel. Unlike a single bin,
  this does not change when you change the sample rate.
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
worse. The spacing follows the modulation (±200 kHz for broadcast FM, ±25 kHz for
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

## FM MPX · Demod · *Lab Signal (`Lab 4`)*

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

- **MPX baseband**: the demodulated composite from 0 to 60 kHz as a braille
  profile, with ticks at 19 k (pilot), 38 k (stereo difference) and 57 k (RDS).
  This is the audio-side spectrum, not the RF one. You are looking *inside* the FM
  channel.
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
- **Traffic** flags, when TP or TA is set,
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
deviation figure. Either enable the DC block (`D` in Lab IQ) or walk the channel
off centre with `←` / `→`.

---

## Timing Bench · *Lab Timing (`Lab 3`)*

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

- **Callback timing** (push) or **read timing** (pull): the measured period against
  the period expected at your sample rate, the throughput that implies, and the
  jitter around it.
- **Deadline budget** (push): per-callback deviation percentiles (p95, p99, peak)
  drawn against a deadline that scales with the sample rate, because a callback
  that is 200 µs late is fine at 2 Msps and fatal at 20. A late-callback count
  sits beside it.
- **Read occupancy** (pull): the share of the read loop spent working rather than
  waiting. Low is healthy and means there is slack. Climbing toward 100 % means
  the reader has stopped having spare time, and the driver's buffer is about to
  start filling, which is the same warning a late callback gives on the other
  transport.
- **Sample rate**: host clock drift in ppm and the drift of the measured rate
  against the configured one. This is clock integrity rather than throughput: a
  steady few ppm is a crystal being a crystal, a wandering figure is not.
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
  upstream is not keeping up.
- **ADC saturation**: how often samples hit the ADC ceiling, with the session
  peak.
- **CPU load and RAM**: sdrtop's own use. CPU is a system-wide percentage, so
  100 % means every core is maxed and a healthy figure on a multi-core machine is
  well under it.
- **USB link**: bus throughput, link utilisation against the device's real
  ceiling, and USB errors (zero-length transfers, usually a cable or hub problem).
  The errors are coloured by recent rate rather than session total, so one old
  glitch doesn't pin the panel red forever.
- **Ring buffer**: peak fill and overrun margin. This is the leading indicator.
  Fill climbing toward the ceiling is the warning that drops are about to start,
  and it arrives before the drop counter does.

`R` resets the session drop counter, `C` clears the sparklines.

---

## Sweep · *Lab Sweep (`Sweep 1`)*

Your radio sees only as much spectrum at once as the sample rate covers, ±10 MHz
at 20 Msps on a HackRF and rather less on an RTL-SDR. **Lab Sweep** maps a wider
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
2. In **Lab IQ (`Lab 1`)**, watch the **constellation**: adjust LNA and VGA until the
   cloud is a bright, well-filled ring sitting comfortably *inside* the unit
   circle (smearing out to the edge means clipping). Glance at **IQ Diagnostics**:
   a centred needle on each null-meter, IRR above 30 dB and DC spike below
   −40 dBFS mean clean quadrature. If the DC spike is in your way, press `D`; if
   the images are, park on a strong carrier and press `C`.
3. In **Lab RF (`Lab 2`)**, focus **RF Diagnostics** (`d`) and press `A` to auto-stage
   the gain, then read **NF** and **MDS** to confirm the receiver is sensitive
   enough for what you're chasing. Watch the **ADC Loading** bell fill the range
   without touching the rails.
4. In **Lab Timing (`Lab 3`)**, confirm the timing verdict is Good or Excellent, and
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
