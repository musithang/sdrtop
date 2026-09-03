# How the Demodulator Works

← [Back](README.md)

The `fm_demod` panel opens a channel up and reports what is inside it. This page
is about how it arrives at those numbers.

[The Lab presets](lab.md#fm-mpx--demod--lab-signal-lab-4) covers the other half:
what each reading means and what to do about it. If you want to *use* the demod,
start there. This page is for anyone who wants to know what is actually happening
between the antenna and the number.

It comes in two sizes. **Part one** is the whole thing in plain language, and it
is short. **[Part two](#part-two-with-the-arithmetic-left-in)** is the same story
with the arithmetic left in, for anyone who is thinking about writing one of
these.

---

# Part one: the short version

**A radio does not send sound. It sends numbers.**

Millions of them a second, each describing where the incoming wave was at that
instant: how strong, and at what angle. What comes down the USB cable is not
music. It is a very long list of coordinates, and sdrtop keeps it that way.

The demodulator's job is to work out what the transmitter was doing and say so in
numbers you can write down. Here is the whole of it.

**1. Pick one station out of the crowd.** The radio hears a wide slice of the
band at once. The demod slides sideways to the station you point it at and
filters the rest away. It pointedly avoids the exact dead centre of that slice,
because the dead centre is where the radio's own electrical noise sits, quietly,
doing an impression of a station.

**2. Watch the wobble.** FM carries information by wobbling its frequency up and
down, so compare each sample's angle to the one before it. That difference *is*
the wobble, in Hz.

And that is the whole of FM demodulation. One subtraction, a few hundred thousand
times a second. There is an enormous amount of theory explaining why it works and
none of it is required in order to use it, which feels a little like getting away
with something.

**3. Then look at the wobble itself.** Treat it as a signal in its own right and
take its spectrum. For a broadcast station it opens like a drawer:

- a steady tone at 19 kHz, the **pilot**, whose entire job in life is to sit
  there and mean "this station is in stereo"
- the stereo difference signal at 38 kHz
- **RDS** at 57 kHz, the small data channel carrying the station name

**4. Read the data channel.** RDS sits at exactly three times the pilot, and its
bit clock is tied to the pilot as well. So lock onto the pilot, which is loud and
clean and thoroughly cooperative, and the data channel's position *and* its
metronome both turn up free of charge. Read the bits, check them, assemble them
into a station name, a programme type and scrolling text.

Whoever specified RDS was plainly thinking about the poor soul who would one day
have to decode it. This cannot be said of every standard.

**5. On a walkie-talkie channel instead**, listen underneath the voice for a
quiet tone that radios use to decide whose calls to let through. There are forty
of them, and the two closest are 2.3 Hz apart, which is the sort of number that
comes out of a committee. Separating those two takes half a second of
uninterrupted signal and cannot be hurried by writing better code.

**6. On AM instead**, measure how much the signal's loudness swings and how
evenly it swings each way. After the other five, AM is almost restful.

That is it. Everything on the demod panel falls out of those steps.

**One thing worth adding, because it is rather the point.** Apart from the FFT,
every piece of arithmetic above is written out in this project's own Rust rather
than lifted from a signal processing library. Not for sport. This is a measuring
instrument, and when one of its readings argues with your bench equipment there
has to be a real answer available, rather than a shrug and a link to somebody
else's issue tracker.

If that was the depth you were after, you are done, and the panel itself is
[documented here](lab.md#fm-mpx--demod--lab-signal-lab-4).

---

# Part two: with the arithmetic left in

## The honest ledger

Two crates do arithmetic here, and it is worth naming them before claiming
anything.

- **`rustfft`** performs the forward FFT. It is used in the spectrum analyser and
  for the recovered MPX baseband. Writing a competitive FFT is a specialist
  discipline and there is no honour in doing it badly.
- **`num-complex`** supplies the complex number type. It gives us multiply,
  conjugate and magnitude. That is a type, not a signal processing library.

Everything else in the chain below is written out in Rust: the filter design, the
decimator, the discriminator, the oscillator, the phase-locked loop, the
resonator, the single-bin DFT, the symbol recovery, the CRC and the block
synchroniser. There is no GNU Radio, no liquid-dsp, no SDR framework underneath
any of it.

That is a statement of fact rather than a boast. It is also the reason this page
exists: if the arithmetic is ours, we are obliged to be able to explain it, and a
measurement nobody can explain is a measurement nobody can defend.

---

## The chain

### 1. Wire bytes to complex samples

The radio hands over interleaved integers, and how to read them depends entirely
on the device: signed 8-bit on a HackRF, unsigned 8-bit with a bias on an
RTL-SDR, 16-bit through SoapySDR with only some of the bits meaning anything. The
sample geometry travels with the device rather than being assumed here.

Nothing is windowed at this stage. This is a time-domain path, not an FFT input,
and windowing it would be a mistake with no error message attached.

### 2. Move the channel off the centre

An oscillator mixes the stream down so the channel of interest lands at DC.

This exists because of a specific and slightly annoying fact: the tuned centre is
exactly where both front ends park their DC offset and their local oscillator
leakage. Demodulating whatever sits at the middle of your span means competing
with the receiver's own artefact for the whole measurement. So the demod tunes
*within* the captured spectrum, and `←` / `→` on the panel move that channel.

The phasor advances by repeated complex multiplication rather than calling `sin`
and `cos` per sample, and gets renormalised periodically so accumulated rounding
cannot walk it off the unit circle over a long block.

### 3. The channel filter

A Hamming-windowed sinc low-pass, decimating as it filters, computing only every
`d`-th output so the cost scales with the channel rate rather than the device
rate.

The interesting part is not the filter. It is choosing `d`, and this is where the
first real mistake lives.

Wide FM targets a **320 kHz** channel, which sounds generous until you do
Carson's rule properly. A broadcast signal at ±75 kHz deviation carrying 53 kHz of
MPX occupies roughly 2 × (75 + 53) ≈ 256 kHz, so the channel has to pass about
±128 kHz. Filter an FM carrier more narrowly than its Carson bandwidth and the
envelope collapses on large excursions, which the discriminator faithfully
reports as clicks pinned at its ambiguity limit. The filter is not lying. You are
asking it a question it cannot answer.

The decimation factor is rounded **down**, never to nearest, and that asymmetry
is deliberate. A channel slightly wider than asked for costs a few extra samples.
A channel slightly narrower clips the signal. Rounding to nearest gets this wrong
at 2.4 Msps, where ÷8 lands on a 300 kHz channel whose passband misses ±128 kHz,
and the result is a panel full of clicks and a very confusing evening.

Tap count comes from the transition width of a Hamming-windowed sinc, roughly
`3.3 / taps`, which works out at about `16.5 × d` taps to hold the transition to a
fifth of the channel bandwidth. It is clamped between 31 and 511. Below 31 the
filter cannot reject a neighbouring station; above 511 the cost stops buying
quality.

**And at very high sample rates that cap means the filter genuinely does get
softer.** The panel says so and advises dropping the sample rate. It does not
quietly hand back a worse measurement wearing the same label.

### 4. The discriminator

Here is the entire core of FM demodulation:

```
f[n] = arg( z[n+1] · conj(z[n]) ) · rate / 2π
```

The phase advance between consecutive samples, scaled by the sample rate, is the
instantaneous frequency in Hz. That is it. That is the thing. Discovering that
after a week of reading was mildly insulting.

It is unambiguous to ±rate/2, so at a 333 kHz channel rate that is ±166 kHz,
comfortably clear of the 75 kHz that broadcast FM is allowed. It always produces
one fewer output than input, and that missing first sample is exactly the splice
guard between blocks, because the previous block's final phase is not usable.

### 5. The envelope gate, and the number that was wrong for a while

An FM carrier has a constant envelope, so on a clean signal every sample is
usable. The envelope only collapses where the phase means nothing anyway: noise
nulls, and the beat nulls of a second carrier inside the channel.

Those samples produce a full 2π phase step, which the discriminator dutifully
reports as an excursion pinned at the ambiguity rail. Ungated, a handful of them
dominate the peak reading. A strong broadcast station read about **125 kHz peak
against a 16 kHz RMS**, a crest factor no real transmitter has ever produced, and
the reading rose when you widened the channel, which is the sort of behaviour
that should have been suspicious a good deal earlier than it was.

Two fixes, and the second is the one worth stealing:

- Samples below the gate are **replaced with the previous trustworthy value, not
  removed.** Dropping them would leave a non-uniform time base, and the MPX
  spectrum cannot work from that: a gap shifts every later sample in time and
  smears the 19 kHz pilot. Holding keeps the sample grid intact.
- The peak reading is the **99.9th percentile, not the maximum.** A quasi-peak
  detector, the way a bench instrument does it. For a sine wave the 99.9th
  percentile sits within 0.001 % of the true peak, so nothing real is lost, and
  one corrupted sample pair no longer gets to define the measurement.

### 6. Three branches

The classifier picks one, or you force it with `T`:

| Mode | Path |
|---|---|
| **WFM** | discriminator → MPX baseband spectrum → pilot, stereo, RDS |
| **NFM** | discriminator → CTCSS tone detection |
| **AM** | envelope → depth and asymmetry |

---

## Wide FM: the MPX baseband

The discriminator's output is already instantaneous deviation in Hz, which turns
out to be a small gift. Run a 2048-point FFT over it and each bin's amplitude is
the deviation contributed by that MPX component, which is *exactly* how pilot
injection is specified in the first place. No conversion, no fudge factor.

At a ~333 kHz channel rate that is ~163 Hz per bin, enough to isolate the 19 kHz
pilot, and the display runs to 60 kHz so the 38 kHz stereo difference signal and
the 57 kHz RDS subcarrier both sit inside it.

The pilot is measured by taking the strongest bin in a small neighbourhood of
19 kHz rather than one exact bin, because the pilot rarely lands on a bin centre
and window leakage spreads it across its neighbours. Broadcast practice injects
it at 8 to 10 % of the deviation limit. Anything at or above 4 % is reported as
locked, so a weakly injected but genuine pilot still reads as stereo. Between
1.5 % and 4 % it is reported as **marginal**, which is its own state rather than
a rounding decision, because "there might be a pilot here" is a true thing that
deserves saying out loud.

---

## RDS, in two halves

RDS lives in two files on purpose, and the split is the most useful structural
decision in the whole demod.

### The signal half

Recovering a bitstream from the 57 kHz subcarrier:

1. **Lock a PLL to the 19 kHz pilot.** Not to the RDS subcarrier. The subcarrier
   is exactly three times the pilot, and the 1187.5 bps symbol clock is that
   subcarrier divided by 48, so tracking the pilot pins the carrier *and* the
   symbol rate at once. The pilot is also the strongest, purest line in the
   baseband, which makes it enormously easier to lock than a suppressed-carrier
   signal 20 dB down. A second-order resonator isolates it first so programme
   audio cannot pull the loop.
2. **Mix by three times the pilot phase** to bring the subcarrier to DC, then
   low-pass hard. The stereo difference signal ends up only about 4 kHz away once
   shifted, which is why that filter is 511 taps.
3. **Find the constellation axis.** RDS is suppressed-carrier BPSK, so the
   recovered points sit on an unknown axis. A squaring estimator finds it. The
   leftover 180° ambiguity needs no resolving at all, because the data is
   differentially encoded and inverting every symbol leaves the bits unchanged.
   This is a nice piece of design by whoever specified RDS, and it is free.
4. **Biphase symbol recovery.** Each bit is Manchester coded, so integrating the
   first half-symbol minus the second recovers it, and the guaranteed mid-symbol
   transition gives the timing loop something unambiguous to lock onto. An
   early-late gate walks the phase into place and then mostly holds still,
   because the pilot has already fixed the rate.

### The protocol half

Everything above produces bits. The protocol layer never touches a waveform,
which means **the entire RDS protocol is testable without a radio.**

RDS sends 26-bit blocks, 16 information bits and a 10-bit checkword, in groups of
four. The checkword is a CRC XOR'd with a per-position offset word, and that is
the clever part: compute the syndrome of any candidate 26-bit window, and if you
get one of the five offset words back, you have found a block boundary *and* you
know which position you are looking at. The blocks locate themselves. There is no
preamble to hunt for.

Three behaviours worth knowing, because each one was a bug first:

- **A dropped block no longer throws away the accumulated text.** Only block
  synchronisation restarts. What was already decoded stays.
- **A station name does not outlive its station.** Retune and everything the
  decoder holds is dropped, including the identity, because none of it describes
  the new frequency. Lose the subcarrier without retuning and the name ages
  visibly first, then goes.
- **Accents work.** RDS is not ASCII, it has its own character set, and every
  accented letter used to arrive as a blank. A Hungarian title now reads as a
  Hungarian title, which mattered rather a lot to the person writing this.

Nothing is displayed until it has been confirmed twice. RDS carries only a block
CRC, so a single corrupt block that happens to pass is entirely possible, and one
wrong character in a station name is more annoying than a second's delay.

---

## Narrow FM: CTCSS, and the rewrite it forced

CTCSS is a sub-audible tone under a voice channel. There are 40 of them in the
table, and at the bottom end they are about **2.3 Hz apart** (67.0 and 69.3).

Detection uses a **Goertzel** rather than an FFT. It is a single-bin DFT, far
cheaper when only forty frequencies matter, and it is not constrained by bin
spacing, which matters because none of the CTCSS tones land on FFT bin centres. A
tone is reported only when it is both strong enough absolutely and at least 6 dB
clear of the best non-adjacent rival, because adjacent table entries sit inside
each other's skirts and a tone that merely edges out its neighbour is an
unresolved measurement rather than a detection.

Now the awkward part.

Telling two tones 2.3 Hz apart from each other takes roughly `1 / Δf` seconds of
observation, so about 430 ms. Half a second, with margin. **This is arithmetic,
not effort.** No amount of clever code shortens it.

Which broke the design, because the rest of the demod is deliberately
duty-cycled: it runs every 250 ms on at most 65,536 sample pairs, and blocks
arriving in between are discarded. That is correct load shedding for a display
feed and deviation statistics are perfectly happy with 33 ms snippets.

CTCSS needs half a second of *unbroken* audio. The original stateless decimator
restarted at every block, discarding the first `taps` samples and resetting the
decimation grid, which put a small timing step at every block boundary. Deviation
statistics never noticed. A narrowband tone detector noticed immediately, because
a phase step inside the observation window destroys exactly the coherence the
detection depends on.

So the channel filter became a **streaming** decimator that carries its filter
history and its decimation phase across blocks, the demod feed carries a sequence
number so a gap can be detected, and a detected gap resets the run rather than
silently corrupting it. Narrowband FM also pays for every block instead of
duty-cycling, because continuity outranks the CPU budget there.

**And when blocks are dropped, the panel counts them and says so**, because a busy
machine and a station with no CTCSS look identical otherwise, and letting you
blame the wrong one would be the worst outcome available.

---

## AM: depth without drama

The envelope is just the magnitude of each complex sample. Depth is the classic
`(Vmax − Vmin) / (Vmax + Vmin)`.

Two small decisions:

- The peaks are **quantiles, not extremes**, for the same reason the FM peak is.
  One impulse should not define a reading.
- Positive and negative depth are reported **separately**, because they fail
  differently. A negative depth reaching 100 % means the carrier is being pinched
  off, which clips and splatters. An asymmetric pair points at a modulator fault
  rather than simply too much level. Averaging those two into one number would
  throw away the diagnosis and keep the symptom.

---

## The two constraints that shaped everything

Neither of these is about DSP, and both of them changed more code than any
filter decision.

**The block stream is lossy on purpose.** Blocks are forwarded with `try_send` on
a bounded channel, so under load they are dropped rather than queued. For a
display feed that is correct. For CTCSS and RDS it is a hazard, which is why both
carry continuity state and both can tell you when the run was broken.

**CPU is a displayed metric.** sdrtop shows its own CPU usage on the vitals
panel, which makes it very difficult to be casual about cost. Work is bounded
twice: a ceiling on samples per update, and a fixed update interval regardless of
how fast blocks arrive. The consequence is that demod cost is roughly independent
of the device sample rate, because the decimating FIR computes only every `d`-th
output and scales with the *channel* rate.

---

## Why write it out

Not purity, and not because the libraries are bad. They are excellent, and for
most projects using one is obviously the right call.

Three reasons, in order of how much they actually weighed:

1. **A measurement you cannot explain is a measurement you cannot defend.** This
   is a bench instrument. When somebody's reading disagrees with their signal
   generator, the answer has to be a paragraph about what the code does, not a
   shrug and a link to somebody else's issue tracker.
2. **The numbers had to mean something specific.** Deviation measured about the
   carrier rather than about the tuned centre. Pilot injection as a percentage of
   the mode's deviation limit. A quasi-peak rather than a maximum. These are
   instrument conventions, not defaults, and bending a general-purpose block into
   each of them is often more work than the twenty lines it replaces.
3. **This one is the foundation for the next ones.** Everything above is a
   primitive with a test suite: windowed-sinc design, streaming decimation, an
   NCO, a polar discriminator, a PLL, a Goertzel, symbol timing recovery, a CRC
   syndrome search. There are 76 tests across the demod stack and none of them
   need a radio.

---

## What this becomes

WiFi, Bluetooth, ADS-B, AIS, LoRa and DMR are on the roadmap, each with its own
panel and its own detailed readout. They will be built on the primitives above,
from scratch, on the same terms, and they will not ship as packet dumps.

The rule they are held to is in [POLICY.md](../POLICY.md), and it is short: a
payload is not a measurement. Printing `51 bytes` is a courier's job. The question
is *why 51*, at what spreading factor, across what bandwidth, how far above the
noise, how much of the margin the preamble spent, and whether the decoder was
confident or merely lucky.

Still no audio. It reads radios, it does not play them.

---

## Where to go next

- **[The Lab presets](lab.md#fm-mpx--demod--lab-signal-lab-4)**: what every
  reading on the demod panel means, and how to act on it
- **[Tips and Tricks](tips-and-tricks.md)**: getting a signal clean enough for
  RDS to decode in the first place
- **[What's new](whats-new.md)**: the checkpoint where the demodulator landed,
  and the one where it was taken apart and checked

← [Back](README.md)
