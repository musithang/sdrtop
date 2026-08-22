# The Lab Presets

← [Back](README.md)

sdrtop's **lab presets** are the bench-engineer views: instead of just a live spectrum, they surface the measurements sdrtop can derive about your receiver's *signal quality* and *hardware health*. They're built for setting up a clean capture and watching for trouble during a long run.

The measurements are split across four focused presets, each on its own number key:

| Key | Preset | Focus |
|-----|--------|-------|
| `5` | **Lab IQ** | IQ diagnostics + constellation + spectrum |
| `6` | **Lab RF** | RF front-end bench: diagnostics + level diagram + ADC loading |
| `7` | **Lab Timing** | stream-timing diagnostics + hardware vitals |
| `8` | **Lab Signal** | spectrum + signal metrics + waterfall |
| `9` | **Lab Sweep** | frequency scanner across a band wider than one window |

This guide explains each measurement below; the heading notes which preset to open for it. Every panel turns its border and title **[STALE]** when RX is not streaming, so you always know whether you're looking at live data or a frozen snapshot.

> The lab panels also have a focus mode for extra actions — see [Keyboard Shortcuts](keys.md#lab-panel-focus-modes). The focus key is the highlighted letter in each panel's title.

---

## RF Front-End Bench  ·  *Lab RF (`6`)*

A three-panel bench that reads the whole receive chain as one story. The thesis it teaches: **level climbs stage by stage; the gap between signal and noise is the SNR set at the antenna; gain only positions that gap in the ADC window** — it never improves it. Each panel restates one face of that.

The banner across the top sums it up: `CHAIN ANT▸LNA▸MIX▸VGA▸ADC · NF 6.0 dB · MDS −105 dBm · SNR 40 dB`, and the marker bar at the bottom reads the ADC window: `CLIP 0 dBFS · PEAK −8 dBFS · Δ headroom +8 dB · NOISE −48 dBFS · SNR 40 dB`.

> **A note on the levels.** The HackRF is not power-calibrated, so the dBm figures here are *modeled / relative*: the lineup is back-computed from the *measured* ADC level through the *known* stage gains, anchored to a documented `0 dBFS = 0 dBm` reference. They're exactly right for staging decisions, and they're not a wattmeter reading. Likewise the linearity figures (below) are datasheet-anchored estimates, not lab measurements — both are labelled as such in the panel.

### RF Diagnostics *(left — focus `D`)*

The chain quantified, top to bottom:

- **Gain lineup** — the signal level after each stage (ANT, LNA, MIX, VGA, ADC), with each stage's gain in the middle column. You can watch the signal climb by each stage's gain and land at the measured ADC level.
- **Gain staging** — LNA `n / 40` and VGA `n / 62` gradient bars (the same bars as the command rail and header), each with a `┊` tick marking the *optimal* target. The `opt` line reads `✓ at optimum` or points at the LNA/VGA the staging wants.
- **Noise figure** — each stage's own NF as a bar, and the Friis **system total** beneath. The system total can sit *below* the worst single stage because the LNA's gain suppresses the noise of everything after it — that's the whole point of leading with a low-noise amplifier.
- **Sensitivity** — **MDS** (Minimum Detectable Signal, `−174 dBm/Hz + 10·log₁₀(BW) + NF`) plus a noise-floor trend sparkline with its ±dB/60s spread. Narrowing the BB filter or lowering the NF improves (lowers) the MDS.
- **Verdict** — a plain-language read of the staging (`WELL-STAGED`, `HOT`, `CLIPPING`, `UNDER-UTILISED`…) and the action chips `[A] auto-gain · [↑↓] LNA · [ ] VGA`.

### Gain-Staging Level Diagram *(centre)*

The lineup drawn as a picture: two traces climbing the stage axis ANT▸LNA▸MIX▸VGA▸ADC — **signal** (filled) and **noise floor** (line). The vertical gap between them is the SNR. Reading it left to right shows the gap being *carried up* the chain and parked inside the ADC window, never widened. Dashed reference lines mark the ADC clip ceiling and 8-bit floor; the band between the traces is shaded as **usable dynamic range** (or, if the noise ever climbs above the signal, flagged as a **buried** band instead of left blank).

### ADC Loading *(right)*

How hard the 8-bit ADC is actually driven:

- **Signed sample histogram** — a centred bell from −FS to +FS. A healthy signal fills the middle without piling up on the rails; the rails turn amber, then red, as clipping appears. A lopsided bell reveals a DC offset.
- **Headroom** bar — clip headroom in dB, with the optimal tick.
- **Loading** — `peak` / `rms` in dBFS and ADC counts, **crest** factor, **effective bits** (ENOB), and the **clip-event** count for the window.
- **Linearity** *(modeled)* — P1dB headroom, IIP3 / IMD3, and SFDR against the honest 8-bit ceiling (`6.02·8 + 1.76 ≈ 50 dB`). These need a two-tone source to measure for real; here they're gain-adjusted datasheet estimates for guidance.

### Auto-gain and freeze

Focus the RF Diagnostics panel with `D`, then:

- **`A` — auto-gain.** When the chain is off-optimal, one press jumps LNA/VGA to the staging target (signal ≈ −8 dBFS, no clip), filling LNA first to protect the noise figure. Once you're already at the optimum, pressing `A` again **latches a continuous auto-track** that re-nudges the gain when the level drifts (the chip lights `✓`); press once more to unlatch. Touching the gain manually (`↑↓`, `[ ]`, `a`, `r`) drops the latch immediately, so it never fights you.
- **`⎵` / `F` — freeze.** Holds the histogram and level diagram on a snapshot so you can study them while RX keeps running; both panels show `[FRZ]` in their title. Press again to go live.

---

## IQ Amplitude Distribution  ·  *optional panel (`iq_histogram`)*

> In the default **Lab IQ** preset the constellation (below) now fills this slot: the same ADC data shown as a richer 2-D cloud. The histogram is still available as a panel if you want the exact Low/Mid/Clip percentages: add `iq_histogram` to a [custom layout](config.md#custom-layout-presets).

A histogram of incoming sample amplitudes across 32 bins, log-scaled vertically so both rare strong peaks and the bulk of weak samples are visible at once. Colour zones:

- **Dim (left)** — low amplitude. The ADC is under-utilised.
- **Green (centre)** — the healthy range.
- **Red (right)** — high amplitude, approaching clipping.

**Numeric breakdown** — the exact percentages so you can set gain precisely:

```
Low  12%   Mid  71%   Clip  17%
```

**PAPR** — **Peak-to-Average Power Ratio** (crest factor) in dB, estimated from the distribution. This is a quick fingerprint of *what kind* of signal you're looking at:

| PAPR | Likely signal |
|------|---------------|
| under 3 dB | CW / FM (constant envelope) |
| 3–8 dB | AM / mixed |
| 8–15 dB | wideband / spread-spectrum |
| over 15 dB | bursty / impulsive |

A status line at the bottom summarises the picture: "Dynamic range OK", "weak signal — ADC under-utilised", or "clipping risk".

---

## IQ Diagnostics  ·  *Lab IQ (`5`)*

The quality of the I/Q signal coming off the ADC. Problems here show up as artefacts in the spectrum. Each *deviation-from-ideal* is drawn as an analog **null-meter**: a centre tick is "perfect", and a coloured needle deflects left/right by how far off you are, with the span between centre and needle filled. A glance reads the state; the number beside it reads the exact value.

- **DC I / DC Q** - how far each channel is offset from zero (a null-meter each), with a combined **DC magnitude** quality bar. A high DC offset puts a fixed tone right in the middle of your spectrum.
- **DC spike** - how tall that centre-frequency spike is, in dBFS. Green below −40 dBFS.
- **Amp imbalance** - whether I and Q carry the same power (null-meter). A mismatch creates mirror images of signals on the opposite side of centre.
- **Phase imbalance** - whether I and Q are exactly 90° apart (null-meter). Also causes mirroring.
- **IRR** - **Image Rejection Ratio** in dB, as a red→green quality bar. This is the key quadrature-quality figure: it tells you how far *below* every real signal its mirror image appears. 30 dB or more is good (images are faint); below 20 dB and the images become a problem.

A contextual hint at the bottom summarises whether anything needs attention, colour-matched to severity.

---

## IQ Constellation  ·  *Lab IQ (`5`)*

The 2-D picture of the same I/Q stream, in the centre of the Lab IQ preset. Where the diagnostics give you the numbers, the constellation gives you the *shape*, and shape is often faster to read.

It plots recent I/Q sample pairs as a dot-cloud over a fixed reference frame (the unit circle, a faint ±0.5 ring, and I/Q axes). What to look for:

- A **circle** centred on the origin means healthy quadrature.
- An **ellipse** means amplitude imbalance (I and Q at different levels).
- A **tilt** means phase imbalance (I and Q not 90° apart).
- The cloud's **offset** from centre is the DC offset (a small crosshair marks the measured DC point).

The cloud is coloured by **point density**: a phosphor-scope look where sparse edges are a cool blue and the dense core glows orange, so you can see where the signal's energy actually concentrates. A measured **imbalance ellipse** is fitted over it: its axis ratio is the amplitude imbalance, its tilt the phase imbalance, the same two faults the diagnostics quantify, drawn straight onto the cloud. No live numbers sit here on purpose; they're one panel to the left. Yes, it looks like an old analog scope. That's the point.

---

## Hardware Vitals  ·  *Lab Timing (`7`)*

Whether the capture chain is keeping up, with a trend sparkline under each metric.

- **Drops** — samples lost per second, plus the session total. Non-zero means USB or CPU can't keep up.
- **ADC saturation** — how often samples hit the ADC ceiling, with the session peak.
- **CPU / RAM** — sdrtop's own processor and memory use. CPU is a system-wide percentage (100% means every core is maxed), so on a multi-core machine a healthy figure is well under 100%. If CPU climbs toward the warn/crit thresholds at high sample rates, that's often the cause of drops.
- **USB errors** — zero-length USB transfers, usually a cable or hub problem. Coloured by recent rate, not session total, so a single old glitch doesn't pin it red forever.
- **SR** — configured versus actually-measured sample rate, e.g. `20.000 → 19.847 MHz (−0.8%)`. A large gap means USB can't sustain the requested rate. Shows `→ ---` when not streaming.
- **BUF fill** — receive-buffer fill percentage with history. A leading indicator: if this trends upward toward 100%, drops are about to start.

---

## Signal Characterization  ·  *Lab Signal (`8`)*

The left-hand column of **Lab Signal**. Where the demod panel opposite opens the
channel up and reads what is inside it, this one answers the question you ask
first: what *is* that, and how clean is it? Everything here comes from the same
FFT frame the spectrum beside it draws, so the two always agree. Press `x` to
focus it, then `C` to write the current readings to the log.

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

## FM MPX · Demod  ·  *Lab Signal (`8`)*

The right-hand column of **Lab Signal** actually demodulates the channel and reads
out what is inside it. It is a **measurement instrument, not a receiver**. There is
no audio anywhere in sdrtop, and this panel exists to tell you things about a
transmission that a spectrum plot cannot: how hard it is deviating, whether it is
in stereo, which subaudible tone opens the squelch, what the station calls itself.

Press `m` to focus it. It only runs while the panel is actually on screen, so it
costs nothing on any other layout. That means it also works in a
[custom preset](config.md#custom-layout-presets) of your own, as long as you list
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

- **MPX baseband**: the demodulated composite from 0–60 kHz as a braille profile,
  with ticks at 19 k (pilot), 38 k (stereo difference) and 57 k (RDS). This is the
  audio-side spectrum, not the RF one. You are looking *inside* the FM channel.
- **Pilot / stereo**: `● STEREO`, `◐ MARGINAL` or `○ MONO`, plus the pilot's own
  deviation and its **injection percentage**. Broadcast practice is 8–10 %, so a
  figure well outside that is a transmitter fault rather than a reception problem.
- **Deviation**: peak (quasi-peak, with decay) and RMS deviation measured *about
  the carrier*, plus a **Carrier** row giving how far the carrier sits from the
  centre of the demodulated channel. Measuring about the carrier is what makes a
  mistuned radio report its tuning error there instead of inflating the modulation
  figure. (That row used to be called "Offset", which collided with the channel
  offset in the headline above it. The two point in opposite directions, so they no
  longer share a name.) The bar is drawn against the mode's nominal limit, ±75 kHz for
  broadcast and ±5 kHz for NFM, and turns amber then red as you approach and exceed
  it.
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

- the **Programme Service** name (`● DANKO`), the 8-character station name,
- **PI**, the station's unique hex identity code,
- **PTY**, the programme type (`Pop Music`, `News`, `Culture`, and so on),
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
deviation figure. Either enable the DC block (`[D]` in Lab IQ) or walk the channel
off centre with `←` / `→`.

---

## Sweep  ·  *Lab Sweep (`9`)*

The HackRF sees only as much spectrum at once as the sample rate covers (±10 MHz
at 20 Msps). **Lab Sweep** maps a wider band by retuning across it: at each step
it measures briefly, records the peak and mean level, then moves on, stitching
the results into one curve with frequency on the x-axis. Known bands are labelled
from the band plan, and the cursor reads out the level and band at any point.

Because a full cycle takes a couple of seconds, sweep is for *finding* a signal,
not watching it — once you spot one, focus the panel with `g` and press `Enter`
to tune straight to the cursor frequency in normal RX. While focused, `s` / `e`
set the start / end frequency live and `+` / `-` adjust the dwell; the band and
dwell also live in the config (see [Configuration → Sweep scanner](config.md#sweep-scanner)). The
`micro_sweep` step in the `0` cycle gives the same scan as a compact field list.

---

## Using the lab presets in practice

A typical setup flow, switching presets as you go:

1. Tune to your target and start RX (`Space`).
2. In **Lab IQ (`5`)**, watch the **constellation**: adjust LNA/VGA (`↑`/`↓`, `[`/`]`) until the cloud is a bright, well-filled ring sitting comfortably *inside* the unit circle (smearing out to the edge means clipping). Glance at **IQ Diagnostics**: a centred needle on each null-meter, IRR above 30 dB and DC spike below −40 dBFS mean clean quadrature.
3. In **Lab RF (`6`)**, focus the **RF Diagnostics** panel (`D`) and press `A` to auto-stage the gain, then read **NF** and **MDS** to confirm the receiver is sensitive enough for what you're chasing. Watch the **ADC Loading** bell fill the range without touching the rails.
4. In **Lab Timing (`7`)**, confirm the timing verdict is Good/Excellent before committing to a long run.
5. During a long capture, keep an eye on **Hardware Vitals** (in **Lab Timing `7`**) — CPU, BUF fill, and Drops together tell you whether the run is sustainable.

---

← [Back to all screens](screens.md)
