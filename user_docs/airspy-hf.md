# Airspy HF+ Discovery

← [Supported hardware](hardware.md)

---

Most SDRs hand you a gain chain and wish you luck: set the gain too low and the
weak signals drown, too high and the first strong station clips the converter.
The HF+ Discovery is built the other way round. It manages its own levels,
inside the radio, all the time, and what reaches the USB cable is already scaled
to fit. On HF, where a big broadcaster a few hundred kHz away is normal, that is
the difference between a receiver you babysit and one you just use.

It also changes what half the numbers on a bench mean, which is why it gets a
page of its own.

## At a glance

| | |
|---|---|
| Bands | 9 kHz to 31 MHz, and 60 to 260 MHz |
| Sample rates | 912, 768, 650, 456, 384, 228, 192 kS/s |
| Samples | The radio's 16-bit word, carried in 32 bits with 8 bits below it |
| Gain controls | ATT (0 to 48 dB, 6 dB steps) and LNA (off or +6 dB), both digital, after the converter |
| Level control | The radio's own AGCs, always working; the `a` key hands them the ATT too |
| Ceiling | None that clips: the radio holds peaks at about −12 dBFS |
| LO | 5 kHz above the centre at 912 and 768 kS/s, at the centre at the others |
| Library | libairspyhf 1.6.8 or newer (rehearsed on 1.8.1), firmware `R5.0.1-CD` |
| Bias tee | None on the Discovery |

Before this backend the HF+ did not work in sdrtop at all: its SoapySDR driver
delivers floats (`CF32`), and sdrtop's pipeline is integer. Now a plain `sdrtop`
opens it natively.

## Not a classic gain chain

The radio runs automatic gain control in its RF, IF and digital stages, and all
three loops are always at work. When the analog stages back off for a strong
signal, the digital gain makes up the difference, so the level in the samples
stays where it was, to a fraction of a dB. You never see the radio protecting
itself. You only see that nothing clipped.

The controls keep the names every HF+ program uses, **ATT** and **LNA**, but
neither is what its name suggests. Both are digital gain, applied after the
converter: ATT sets the digital stage's gain down in 6 dB steps, and the LNA adds
one more 6 dB step. They decide where the signal sits in the 16-bit word. They do
not decide the signal-to-noise ratio, which was settled before them. Measured
against steady carriers, every ATT step is 6.0 dB and the LNA 6.0 dB, within
0.1 dB on VHF and 0.2 dB on HF.

ATT also works ahead of the hold described below. While the digital AGC is
holding a strong signal, more ATT brings it back under the hold, onto its line
again, where its level is the antenna's.

sdrtop draws the radio as it is:

- The chain reads `ANT▸AGC▸ADC▸ATT▸LNA▸word`. The gain lineup starts at the
  converter, and the level diagram walks the samples back through the digital
  gain, not out to the antenna.
- The RF bench talks about placement: `LEVEL WELL PLACED`,
  `LEVEL LOW IN THE WORD`, `LEVEL HOT`, `LEVEL HELD`. It never promises a better
  signal for more LNA, and the footer says `SNR set in the radio`.
- The noise sweep (`[K]`) is refused. Gain that moves the floor as far as the
  signal leaves no front end to measure.
- The converter bench is called **Word Loading**, since what it reads is the USB
  word. The radio's own converter is never visible from outside.
- No noise figure card. There are no per-stage figures to compute one from.

The keys: `↑` `↓` move ATT, `[` `]` switch the LNA. sdrtop starts with ATT at 0
and the LNA off, at first run and on `[R]`. On any antenna whose band noise is
above the radio's own, the LNA only lifts everything together and costs 6 dB of
headroom.

### The `a` key

`a` switches the radio's AGC setting, saved per radio like every other boost.
What it visibly does is hand the attenuator to the radio: ATT reads `AGC`, a
manual setting stops applying, the LNA stays yours. The radio does not report the
attenuation it picks, so sdrtop does not show one. The radio's AGC loops keep
working either way, and the log says so: `AGC ON: the radio sets ATT; its AGCs are
always on`. There is no threshold control: on a cable, low and high measured the
same, so sdrtop leaves it at low.

## The hold

Push a strong signal in and its peak stops rising at about −12 dBFS, cleanly: no
clipping, no new products. That is the digital AGC turning the radio's own gain
down, early enough to leave the word room for transients. Measured on a cable, as
the sample peak on the USB word:

| | Arriving well past the hold | After any gain command |
|---|---|---|
| HF (below 31 MHz) | −13.5 dBFS | −11.6 dBFS |
| VHF (60 MHz and up) | −14.8 dBFS | −13.3 dBFS |

The upper figures are the same at all seven rates, each within 0.2 dB; the lower
ones repeat to 0.1 dB. Which one you get depends on what came before: a carrier
that arrives well past the hold is held lower, and the next gain command sent while
it is there lifts it to the upper figure, even a command that changes nothing.
That is the AGC's hysteresis, and a command starts it afresh. sdrtop only sends a
command when you press a key, so the lower figure is the usual one.

So sdrtop shows the hold as a span, `HOLD -14 to -12 dBFS (digital AGC)` on HF, and
judges from its low edge: past it, the level in the samples may no longer be the
antenna's. A peak there reads `HELD`, not `CLIPPING`, because nothing was lost.
Auto-gain (`A`) from a held peak goes to full attenuation first, since how far past
the hold the signal is cannot be known, and the next `A` lands it. For a strong
signal that is the setting this radio is built for anyway: more ATT, LNA off,
and the signal sits comfortably inside the radio's numeric range, where it cannot
overload by construction.

## How hard it can be pushed

On a cable, a carrier was driven up to **29 dB past** the point where the output
stops rising, about 17 dB more than the USB word could carry, and held there for
six seconds. The output stayed at the hold, clean. The source's own spurs rose
with the carrier at the same dBc, and nothing new appeared. Afterwards AGC off,
on-low and on-high read the same within a tenth of a dB, where one attenuator step
left behind would have shown as six. Whatever the analog loops took, the digital
gain gave back. That is what "the radio protects itself" looks like from outside:
nothing to see. Which is the point.

The other end is just as quiet. At full attenuation the radio's own noise is less
than one step of the 16-bit word, and the library's corrections leave fractions of
a step. Rounding those back to 16 bits lifted the floor by up to half a decibel,
which is why sdrtop carries the samples in 32 bits with 8 below the word. The
difference then measures under 0.001 dB. Recordings of the HF+ are `ci32_le` for
the same reason ([the SigMF extension](sdrtop.sigmf-ext.md)).

## Two bands and a gap

The radio accepts any frequency and shows a flat floor where it does not receive,
so the bands are Airspy's published ones. A frequency typed into the gap is refused
with both bands named. `←` and `→` stop at a band's edge instead of jumping 29 MHz:
type the frequency to cross. The header's dial caps each band's edge and names
the gap between them (`╡ 31–60M ╞`), the menu's ruler draws it dotted, and a sweep across it leaves a hole rather than tuning
where nothing is received. Bluetooth and Wi-Fi are refused: 2.4 GHz is about nine
times higher than this radio goes.

## Rates, and what 0 dBFS means

sdrtop starts on 768 kS/s at 7.100 MHz, because somewhere on 40 m there is always
something.

**0 dBFS is the 16-bit USB word**, where the samples really clip. The firmware
filters each rate with its own gain, so a carrier's dBFS changes with the rate by
that gain:

| Rate (kS/s) | 912 | 768 | 650 | 456 | 384 | 228 | 192 |
|---|---|---|---|---|---|---|---|
| Filter gain (dB) | 0 | 0 | 3 | 7 | 9 | 13 | 16 |

The library does not publish this figure. sdrtop reads it from the samples at the
start of every stream: the first block comes with the library's corrections off,
where every sample is a whole word times that gain, so the gain comes out exact.
That block is not shown, the log says what was read, and on another firmware it
is read the same way rather than assumed.

**A rate change stops the stream first.** On this firmware, switching to a low-IF
rate while streaming and then tuning near 9 kHz can leave the radio unable to
stream again until it is unplugged. Airspy traced it to the silicon. Changing the
rate with the stream stopped never triggers it, so sdrtop always does it that way.

## The LO, and the Library DSP switch

The firmware tunes in whole kHz. libairspyhf adds the rest by default: it
fine-tunes to the hertz, and at the two zero-IF rates (912 and 768 kS/s) it
balances I and Q and puts the LO **5 kHz above** the asked frequency, so the LO's
DC leak sits off the centre instead of on top of your signal.

sdrtop follows the LO wherever it is. The I/Q bench reads the leak at `+5.0 kHz`,
the image scope mirrors about the LO, the demodulator warns about the LO's leak.
The I/Q balance is **not judged** at those two rates: the samples have been turned
off the LO and already balanced, so their balance says nothing about the hardware.
The bench says so, and `[C]` declines to calibrate. At the low-IF rates the LO is
the centre and the balance is measured as usual.

The menu's **Options** pane has a **Library DSP** switch, on by default. Off, the
samples arrive exactly as the radio sent them, the radio sits on the kHz grid, and
the header shows the frequency it really has: ask for 7.0743 MHz and it reads
7.074 MHz, with `tuned to 7.074000 MHz, -300 Hz` in the log. Below 180 kHz it
cannot be tuned with the switch off, so there it is refused. Saved as
`[airspyhf] dsp`.

## Timing

libairspyhf delivers on its own thread, eight blocks deep, 4096 sample pairs a
block with 1.8.1 (2048 with 1.6.8); sdrtop reads the size at open. On the
deliberately old i3, five minutes at 912 kS/s took 13 to 14 % CPU with no drops,
and the clock settled within ±15 ppm. Jitter is about ±1.5 ms at the zero-IF
rates, where the library balances I and Q on its delivery thread, and about
±0.2 ms at the others. `link util` reads against this radio's own top rate, so
768 kS/s shows about 84 %: correct, not a warning.

## Observer mode, and the other HF+ boards

In [observer mode](advanced.md#observer-mode-when-another-app-owns-the-radio) the
radio is found on USB as `03eb:800c` with the product name `AIRSPY HF+`. That
vendor id is Atmel's and turns up on boards that are not radios, so the name has
to match too. sdrtop names the program holding it.

The **HF+ Dual Port** and the **Ranger** answer on the same id and the same
library. sdrtop tells them apart by the part id their firmware reports, names
them, and does not drive them: nobody here has held one, and their attenuators,
front ends and rates would be guesses. Through SoapySDR they meet the same `CF32`
refusal.

## Installing the library

`sh install.sh --airspyhf`, or your distribution's package from
[Getting started](getting-started.md#what-you-need). Which library versions and
file names sdrtop accepts is under
[host platforms](hardware.md#host-platforms).

---

← [Supported hardware](hardware.md)
