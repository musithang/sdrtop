# SDRplay RSP1B

← [Supported hardware](hardware.md)

---

SDRplay sent this one. It is a 14-bit receiver from 1 kHz to 2 GHz, and it is
the most polite radio sdrtop drives: it never talks to sdrtop directly. It talks
to SDRplay's service, which runs as root, which talks to SDRplay's API, a closed
library, which talks to sdrtop. Every number on this page went through that
chain, and a few of them are about the chain itself.

Everything here was measured on the RSP1B on the desk, mostly on a cable from a
HackRF, with no attenuator and a great deal of care.

## At a glance

| | |
|---|---|
| Range | 1 kHz to 2 GHz |
| Sample rates | 62.5, 125, 250, 500 kS/s, 1, 2, 2.048, 3, 4, 5, 6, 7, 8, 9, 10 Msps |
| Samples | 16-bit I/Q at full scale 32768, carrying 14 bits up to 6 Msps, 12 from 6.048 to 9 Msps, 8 from 9.216 Msps |
| Gain controls | LNA (the radio's states, 7 to 10 by band, measured in dB per band) and IF (39 dB, measured) |
| The knob | SDRplay-style curves, Sensitivity and Linearity, built from the measured tables |
| AGC | SDRplay's, on the IF only, set point -30 dBFS |
| Ceiling | The converter's 0 dBFS; the radio's own overload flag fires at about -12 dBFS peak |
| Corrections | SDRplay's DC and I/Q correction, on by default, switchable |
| Notches | AM/FM and DAB, switchable |
| Bias tee | 4.7 V on the antenna port, off at every start |
| API | SDRplay API 3.15 (3.14 is refused by name: its layout was never checked) |

A plain `sdrtop` opens it natively. It needs the service running, which is the
first thing to check when it doesn't (`sudo systemctl start sdrplay`).

## The gain is measured, not copied

SDRplay publishes a table of how much each LNA state takes away. The radio on
the desk agrees with it under 50 MHz to about 1 dB. From 50 to 420 MHz it takes
2.5 to 6 dB more than the table from state 4 on, from 420 MHz to 1 GHz 1 to 4 dB
more, and in L-band the shape of the table itself changes with frequency (state
3 is 20.8 dB down at 1200 MHz and 15 at 1900). So sdrtop carries its own
tables, one per band, measured at 14 frequencies, and shows the LNA in dB for
the band you are tuned to. The IF is one staircase of 39 dB, about 1 dB a step
and every sixth step 1.25, the same everywhere.

Checked afterwards against a carrier, a step at a time across every state: the
gain on screen follows the carrier to within 0.2 to 0.5 dB at 12 of the 14
frequencies. **300 MHz is the exception at about ±0.8 dB**, repeatably, for a
reason not yet pinned down.

### The mixer trap

Some LNA states bring a mixer's gain reduction in with them, and SDRplay's table
says so in a footnote. What the footnote does not say: in those states, with the
IF turned up past about 36 dB, more IF gives you 9 to 15 dB *less* gain. sdrtop
keeps out of it with one rule for every band: LNA states 4, 6, 7, 8 and 9 never
get the IF's last 3 dB. One rule, because a setting legal in one band would
otherwise land in another band's trap the moment you retune. Set by hand, it is
refused and the log says why.

### States a band does not have

Under 50 MHz the radio has seven LNA states, above 1 GHz nine, in between ten.
SDRplay's API refuses a state the band does not have, even to retune or to
start. So when you retune from 150 MHz with LNA state 8 down to 14 MHz, sdrtop
gives the radio state 6, the band's last, and says so:
`LNA: state 6 at 14.000 MHz, as the radio holds it (state 8 before)`. Tune back
and you get state 8 again.

### The corner nobody can measure

In the high-gain states with the IF near its top, the RSP1B clips on its own
noise: with nothing at all at the input, LNA 0 sits at -2 dBFS rms from about
half the IF upward. No signal can measure a gain there, so the curves stay out
of that corner. The keys can still take you there, and the ADC bench will show
you the clipping.

## The knob is a curve

Two stages, two ways to spread a total over them:

- **Sensitivity** gives the LNA as much as it can and lets the IF do the rest.
  The lowest noise figure: at the same total gain it showed 15 dB less of its
  own noise than Linearity.
- **Linearity** turns the IF up first and the LNA later. The most room for a
  strong neighbour.

`↑` `↓` walk the curve in steps of about 2 dB, built per band from the measured
tables, so a step means the same in every band. `,` `.` go round the curves and
then each stage by hand. The header reads `LIN 21/50 ▸ LNA 7 · IF 18   42 dB`.
A first run starts on Linearity at about 42 dB, which puts a quiet band's noise
some 15 dB over the converter's own.

## The AGC

`a` switches SDRplay's AGC. It moves the IF only, never the LNA, and it is set
to -30 dBFS (SDRplay's own default of -60 sits under a quiet band's noise and
does nothing at all). While it works the header says what it picked, from the
radio's own reports: `AGC ▸ LNA 3 · IF AGC 28 dB   77 dB`.

Two rules come with it. The AGC runs the IF straight into the mixer trap, so it
is refused in LNA states 4 and 6 to 9 (at LNA 8 it was measured sitting 5 to 10
dB short of its set point, for good). And when you switch it off, sdrtop gives
the radio back your IF: left alone, the radio stays wherever the AGC last was.

## Corrections, image and notches

SDRplay's API removes DC and balances I and Q before sdrtop sees a sample. It is
on by default, because the radio's own balance is not great above VHF: the image
sits 45 dB down at 150 MHz, 34.5 at 450 and 28 at 950 without it, and 58 to 66
dB down with it. While it is on, the I/Q bench says `after SDRplay's correction`,
since what it measures is what the correction left. Switch it off in the Options
pane to see the radio's own balance. The choice is kept in `[sdrplay]`.

The notches, also in the Options pane and also kept: the AM/FM notch takes about
51 dB at 100 MHz, the DAB notch about 29 at 200 MHz. Tuned where one acts, the
header says `AM/FM NOTCH IN` and the RF bench's chain shows it, since every level
on the deck is then read through it.

The bias tee is in the Options pane too, and is never kept: it powers whatever
is on the antenna port, so it starts off every time.

## Rates

Each rate is made one way. 2 to 10 Msps are the converter's own, at zero IF.
Under 2 Msps, the API decimates 2 Msps by 2 to 32. 2.048 Msps comes from 8.192
MHz at SDRplay's 2.048 MHz low IF, with the LO outside the output. A rate change
while streaming restarts the stream: changed live, the API's sample count stops
matching the samples (a decimation change jumped it by 14 million), and every
mode started fresh counts true.

## Timing, and the service in the middle

The service holds the stream and hands it over in bursts, up to about 70 ms at a
time. That is its rhythm, not the USB link's, so the timing bench does not grade
it against a deadline: it shows how long the service held the stream (`HELD ≤ 65
ms`), and reads the sample clock from the deliveries that came earliest, good to
about 70 ppm through the service. Transfers are bulk, not SDRplay's isochronous
default, which lost the odd quarter-block over minutes.

On the deliberately old 2-core i3, up to 6 Msps runs with room to spare. 10 Msps
sits at the edge: sdrtop takes about one core and the service half of another,
and if anything else wants the CPU at the same time, some samples are lost. The
log says when: `Lost 1612800 samples: driver 0 · sdrtop 1612800`.

**If the service stops or restarts under a stream**, SDRplay's API sends no word
of it: the samples just stop, two of its threads start spinning, and it can
never be used again in that run. sdrtop notices the silence after 3 seconds,
says `SDRplay's service stopped answering`, and leaves the API alone so that `q`
still quits. Start sdrtop again.

## Observer mode, SoapySDR, and the other RSPs

A radio another program holds through SDRplay's service is not in SDRplay's list
at all. sdrtop finds it on USB (`1df7:3050`) and goes to
[observer mode](advanced.md#observer-mode-when-another-app-owns-the-radio). The
program holding it cannot be named, since the system names the service whoever
is listening; what sdrtop can say is which of your programs has SDRplay's API
loaded, so it does: `SDRplay's API loaded in: SoapySDRUtil (PID 217897)`.

`--device soapy` still reaches the RSP1B through SoapySDRPlay3. The two never
share a process: SoapySDRPlay3 keeps its own session of SDRplay's API, and two
of those in one process wreck each other at exit. So a plain search leaves
SoapySDRPlay3 out whenever the native backend is there.

The other RSPs (RSP1A, RSP2, RSPduo, RSPdx) are listed by name and not driven
natively: nobody here has held one. SoapySDR is the road to them, with the
[usual honesty](soapysdr.md) about what that means.

## macOS

SDRplay ships its API for macOS too, and sdrtop looks for it where SDRplay's
installer puts it. Nobody has run an RSP with sdrtop on a Mac yet, so that part
is reasoned, not verified. If you try it, tell me what it said.

## Installing the API

SDRplay's API comes from [SDRplay](https://www.sdrplay.com/api/), as an
installer that also sets up the service (on Arch, `libsdrplay` from the AUR). No
distribution carries it. `sh install.sh --sdrplay` checks that the API is there
and the service is running, and tells you what to do if not. Then:

```sh
sudo systemctl start sdrplay     # now
sudo systemctl enable sdrplay    # and at every boot
```

sdrtop looks for `libsdrplay_api.so.3` and `libsdrplay_api.so`. On macOS
SDRplay's installer uses the same `.so` names, looked for bare and under the
Homebrew and MacPorts prefixes.

---

← [Supported hardware](hardware.md)
