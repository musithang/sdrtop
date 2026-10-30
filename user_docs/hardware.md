# Supported Hardware

← [Back](README.md)

---

## What works today

| Device | Status |
|--------|--------|
| HackRF One | Fully supported: spectrum, waterfall, every diagnostic |
| RTL-SDR (R820T / R828D / E4000) | Fully supported: the whole spectrum, waterfall and lab stack, with a single tuner gain plus AGC |
| Airspy R2 | Fully supported natively: spectrum, waterfall, every diagnostic, with measured gain tables and Airspy's own gain curves. See [below](#airspy-r2) |
| Airspy HF+ Discovery | Fully supported natively: both bands, every rate, every diagnostic, with its digital gain drawn as digital. See [below](#airspy-hf-discovery) |
| tinySA / tinySA Ultra / Ultra+ | Spectrum, waterfall and native band sweeps in calibrated dBm. ZS405 is verified on hardware |
| **Anything with a SoapySDR driver** | Supported, **and not yet confirmed on hardware other than a HackRF**. See [below](#soapysdr-the-honest-version) |
| PortaPack H4M (Mayhem) | Fully supported (HackRF mode) |

HackRF One, RTL-SDR, the Airspy R2, the Airspy HF+ Discovery and the tinySA Ultra
ZS405 are tested on real hardware.

The third row is a deliberate exception, and it gets its own section rather than
a footnote, because you deserve to know which kind of "supported" you are
looking at.

### What differs between them

The UI adapts rather than showing you fields that can't mean anything:

| | HackRF One | RTL-SDR | Airspy R2 | Airspy HF+ Discovery |
|---|---|---|---|---|
| Frequency range | 1 MHz to 6 GHz | By tuner: 24 MHz to 1.766 GHz on R820T / R828D, 52 MHz to 2.2 GHz on E4000 | 24 MHz to 1.8 GHz | 9 kHz to 31 MHz and 60 to 260 MHz |
| Sample rate | 2 to 20 Msps | 0.9 to 3.2 Msps | 2.5 or 10 Msps | Seven, 192 to 912 kS/s |
| Gain stages | LNA (0–40, step 8) and VGA (0–62, step 2) | One tuner gain, stepping a table the device reports | LNA, mixer and VGA, each a measured table; the knob walks Airspy's curves | ATT (0 to 48 dB, step 6) and LNA (0 or 6), both digital, after the converter |
| Front-end boost | RF amplifier (`a`) | Tuner AGC (`a`) | Tuner AGC on the LNA and mixer (`a`) | The radio's AGC setting, which hands it the ATT (`a`) |
| Baseband filter | Tunable, follows the sample rate | None, so the panel reads N/A | None, so the panel reads N/A | None, so the panel reads N/A |
| Friis noise figure | Computed per stage | N/A: the dongle doesn't expose per-stage gains | N/A: nobody publishes each stage's noise figure | N/A: its stages are digital |
| Clip ceiling | The converter's 0 dBFS | The converter's 0 dBFS | The tuner's, about −3 dBFS | None: the radio holds peaks at about −12 dBFS |
| Sample format | Signed 8-bit | Unsigned 8-bit, biased at 127.5 | 12 bits, delivered as 16-bit I/Q | 16-bit USB word, carried in 32 bits with 8 below it |

Everything else, including every lab bench, the demodulator and the sweep scanner,
works the same on all four, apart from what the
[HF+ section](#airspy-hf-discovery) spells out.

## tinySA

sdrtop uses the tinySA USB console for calibrated power traces. The backend
supports the basic tinySA and the Ultra family. It was verified on a ZS405
running `tinySA4_v1.4-236-ge5aa115`.

The device supplies swept power readings without IQ samples. sdrtop offers the
spectrum, waterfall, full band sweep and micro sweep layouts. The Options pane
controls scan points, RBW, attenuation, spur removal and external gain. Basic
LOW and Ultra offer `Attenuation (dB)` with `auto` and 0–31 dB choices. Basic
HIGH offers `Coarse attenuation` with `off` and `on` choices. `on` asks the
firmware for frequency-dependent attenuation of roughly 25–40 dB. Ultra models
also expose the external front-end LNA. Scan points are a host-side setting.
The other controls use the matching tinySA console commands.

sdrtop disables output and aborts pending work during startup. It then forces
input mode and applies a safe automatic baseline before restoring `[tinysa]`
settings. Basic HIGH uses a truthful coarse attenuation baseline of `off`.
Recovery restores the active connector's attenuation setting. LOW numeric
attenuation and HIGH coarse attenuation are saved independently.

```sh
sdrtop --device tinysa
sdrtop --device tinysa=/dev/ttyACM2
sdrtop --device 'tinysa=/dev/ttyACM2?input=high'
```

Automatic discovery recognizes the official USB CDC identity: on Linux among
`/dev/ttyACM*`, on a Mac among `/dev/cu.usbmodem*` (there the path looks like
`tinysa=/dev/cu.usbmodem4001`). The explicit form selects a path when several
devices are present or discovery can't see the USB identity. A Basic tinySA uses the persisted
`[tinysa].basic_input` value. The default is LOW from 100 kHz to 350 MHz. HIGH
covers 240 MHz to 959 MHz. The supported firmware defines 959 MHz as the HIGH
limit. `?input=low` or `?input=high` overrides the config at startup. A bare
`--device tinysa` or `tinysa=PATH` uses the config value.

Connector selection happens only during startup. Restart sdrtop to use an edited
config value. One Basic session stays on one connector. Quitting after a Basic
session saves the connector that session used. This includes a CLI override.

Ultra firmware always uses automatic input selection. The Basic connector value
does not affect an Ultra open and remains saved for the next Basic session. An
explicit `?input=low` or `?input=high` is ignored on Ultra. The startup log says
that it was ignored.

> **RTL clones vary.** Different tuners, different gain tables, different quirks,
> and no single person owns them all. If yours behaves oddly, please
> [open an issue](../../../issues) with the tuner type (`rtl_test -t` prints it)
> and what you saw. That's how "works on the units we've tried" becomes "works,
> full stop". The [troubleshooting page](troubleshooting.md) covers the common
> RTL-specific traps, starting with the kernel's DVB driver claiming the dongle
> before sdrtop can.

---

## Airspy R2

Airspy sent one, which is the only reason this section exists. It also worked
through SoapySDR from day one, so the obvious question is what a native backend
was for. Mostly the gain. SoapySDR has no unit for a gain except decibels, so the
R2's register steps arrived as "dB" that were not dB, every gain figure on screen
was quietly wrong, and the first stage looked like an on/off switch. sdrtop now
drives the R2 through libairspy itself, and was rehearsed on this one: firmware
`v1.0.0-rc10` (2016) with libairspy 1.0.10. A newer firmware may well behave
differently, and if yours does, I would like to hear how.

On the timing bench, at the same settings, the native path ran with a jitter
under a millisecond where the SoapySDR path showed eleven, and lost nothing
where SoapySDR lost samples in its first twenty seconds, at about a third of
the CPU.

### The gain is measured, not read off a box

libairspy takes register positions (0 to 14 for the LNA, 0 to 15 for the mixer
and the VGA) and says nothing about decibels. So each position was measured on
the bench against steady carriers, and the tables are what the screen uses. The
mixer and VGA tables hold across the range. The LNA's do not: it gives about
36 dB in total at 93 MHz and about 31 at 930 MHz, so its table is trusted from
88 to 170 MHz only. Outside that band the LNA reads as its step (`6/14`), and
anything that would add it to the others (the chain total, auto-gain) says it
can't, rather than adding a position to a number of decibels. In the rehearsal
every step, of every stage and both curves, came out within 0.8 dB of the
tables. (The mixer's top three steps add nothing measurable. They are shown
anyway, because they are settings the radio has, and pretending otherwise
would be tidier and less true.)

### The knob is a curve

`↑` and `↓` don't move one stage. They walk libairspy's own **Linearity** curve,
22 steps that set all three stages the way Airspy's own software does, so the
header reads something like `LIN 12/21 ▸ LNA 9 · MIX 0 · VGA 10   60 dB`. It
starts on Linearity 10. Linearity keeps the LNA low until the middle of the
curve, which makes it hard to overdrive near strong stations; **Sensitivity**
raises the LNA first, which is what you want for weak signals and what you
regret next to an FM transmitter. `,` and `.` go round a ring: Linearity,
Sensitivity, then the LNA, mixer and VGA one at a time, by hand. `[` and `]` stay
on the mixer. Whatever you pick is saved as `gain_mode`, and auto-gain (`A` on
the RF bench) walks the same curve rather than inventing its own split.

### The ceiling is the tuner, at about −3 dBFS

This one took an afternoon. Push the gain up on a strong signal and the level
stops rising at about −3 dBFS while the noise floor jumps, and the ADC bench says
`clip none`. The converter's raw samples settle it: they never reach their rails,
because the tuner's IF output has already squared the signal off, at about
70 % of the converter's range. The clip counter is right about the converter and
wrong about your signal. So on an R2, sdrtop measures headroom from **−3 dBFS**,
draws the rail there (`CLIP -3 dBFS (tuner)`), calls a peak at it clipping, and
says under `clip none` that the tuner clips there, uncounted. Auto-gain lands
about 8 dB under it, near −11 dBFS. From a clipped peak it may take two or
three presses, because a clipped reading only knows "at least this much over".

### The AGC, and what it costs

`a` switches the R820T's own AGC on the LNA and the mixer (the VGA has none).
While it's on, those two read `AGC` wherever a number would be, the chain total is
a dash, and auto-gain politely declines: the tuner is setting them, and sdrtop
doesn't know to what. Touch a gain by hand and the AGC switches off first, which
is also what libairspy's own presets do. One warning from the bench: the tuner's
AGC levels its own IF, not the converter, and on a strong FM station it parked the
signal right at the −3 dBFS ceiling. The curves are the everyday control; the AGC
is a choice, and that is its price.

### Options: packing and the bias tee

Both live in the menu's **Options** pane.

- **12-bit packing** sends 3 bytes per sample pair over USB instead of 4 (at
  10 Msps, 28.6 MB/s instead of 38.1), and the host pays for it in unpacking. It
  is saved under `[airspy]`, and switching it while streaming restarts the stream,
  because libairspy won't change it on a running one.
- **The bias tee** puts 4.5 V on the antenna port. It starts **off** every time,
  whatever it was last session, and it is never saved, because 4.5 V into an
  antenna port you rewired yesterday is how an LNA dies. Switching it is logged.
  Know what's on the end of the cable before you turn it on.

### Timing, and the rare short loss

libairspy delivers on its own thread, and sdrtop hands each block on and returns.
At 10 Msps on the deliberately old i3 this runs at about 40 % CPU with jitter
under a millisecond. Once in a long while the whole machine pauses for a few
hundred milliseconds, and a burst of samples is lost; sdrtop counts it, marks the
gap, and the timing bench says whose it was: `Lost: driver N · sdrtop M`, the
driver's own queue or sdrtop's.

One libairspy 1.0.10 quirk worth knowing: if the R2 drops off the USB bus while
streaming (seen once, cause unknown), the library cannot close that handle
safely. sdrtop keeps it rather than crash on the way out, and the radio comes back
as a new device: reopen it from the menu, or restart sdrtop.

### Not the R2

The **Airspy Mini** answers on the same USB id. sdrtop tells them apart by the
sample rates the firmware offers and leaves a Mini with SoapySDR, which drives it
today, unless `--device airspy` asks for the native backend by name: nobody here
has held a Mini, and its gains would be guesses. The **HF+ Discovery** is a
different radio with a different library, and it has
[its own section](#airspy-hf-discovery). In
[observer mode](advanced.md#observer-mode-when-another-app-owns-the-radio) an
Airspy is named by what it says on USB, `AIRSPY`, because USB can't tell an R2
from a Mini either.

## Airspy HF+ Discovery

Airspy sent this one along with the R2, and it is sdrtop's first radio built for
the bands below 30 MHz. Unlike the R2, it never worked through SoapySDR here:
SoapyAirspyHF hands its samples over as floats (`CF32`), sdrtop's pipeline is
integer, and the open was refused. So this is not a better road to a radio sdrtop
could already use. Before this backend, there was no road at all.

sdrtop drives it through libairspyhf, and it was rehearsed on this one: firmware
`R5.0.1-CD`, libairspyhf 1.8.1 for streaming. The 1.6.8 that most distributions
ship has the same calls, and sdrtop asks for nothing newer. The radio's own flash
holds a frequency correction in ppb, which libairspyhf applies to every tuning
and sdrtop logs at open. sdrtop never writes the flash.

The HF+ is a different kind of receiver from everything else on this page, and
most of what follows comes down to that. Its gain controls are digital, the radio
runs its own AGCs whatever you do, and a number that would mean "front end" on a
HackRF means something else here. sdrtop says so wherever it matters.

### Two bands and a gap

The Discovery tunes **9 kHz to 31 MHz** and **60 to 260 MHz**, which are Airspy's
published bands. The radio itself accepts any frequency without complaint and
shows a flat floor where it does not receive, so the bands come from Airspy, not
from asking it. A frequency typed into the gap is refused with both bands named.
`←` and `→` stop at a band's edge rather than jumping 29 MHz in one press: type
the frequency to cross. The header's dial leaves the gap blank, the menu's ruler
draws it dotted, and a sweep across it leaves a hole in the curve instead of
tuning the radio somewhere it cannot receive. Bluetooth and Wi-Fi are refused,
since 2.4 GHz is about ten times higher than this radio goes.

### Seven rates, one scale

The firmware offers 912, 768, 650, 456, 384, 228 and 192 kS/s. sdrtop starts on
768 kS/s at 7.100 MHz, because somewhere on 40 m there is always something.

**0 dBFS is the radio's 16-bit USB word**, where its samples actually clip. That
has one consequence worth knowing: the firmware filters and decimates each rate
with its own gain, so **a carrier's dBFS changes when you change the rate**, by
that rate's filter gain.

| Rate (kS/s) | 912 | 768 | 650 | 456 | 384 | 228 | 192 |
|---|---|---|---|---|---|---|---|
| Filter gain (dB) | 0 | 0 | 3 | 7 | 9 | 13 | 16 |

libairspyhf keeps this figure to itself. sdrtop reads it from the samples at the
start of every stream: the first block comes with the library's correction
switched off, where every sample is a whole word times that gain, so the gain can
be worked out exactly. That block is not shown, and the log says what was read.
On this firmware it matched the table at every rate. On another firmware it is
read the same way, not assumed.

The samples are carried as 32-bit integers with eight bits below the word. At
48 dB of attenuation the radio's own noise is less than one step of the 16-bit
word, and rounding the library's corrected output back to 16 bits lifted that
floor by up to half a decibel. With the extra bits the difference is gone (under
0.001 dB). Recordings of the HF+ are `ci32_le` for the same reason; see
[the SigMF extension](sdrtop.sigmf-ext.md).

**Rate changes stop the stream first.** On this firmware, switching to a low-IF
rate while streaming and then tuning down near 9 kHz left the radio unable to
stream again until it was unplugged. Airspy looked at the report and called it a
silicon problem. Changing the rate with the stream stopped never did it, so
sdrtop always does it that way, and the timing bench's clock restarts cleanly
afterwards.

### Where the LO is: the Library DSP switch

libairspyhf does two things to the samples by default. It corrects the I/Q
balance at the two zero-IF rates (912 and 768 kS/s), and it fine-tunes to the
hertz. The firmware itself tunes in whole kHz. At those two rates the library
also puts the radio's LO **5 kHz above** the frequency you asked for and shifts
the samples back, so the LO's DC leak sits 5 kHz off the centre instead of on it.
sdrtop follows the LO wherever it is. The I/Q bench reads the leak at
`+5.0 kHz`, the image scope mirrors about the LO, and the demodulator warns about
the LO's leak rather than a DC spike at the centre. The I/Q balance is **not
judged** there, because the samples have been turned off the LO and already
balanced by the library, so their balance says nothing about the radio. The
bench says so and gives the reason, and `[C]` declines to calibrate. At the low-IF
rates the LO is the centre and the balance is measured as usual.

The menu's **Options** pane has a **Library DSP** switch, on by default. Off, the
library leaves the samples exactly as the radio sent them. The radio then tunes
to the kHz grid, the centre moves to where it really is, and the header shows
that frequency: ask for 7.0743 MHz and it reads 7.074 MHz, and the log says
`tuned to 7.074000 MHz, -300 Hz`. Below 180 kHz the radio cannot be tuned with it
off, so there it is refused. The choice is saved as `[airspyhf] dsp`.

### ATT and LNA are digital

The Discovery has two gain controls, called what libairspyhf and every other
program call them:

- **ATT**: an attenuator, 0 to 48 dB in 6 dB steps, shown as a negative gain
  (`-48` to `0`). `↑` and `↓` move it.
- **LNA**: off or +6 dB. `[` and `]` switch it.

Measured on the bench against steady carriers, every attenuator step is 6.0 dB
and the LNA is 6.0 dB, to within 0.1 dB on VHF and 0.2 dB on HF.

What they are not is front-end gain. **By Airspy's account both are digital gain
after the converter**, and every measurement here agrees. They decide where the
level sits in the samples. They do not decide the signal-to-noise ratio, which
the radio's own AGCs set before them. So on the HF+:

- The chain reads `ANT▸AGC▸ADC▸ATT▸LNA▸word`, the gain lineup puts the stages
  after the converter, and the level diagram walks the samples back through the
  digital gain, not out to the antenna.
- The RF bench's verdict talks about placement: `LEVEL WELL PLACED`,
  `LEVEL LOW IN THE WORD`, `LEVEL HOT`, `LEVEL HELD`. It never promises that more
  LNA buys a better signal, and its footer says `SNR set in the radio`.
- The noise sweep (`[K]`) is refused. Digital gain moves the noise floor exactly
  as far as the signal, so there is no front end to measure.
- The converter bench is titled **Word Loading**, because what it sees is the
  USB word, after the digital gain. The radio's own converter is never visible.
- There is no noise figure card. Nobody publishes one per stage, and these
  stages would not have one anyway.

sdrtop starts with ATT at 0 and the LNA off, at first run and on `[R]`. On any
antenna whose band noise is above the radio's own, the LNA only lifts signal and
noise together and takes 6 dB of headroom.

### The radio's AGCs, and the `a` key

The radio runs an RF, an IF and a digital AGC. **By Airspy's account they are
always at work**, with the digital gain making up for whatever the analog ones
take, so the level in the samples stays put. The `a` key switches the radio's AGC
setting, saved per radio like every other boost. What it does, as far as anyone
can see from outside, is **hand the attenuator to the radio**. With it on, ATT
reads `AGC` and a manual setting no longer applies. The LNA stays yours. sdrtop
logs `AGC ON: the radio sets ATT; its AGCs are always on`. The library has no way
to ask what attenuation the radio chose, so sdrtop does not show one.

What it does not do is change the level of anything measurable. On a cable, with
a carrier driven up to 29 dB past the point where the output stops rising and
held there for six seconds, AGC off, on-low and on-high read the same to within a
tenth of a dB, at a level where one attenuator step left behind would have shown
as six. That fits Airspy's account: whatever the analog loops
take, the digital gain gives back. So there is no threshold control (it starts at
low and stays there), because there was nothing to measure it with.

### The hold, at about −12 dBFS

Push a strong signal in and its peak stops rising, cleanly, with no clipping and
no new products. That is the radio's digital AGC turning its own gain down. On
the USB word it holds a carrier's sample peak at:

| | Arriving well past the hold | After any gain command |
|---|---|---|
| HF (below 31 MHz) | −13.5 dBFS | −11.6 dBFS |
| VHF (60 MHz and up) | −14.8 dBFS | −13.3 dBFS |

The upper figures were measured on a cable at all seven rates, each within
0.2 dB, and the lower ones at 768 and 192 kS/s, repeatable to 0.1 dB. Where it holds depends on
what came before. A carrier that arrives well past the hold is held lower, and the
next gain command sent while it is there lifts the hold to the upper figure, even
one that changes nothing. That is the AGC's hysteresis being reset. Since sdrtop
sends a command only when you press a key, the lower figure is the one you will
usually see.

sdrtop shows the hold as a span (`HOLD -14 to -12 dBFS (digital AGC)` on HF) and
judges from its low edge, because past that point the level in the samples may no
longer be the antenna's. A peak there reads `HELD`, not `CLIPPING`, since nothing
is lost. The radio cut its gain and did not say by how much. Auto-gain (`A`) from
a held peak goes to the most attenuation first, since how far past the hold the
signal is cannot be known, and the next `A` lands it. That is also Airspy's own
advice for a strong signal: more ATT, LNA off.

### Timing

libairspyhf delivers on its own thread, eight blocks deep, 4096 sample pairs a
block with 1.8.1 (2048 with 1.6.8). sdrtop reads the size at open rather than
assuming it. On the deliberately old i3, five minutes at 912 kS/s ran at 13 to
14 % CPU with no drops, and the clock settled within ±15 ppm. Jitter is about
±1.5 ms at the two zero-IF rates, where the library balances I and Q on its
delivery thread, and about ±0.2 ms at the others. The `link util` line reads
against this radio's own top rate, so 768 kS/s shows about 84 %. That is correct,
not a warning.

### Observer mode, and the other HF+ boards

In [observer mode](advanced.md#observer-mode-when-another-app-owns-the-radio)
the HF+ is found on USB as `03eb:800c` and its product name, `AIRSPY HF+`. The
vendor id is Atmel's, shared with boards that are not radios, so a device on that
id that does not call itself an HF+ is not taken for one. sdrtop names the program
holding it.

The **HF+ Dual Port** and the **Ranger** answer on the same id and through the
same library. sdrtop tells them apart by the part id their firmware reports,
names them, and does not drive them. Nobody here has held one, and their
attenuators, front ends and rates would be guesses. Through SoapySDR they hit the
same `CF32` refusal as the Discovery. The Discovery has no bias tee, so there is
no option for one.

## SoapySDR: the honest version

Here is the thing. sdrtop supported two radios because I owned two radios, and I had
a rule: **hardware support lands only after physical testing.** It is a good
rule. It is also why every week somebody says they would try this if it spoke to
their Airspy, and every week the answer is "sorry, I do not have one".

So I broke my own rule, on purpose, in one specific place.

[SoapySDR](https://github.com/pothosware/SoapySDR) is one API with a lot of
radios behind it: the Airspy Mini, SDRplay's RSP line, PlutoSDR, LimeSDR, bladeRF, USRP,
and SoapyRemote for a radio on a different machine entirely. sdrtop now speaks
it. **The backend was written from the SoapySDR headers, not from owning the
devices**, which means it is exactly as good as the drivers are honest and as
careful as I could be reading a C header at midnight.

What I did instead of pretending otherwise:

- **Nothing about your radio is hardcoded.** Frequency range, sample rates, gain
  range, whether there is an AGC, whether there is a baseband filter, the sample
  format and how many bits actually mean something in it: sdrtop asks the driver
  for all of it. There is no table of devices in the source, because a table
  would be me guessing about hardware I have never held.
- **What cannot be asked is refused, not invented.** If sdrtop cannot work
  something out, that reading reads as unavailable. It does not fall back to a
  plausible number. A plausible number is the worst thing a measurement tool can
  produce.
- **When a driver disagrees with sdrtop, the log says so by name**, including the
  driver's own error text. That is in `~/.config/sdrtop/sdrtop.log`, and it is
  what makes a bug report actionable instead of a mystery.

A necessary evil, then, and it turned out to be less of an evil than I feared.
Before this, every row in the table above was green, and "tested on real
hardware" was a phrase that covered everything I shipped, which is another way of
saying it separated nothing. The grey rows are what give the green ones meaning.
A rule nobody has ever had a reason to test is not a rule, it is a habit, and
this one now has one marked edge on it. Marked being the whole point: an
exception you do not write down is not an exception, it is a rule you quietly
dropped.

### What is actually verified

| Thing | State |
|---|---|
| Loading libSoapySDR, ABI check, enumeration | Verified on a real install (0.8.1); 0.9 read from its headers and loaded in CI on macOS |
| Opening a device, reading its capabilities | Verified, on a HackRF through `SoapyHackRF` |
| Streaming, at full sample rate, no drops | Verified, same HackRF, 8 Msps and 10 Msps |
| An Airspy R2 through `SoapyAirspy` | Verified: streamed, and compared bench by bench with sdrtop's own [Airspy R2](#airspy-r2) backend |
| The two radios above, unaffected | Verified. The native paths did not change |
| A HackRF through **SoapyRemote** | Verified by a user, not by me: Raspberry Pi Zero 2 W server, Wi-Fi link, 1 Msps. See [below](#a-radio-on-another-machine-soapyremote) |
| **Every other radio** | **Unverified.** The code is right as far as I can reason. That is not the same as right |

So if you have an Airspy Mini, an RSP, a Pluto or a Lime: you are the test. I would
genuinely love an issue either way, working or not. "It works, here is what the
header said" is as useful to me as a crash.

### Using it

Nothing to enable. If libSoapySDR is installed, sdrtop finds it at startup and
your devices appear in the picker next to any HackRF, RTL-SDR or Airspy R2. If it is not
installed, sdrtop behaves exactly as it did before, same binary, no complaints.

```sh
# Is your radio visible to SoapySDR at all? Start here, always.
SoapySDRUtil --find

# Everything SoapySDR can see, and nothing else
sdrtop --device soapy

# One driver in particular
sdrtop --device soapy=driver=airspy
```

`SoapySDRUtil --find` is the first thing to run and the first thing to paste into
an issue. If SoapySDR cannot see your radio, sdrtop has no chance, and the
problem is a missing driver module rather than anything in here.

### A radio on another machine: SoapyRemote

SoapyRemote puts the radio on one machine and sdrtop on another: a HackRF on a
Raspberry Pi in the attic, sdrtop on the laptop downstairs. To sdrtop it is still
a SoapySDR device, so everything on this page applies. The network is just one
more stage in the chain, and usually the slowest one.

On the machine with the radio, install the server (`soapyremote-server` on
Debian and Ubuntu) next to the radio's own driver module, and start it:

```sh
SoapySDRServer --bind
```

On the machine running sdrtop, install the client module
(`soapysdr-module-remote`), then:

```sh
# Can this machine see the server? Start here, as always.
SoapySDRUtil --find="driver=remote"

# Only radios reached over the network
sdrtop --device soapy=driver=remote

# One server in particular, written exactly as SoapySDRUtil prints it
sdrtop --device soapy=remote=tcp://192.168.1.20:55132
```

The remote radio shows up in the picker as `SoapySDR · remote · HackRF One …`,
and from there it is the same radio as if it were plugged in locally.

Two things to know before you rely on it:

- **sdrtop finds the server the way SoapyRemote finds it on its own**, by asking
  the local network. If `SoapySDRUtil --find="driver=remote"` lists your radio,
  sdrtop will too. If it only shows up when you hand SoapySDRUtil the address
  yourself (another subnet, a VPN, a network that drops multicast), sdrtop
  cannot reach it yet: there is no way to give sdrtop the address up front.
- **Every sample crosses the network.** Pick a sample rate the link can carry.
  The setup this was verified on ran 1 Msps over Wi-Fi that managed about
  1.8 MB/s. If the link falls behind, the drop counters say so, and here the
  culprit is the network rather than the USB cable: lower the rate first
  ([more on drops](troubleshooting.md#samples-are-dropping)).

This works thanks to a user who owns this exact setup, found why it did not, and
sent the fix. That is exactly how the grey rows above are meant to turn green.

### The gotchas I already hit, so you do not have to

| Symptom | Cause / fix |
|---|---|
| Your **sound card** shows up as an SDR | It is real: `soapysdr-module-audio` presents audio inputs as SDR sources, which is genuinely useful with a soundcard receiver and confusing on a laptop. sdrtop skips the `audio` driver by default. Ask for it with `--device soapy=driver=audio` |
| A **SoapyRemote** radio is listed but **will not open** | A bug in 0.6.2 and earlier: the server address was dropped on the way from the picker to the open. Fixed; update sdrtop |
| Your HackRF, RTL-SDR, Airspy R2 or HF+ Discovery appears **once**, not twice, even with the Soapy module installed | On purpose. sdrtop's own driver for those four knows more about them than the generic path does, so the native one wins. Force the other with `--device soapy` |
| The **RF bench is missing** its noise figure, MDS and linearity card | Also on purpose. Those model a specific front end stage by stage. SoapySDR does not publish that, and inventing it would be worse than leaving it out |
| **No `[A]` boost** on your device | sdrtop asks the driver whether there is an automatic gain mode and only offers the key if there is. A HackRF reached through SoapySDR reports there is not, which surprised me too |
| `[` and `]` do nothing | Those two are the HackRF's VGA keys. Everywhere else, pick a stage with `,` / `.` and use `↑` / `↓`. See below |
| "its native sample format is CF32" in the log | sdrtop's pipeline is integer, so it handles `CS8`, `CU8` and `CS16` and refuses the rest by name rather than guessing at a conversion. Open an issue with the driver name |
| The frequency range looks **too optimistic** | It is the driver's number, not mine. `SoapyHackRF` claims 0 to 7.25 GHz where the datasheet says 1 MHz to 6 GHz. sdrtop reports what it is told; the radio will refuse the rest, and the log will say so |

### What a SoapySDR device gets less of

| | Native HackRF / RTL-SDR | Through SoapySDR |
|---|---|---|
| Gain | LNA and VGA on their own keys | Every element the driver names, each on its own range. `↑` / `↓` move the whole chain; `,` / `.` pick one stage, from any layout |
| Front-end boost | Always there | Only if the driver reports an automatic gain mode |
| Friis noise figure, MDS | Modelled per stage | Not shown. We do not know the chain |
| Linearity card (IIP3, IMD3, SFDR) | Shown | Not shown. Those are one front end's datasheet |
| ADC bench | 8-bit | Follows the converter the driver reports, so a 12 or 14-bit radio is described as one |
| Everything else | | The same |

### The gain, in more detail

This used to be one overall number, and one number was not enough. Handing a
driver a total and letting it decide the split is the standard thing to do, and
on a HackRF through `SoapyHackRF` what it decides is not what you want: asked for
60 dB it put most of it in the VGA and left the LNA low, which is the arrangement
with the *worst* noise figure of the ones available. Worse, it was not monotonic.
Turning the knob up could collapse the LNA from 32 dB to 19 on the way.

So sdrtop no longer asks the driver to split anything. It reads every gain
element the device names and every element's own range, and places gain itself:

- **`↑` / `↓` move the whole chain**, filling the front stage first, up to its
  ceiling, then the next. Front-first is the arrangement with the best noise
  figure, which is the arrangement you want unless you have a specific reason to
  want another one.
- **`,` / `.`**, from any layout, pick one element by name, and
  then `↑` / `↓` move that one alone, by its own step, leaving the rest exactly
  where they are. This is the only way to reach a third gain element, and some
  radios have one.
- **The config file names them too**, so a setup you worked out once survives a
  restart: `gain = "LNA=32,VGA=20"`. See [config.md](config.md#the-gain-line).

The element **names** are the driver's, not sdrtop's, and nothing is mapped onto
an LNA/VGA pair. A driver that calls its stages `IF1` and `IF2` gets rows labelled
`IF1` and `IF2`. Guessing which one is "really" the LNA is exactly the bug that
costs somebody an afternoon, and the names are right there to be read.

There is no table of devices behind any of this. If your radio's driver reports
its elements, it works; if it reports one element, you get one row and one knob,
which is the honest answer rather than a guess dressed as two.

---

## Host platforms

| Platform | Status |
|----------|--------|
| x86-64 Linux | Fully supported |
| Raspberry Pi (Pi 2 and newer, 64-bit Raspberry Pi OS Bookworm) | Supported, with lower sample rates on older Pis |
| ARM / Android (Termux) | Builds and runs; needs a root-capable USB stack to reach the device |
| macOS (Homebrew) | Unofficial, tested by the people who run it: [macOS, unofficially](#macos-unofficially) |

sdrtop loads libhackrf, librtlsdr, libairspy and libairspyhf at runtime. None of them is
required to build sdrtop. A missing or incompatible library disables only
its backend.

HackRF needs libhackrf 2023.01.1+ with all required symbols, including
`hackrf_board_rev_read` and `hackrf_usb_api_version_read`. Linux candidates are
`libhackrf.so.0` and `libhackrf.so`. Firmware metadata reads may fail without
preventing the device from opening.

RTL-SDR needs librtlsdr with all required symbols. Linux candidates are
`librtlsdr.so.0`, `librtlsdr.so.2` and `librtlsdr.so`. Runtime packages include
`librtlsdr0` on Debian, `librtlsdr2` on Ubuntu and `rtl-sdr` on Arch.

The Airspy R2 needs libairspy with all required symbols (1.0.10 is what it was
rehearsed with). Linux candidates are `libairspy.so.0` and `libairspy.so`.
Runtime packages include `libairspy0` on Debian and Ubuntu and `airspy` on Arch
and Homebrew.

The Airspy HF+ Discovery needs libairspyhf 1.6.8 or newer (1.8.1 is what it was
rehearsed with). Linux candidates are `libairspyhf.so.0`, `libairspyhf.so.1`
(Debian and Ubuntu rename it) and `libairspyhf.so`. Runtime packages include
`libairspyhf1` on Debian and Ubuntu and `airspyhf` on Fedora, openSUSE and
Homebrew. Arch has no official package, only the AUR's `airspyhf-git`.
Ubuntu 20.04's 1.1.5 is too old and is refused whole.

Library loading results are cached for the process lifetime. Unavailable-backend
diagnostics appear on explicit selection or when automatic discovery finds no devices.
Successful discovery stays quiet about unused native backends.
Loaded library handles stay open until process exit.
Restart sdrtop after installing or updating a library.

**libSoapySDR is not needed to build and not needed to run.** It is opened at
runtime if it happens to be there, which is why the same binary serves people who
have it and people who have never heard of it. On Debian and Ubuntu that is
`libsoapysdr0.8` plus a driver module such as `soapysdr-module-airspy`, or
`soapysdr-module-all` if you are feeling generous with disk space.

### macOS, unofficially

Linux is home, and that's not an oversight: it's the only system I have run
sdrtop on with a radio plugged in. The Mac build is built and tested on every
change, on GitHub's Apple Silicon machines, which have no radio. A user proved
the rest before I did: an M4 MacBook Air with an RTL-SDR Blog V4, through
Homebrew. [Observer mode](advanced.md#observer-mode-when-another-app-owns-the-radio)
works there too, reading what macOS's own device registry says about the radio
and the app holding it. The same user watched it switch over while `rtl_test`
held the V4, and it named `rtl_test` as the holder. Nobody has tried a HackRF
or a [tinySA](#tinysa) there yet (the tinySA is looked for on its
`/dev/cu.usbmodem*` port), so if you do, tell me what it said. sdrtop looks for the
libraries where Homebrew and MacPorts put them (`/opt/homebrew/lib`,
`/usr/local/lib`, `/opt/local/lib`), so this is the whole setup:

```sh
brew install rtl-sdr        # an RTL-SDR
brew install hackrf         # a HackRF
brew install airspy         # an Airspy R2
brew install airspyhf       # an Airspy HF+ Discovery
brew install soapysdr       # SoapySDR
brew install soapyrtlsdr    # and the driver module for your radio (soapyhackrf for a HackRF)
cargo install sdrtop --locked
```

The [one-line installer](getting-started.md) works on a Mac too: it takes the
macOS tarball for your Mac when the release carries one and compiles with cargo
when it doesn't (that needs Apple's Command Line Tools, `xcode-select
--install`), and its `--hackrf`, `--rtlsdr`, `--airspy`, `--airspyhf` and `--soapy` go through Homebrew.
I don't own a Mac, so this stays unofficial: tested by the people who run it. If something behaves
differently there, an issue is how I find out.

---

## What's coming

| Device | Status | Notes |
|--------|--------|-------|
| SDRplay RSP1B | Planned | On the desk, sent by SDRplay |
| Airspy Mini | Planned | Needs hardware to test |
| HackRF Pro | Planned | Needs hardware to test |
| LimeSDR / bladeRF / SDRplay / PlutoSDR | Try SoapySDR | These should already work through the SoapySDR backend. Nobody has told me yet |

**Native** device support means physically owning and testing the hardware, and
development here runs on a HackRF One and a PortaPack H4M, with an RSP1B from
SDRplay on the desk, waiting for its native backend. The Airspy R2 and the
HF+ Discovery that Airspy sent have their own now ([R2](#airspy-r2),
[HF+](#airspy-hf-discovery)). That is the whole bottleneck: the list moves at exactly the speed
of a hobby budget. The SoapySDR backend exists precisely because that speed was
not good enough.

If you'd like to move it along, the project has a
[GitHub Sponsors page](https://github.com/sponsors/musithang).
