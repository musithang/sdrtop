# The band · `Band 1` and `Band 2`

← [The NET section](net.md)

Before anything is decoded, two plainer questions: can this radio do the
job at all, and what is actually going on in the 83 MHz between 2400 and
2483? The first is answered before a single sample arrives; the second
takes a while, and a little patience.

## Capability · `Band 1`

What the radio in your hands can do in this band. The verdict at the top
(`5 OF 8 MODES`, `OUT OF BAND`) is composed only from the facts drawn below
it, so the headline and the evidence cannot disagree.

**Tuner**: whether the tuning range covers the whole band, and with how
much to spare either side.

**Modes**: which Bluetooth and Wi-Fi modes the sample-rate ceiling can
carry. Every mode is a line out to the rate it needs, on one log scale, and
the radio's ceiling is a single orange rule down all of them: a green line
that stops short of it fits, and one that runs past it turns red for exactly
as far as it is short.

| Mode | Sample rate it needs |
|------|----------------------|
| BT BR/EDR | 2 Msps |
| BLE 1M | 2 Msps |
| BLE Coded | 2 Msps |
| BLE 2M | 4 Msps |
| 802.11a/g/n HT20 | 20 Msps |
| 802.11b DSSS | 22 Msps |
| 802.11n HT40 | 40 Msps |
| 802.11ac VHT80 | 80 Msps |

A HackRF's 20 Msps carries the first five. 802.11b misses by a whisker;
VHT80 misses by a postcode.

**Retune** (`K`, focus `k`): times the radio's tuning call across the band.
A call slower than the shortest BLE connection interval, 7.5 ms, rules out
following a connection *by retuning*. (LE 3 follows without retuning, so it
does not wait on this.) A faster call is necessary but not proof, because
the call does not include the synthesiser settling or the samples already
in flight. The panel says which of the two it is, and never "fast enough",
because optimism is not a measurement.

## Survey · `Band 2`

What is in the band, measured one megahertz at a time. The band is measured
only while this view is open: the other views do not show it, and on the
old laptop sdrtop is tuned on, measuring it anyway cost the Bluetooth
receivers about a fifth of their time. Time spent elsewhere shows on the
Coexistence canvas as "nobody looked", so its seconds stay seconds.

### Occupancy *(focus `j`)*

The band's **duty cycle** per megahertz: the fraction of time something was
above the noise floor there. Duty is measured against a floor sdrtop
estimates from the band itself, and the floor has to pass its own checks
first. If it fails them, no duty is stated anywhere, because a duty against
a floor that is not a floor is a number about nothing.

A cell's **coverage** says what fraction of wall time it was actually
watched: in SURVEY about one position in the plan's hop count, in LOCK all
of it. Coverage is reported beside the duty, never multiplied into it. A
cell never watched is shown as not observed, which is a different thing
from a quiet one:

| A cell reads | Meaning |
|--------------|---------|
| a duty, high coverage | Watched a lot, and this busy |
| a duty, low coverage | Watched a little; the duty is as good as that little |
| not observed | Never watched. Not quiet, unknown |

The profile's height is scaled to the room. The top of it is the first of
5, 10, 20, 50 or 100 % that holds the busiest cell, written on the dotted
line across the top, with half of it midway. A quiet office at 8 % gets
bars that fill the panel instead of a row of stubs, and the number on the
line keeps you from mistaking a tall bar for a busy band. Stepped back in
time, the bars are one plain colour: the history keeps duty, not power, so
there is nothing to colour them by, and pretending otherwise would be
decoration.

| Key | What it does |
|-----|--------------|
| `←` `→` | Move the cursor a megahertz at a time; the cell under it is read out |
| `B` | Cursor to the busiest cell |
| `L` | Lock the receiver there |

### Coexistence *(focus `z`)*

The same band over time, as a heatmap under the occupancy profile, sharing
its frequency ruler. Decoded BLE packets and classic hits are marked over
it in their own colours, so "something is here" and "this is Bluetooth"
read from one picture. The colour runs on the same scale as the profile
above it, written in the footer (`colour 0–10 % busy`). An ordinary room
used to sit in the darkest tenth of a 0–100 % ramp, which made the whole
history one tasteful shade of navy.

| Key | What it does |
|-----|--------------|
| `↓` | Back in time; the profile above shows that moment |
| `↑` | Forward in time |
| `N` | Back to now |

In SURVEY the classic receiver runs beside the band measurement, on as many
channels as the measured decode load leaves room for: one more while the
load stays under 70 %, one fewer once it passes 80 %, starting from none.
So the key says what it is counting from:

| The key | Meaning |
|---------|---------|
| `■ BT 12` | 12 classic hits, from every channel in view |
| `■ BT 12 on 1 ch` | 12 hits from one channel, because the load allows no more |
| `■ BT: not running, load` | The survey has the machine to itself. Not the same as a band with no classic in it |

On an old i3 that is about one channel at 8 Msps and none at 20. The
classic views (`Classic 1` to `3`) always run their full set.

### Decode health

The account of the feed and the decoders: blocks in, blocks lost, gaps, and
the BLE decode funnel (triggers, and how each one ended). The funnel is
also one bar, good, CRC failed and gave up in their shares, and the decode
load is a bar against a rule at 100 %, red past it. Once the LE Coded
receiver has run this session, its funnel and its AuxPtrs are counted here
too; before that it is not mentioned, because this view never starts it.
Wi-Fi's receiver is treated the same way: once it has run, a `Wi-Fi` row
says which channel it is on, or why it is not running. Under it, the
`repeats` it has found, and what the long training field that follows each
one made of it:

| Row | What it counts |
|-----|----------------|
| `repeats` | What a frame's short training field looks like. Not yet a frame: a neighbour's repeats the same way, and so do some other signals |
| `this channel` | The long training field matched this channel's: a frame of ours |
| `L-SIG read` | Of those, the ones whose SIGNAL field passed its parity, named a rate the standard lists and ended in a zero tail |
| `greenfield` | The ones in 802.11n's greenfield format, which has no SIGNAL field: an HT-SIG right after the training, its CRC good |
| `unreadable` | The ones where neither passed its checks |
| `adjacent` | It matched a channel 5, 10 or 15 MHz away, judged on the part of it our 20 MHz can hear |
| `not a preamble` | It matched nothing |
| `not a short field` | It repeated better 0.4 µs apart than 0.8 µs apart, which a short training field never does (its tones cancel at 0.4 µs) and a Bluetooth burst nearly always does: turned away before the match, the costliest step of the receiver (a steady tone, alike at both, is left to the match). One case is given up knowingly: a frame through an echo two thirds as strong as itself, exactly 0.4 µs (120 m of extra path) late, at low signal |
| `cut short` | A break or a retune took it before it could be judged and its SIGNAL field heard (on the survey, every hop is a retune) |

`this channel`, `adjacent`, `not a preamble`, `not a short field` and
`cut short` add up to
`repeats`, and the three rows under `this channel` add up to it.

802.11b is counted beside it, because it is a different radio altogether
(the original 1999 one, still alive in every access point's beacons): no
training symbols, just eleven-chip Barker codes, found by their own
detector running on a core of its own.

| Row | What it counts |
|-----|----------------|
| `802.11b SYNCs` | Sixteen Barker symbols in a row at one timing: the start of an 802.11b frame, or something remarkably like one |
| `header read` | Its SFD found and its header's CRC good: rate, length and airtime known |
| `header refused` | The header failed its CRC or named a rate there is no such thing as |
| `no SFD` | The chips, but no start-of-frame where one should be |
| `SYNC cut` | A break or a retune took it before its header was held |
| `inside a frame` | Found again inside a frame already read, after a fade in its data: the same frame, not a new one |
| `another channel` | Chips centred whole megahertz from ours: a frame of another channel, read at its own centre (header or not) and never counted as this one's |

The six rows under `802.11b SYNCs` add up to it. Barker chips are
forgiving enough to be despread from a channel or two away, which is why the
last row exists: without it, the loudest access point nearby would be
credited to every channel it can be heard on. Anywhere with more than one
network in range (a block of flats, a café, a campus) expect a lot of all of
them; the air is busier than any list of networks suggests, and a channel
with no network on it still hears the ones around it.

The SIGNAL field (the L-SIG) is the symbol after the training that says a
frame's rate and length, and so how long it stays on the air. Every newer
Wi-Fi format sends one too, for the benefit of older radios, and it tells
them the truth about the time while fibbing about the rate: an 802.11n or
ax frame's L-SIG always says 6 Mb/s. So the Wi-Fi panel shows it as
`L-SIG 6 Mb/s`, the L-SIG's word, and the airtime beside it, which holds
for all of them (to the next 4 µs for the newer ones).

What the frame actually is comes from the symbols after it, told apart the
way the standard tells its own receivers to: a quarter-turned symbol for
802.11n (`HT-mixed`, with the MCS and length its HT-SIG gives, or
`HT-greenfield`), a repeated SIGNAL field for 802.11ax and 802.11be (`HE
SU`, `HE MU` and friends, `EHT`, or `PHY version 3, after EHT` for a radio
from a future this one was not written in), a quarter-turn one
symbol later for VHT (a 5 GHz format that a few 2.4 GHz chips send
anyway), and `non-HT` (802.11a/g) when none of those signatures is there.
Every signature is only believed under its own check: a quarter-turned
symbol whose HT-SIG fails its CRC is `unreadable`, because noise turns
symbols too.

An ax or be frame also says which network it belongs to before a single
MAC byte: a `BSS colour` from 0 to 63, whether it is going `uplink` (to
the access point) or `downlink`, and how long a `TXOP` it is holding the
air for. The colour is a label the access point picked, not an address, so
two networks can wear the same one; read it as "looks like the same
network", never as a name. And a four-bit CRC guards all of it, so about
one damaged field in sixteen gets through looking healthy. That is the
standard's choice, not ours, and the numbers are shown as what they are.

The standard makes every transmitter run its carrier and its sample clock
from one oscillator, so a crystal 12 ppm fast should be 12 ppm fast twice.
The Wi-Fi panel checks: the carrier offset comes from the training, and
the clock from the pilots drifting across a frame's data, so it needs a
frame of 30 data symbols or more (a non-HT one, or an 802.11n one with a
single stream). The `Clock` row shows the latest pair side by side, and
the `Clocks` row the difference over every frame this session, with a
chi-square that says whether the frames agree with one another. Near zero
with a chi-square near one: everybody's clocks are one, ours included.
Anything else is a finding, and it is left on screen rather than averaged
away; the bench does not yet know who sent which frame, so it cannot say
whose.

One such finding is already in, and it shaped the rows: the carrier read
from the preamble, a 6.4 µs look, wanders from the carrier over the rest
of the frame by about 4 kHz either way, on every transmitter we heard.
Someone's oscillator is jittery over microseconds (quite possibly the
HackRF's own; the bench cannot tell whose yet). So the `Clock` row's
carrier is the one read from the pilots across the data, where the
crystal shows, and the preamble's is shown beside it with how far it lay.
The `Last frame` offset is labelled as the preamble's for the same
reason.

Every count on the other NET panels is only as complete as this panel says
the feed was. If this one is unhappy, believe it over the others.

---

← [The NET section](net.md)
