# Airspy R2

← [Supported hardware](hardware.md)

---

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

## At a glance

| | |
|---|---|
| Range | 24 MHz to 1.8 GHz |
| Sample rates | 2.5 or 10 Msps |
| Gain stages | LNA, mixer and VGA, each a measured table; the knob walks Airspy's curves |
| Front-end boost | The tuner's AGC on the LNA and mixer (`a`) |
| Baseband filter | None, so the panel reads N/A |
| Friis noise figure | N/A: nobody publishes each stage's noise figure |
| Clip ceiling | The tuner's, about −3 dBFS |
| Samples | 12 bits, delivered as 16-bit I/Q |

## The gain is measured, not read off a box

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

## The knob is a curve

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

## The ceiling is the tuner, at about −3 dBFS

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

## The AGC, and what it costs

`a` switches the R820T's own AGC on the LNA and the mixer (the VGA has none).
While it's on, those two read `AGC` wherever a number would be, the chain total is
a dash, and auto-gain politely declines: the tuner is setting them, and sdrtop
doesn't know to what. Touch a gain by hand and the AGC switches off first, which
is also what libairspy's own presets do. One warning from the bench: the tuner's
AGC levels its own IF, not the converter, and on a strong FM station it parked the
signal right at the −3 dBFS ceiling. The curves are the everyday control; the AGC
is a choice, and that is its price.

## Options: packing and the bias tee

Both live in the menu's **Options** pane.

- **12-bit packing** sends 3 bytes per sample pair over USB instead of 4 (at
  10 Msps, 28.6 MB/s instead of 38.1), and the host pays for it in unpacking. It
  is saved under `[airspy]`, and switching it while streaming restarts the stream,
  because libairspy won't change it on a running one.
- **The bias tee** puts 4.5 V on the antenna port. It starts **off** every time,
  whatever it was last session, and it is never saved, because 4.5 V into an
  antenna port you rewired yesterday is how an LNA dies. Switching it is logged.
  Know what's on the end of the cable before you turn it on.

## Timing, and the rare short loss

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

## Not the R2

The **Airspy Mini** answers on the same USB id. sdrtop tells them apart by the
sample rates the firmware offers and leaves a Mini with SoapySDR, which drives it
today, unless `--device airspy` asks for the native backend by name: nobody here
has held a Mini, and its gains would be guesses. The **HF+ Discovery** is a
different radio with a different library, and it has
[its own page](airspy-hf.md). In
[observer mode](advanced.md#observer-mode-when-another-app-owns-the-radio) an
Airspy is named by what it says on USB, `AIRSPY`, because USB can't tell an R2
from a Mini either.

## Installing the library

`sh install.sh --airspy`, or your distribution's package from
[Getting started](getting-started.md#what-you-need).

The Airspy R2 needs libairspy with all required symbols (1.0.10 is what it was
rehearsed with). Linux candidates are `libairspy.so.0` and `libairspy.so`.
Runtime packages include `libairspy0` on Debian and Ubuntu and `airspy` on Arch
and Homebrew.

---

← [Supported hardware](hardware.md)
