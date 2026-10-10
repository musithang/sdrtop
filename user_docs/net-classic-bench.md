# Bench · `Classic 3` *(focus `c`)*

← [The NET section](net.md)

The same piconet, one end against the other. A piconet is two radios
taking turns, and [Classic 1](net-classic.md) pools them into one account;
here each gets its own row, so when one of them is the problem, it is
plain which.

`3` opens the bench on the piconet selected on Classic 1, and `← →`, with
the bench focused, step through the piconets. Three columns, MODULATION,
CARRIER and TIMING, and every reading in two rows: `▶` the master's, in
the piconet's colour, and `◀` the slave's, in the ordinary ink. Each row
carries its bar against the limit the column's heading cites, and the
heading says what the readings rest on (`258 hdr`, `1204 hits`). The
MODULATION column, drawn by the panel itself from a test's numbers (which
is why it is tidier than anything off the air):

```
├╴ MODULATION ╶── 258 hdr · BR limits: Core 5.4 Vol 2 A 3.1.1
 Mod index   ▶ 0.3240 ±0.0003      [0.28 ········|···· 0.35]
             ◀ 0.3160 ±0.0003      [0.28 ······|······ 0.35]
 df1 avg     ▶ 162.00 ±0.17 kHz    [140 ·········|····· 175]
             ◀ 158.00 ±0.17 kHz    [140 ·······|······· 175]
 df2/df1     ▶ 0.9393 ±0.0019      [        min 0.8        ]
             ◀ 0.9395 ±0.0019      [        min 0.8        ]
```

A side with nothing yet says so in words (`not measured`, `collecting`,
`0 of 8 timed`), never with a zero that would look like a measurement. Only
a packet whose header placed it on a side counts for that side; the rest
are counted under TIMING as not yet placed, and kept out of both columns.

## Modulation

| Row | What it is | Limit (Core 5.4 Vol 2 Part A 3.1.1) |
|-----|------------|-------------------------------------|
| `Mod index` | The BR modulation index | 0.28 to 0.35 |
| `df1 avg` | The deviation on a run of equal bits | 140 to 175 kHz |
| `df2/df1` | The deviation on alternating bits, as a share of `df1` | at least 0.8 |

All three come from every header's symbols, read the way the SIG's test
suite defines them, which took some doing. A tester commands the device to
send `00001111` and `10101010` and reads particular bits of them. sdrtop
cannot ask a stranger's headphones for anything, so it reads every bit
whose two neighbours make it one of those bits (the same either side, or
opposite either side), which for this kind of modulation is the same
measurement, and a test holds it to the suite's own figures.

Each header is taken again from the raw samples and read at bit centres
timed from the packet's own sync word. The receiver that finds the packets
guesses where a bit's centre is, which is fine for finding packets and was
not fine for measuring them: until this was built, it read a perfectly
healthy transmitter's `df2/df1` as about 0.45. That is how this whole
section learned to check itself against a reference first.

The suite's own recommended filter turned out to be the wrong one for
traffic: it is built for a tester reading its own test patterns, and bends
ordinary traffic by a few percent. So the reading uses a wider one, and the
price is the next channel. Up to 20 dB below the transmitter being measured,
a neighbour moves the figures by less than 1 %; a header with anything
louder next door at the same moment is not read at all, just counted
(`headers not read: the next channel was busy at the time`).

A weak signal makes `df2` scatter, one reading per bit at its centre
carrying all the noise. The `±` beside it says how much, and at 20 dB above
the noise it also leans a percent or so, which the `±` does not cover.
Stronger is better, as with most things in radio and few things in life.

## Carrier

| Row | What it is | Limit (Core 5.4 Vol 2 Part A 3.1.3) |
|-----|------------|-------------------------------------|
| `f0` | The initial carrier offset, from the four preamble bits; the mean over every header measured | ±75 kHz, judged only once a reference makes it absolute |
| `Drift worst` | How far the carrier wandered from `f0` over the access code and header; the worst header's | ±40 kHz |
| `Rate worst` | The steepest step of that wandering; the worst header's | ±400 Hz/µs |

The drift and its rate are the worst header's, because the limits are on
every packet, not on the average one. `f0` is relative to our own
oscillator, like every ppm in NET (see [what an offset is
worth](net-trust.md#what-an-offset-is-worth)), until a reference makes it
absolute. The drift is held to the 40 kHz every packet type has to meet: a
one-slot packet's tighter 25 kHz is over its whole length, which a header
alone cannot show.

Every bit read here is either known (the whole access code) or decided
again from the measurement itself, never taken from the part of the
receiver that finds packets: one wrong bit there once looked like 30 kHz of
drift. Noise makes drift too, as it does on BLE; the Modulation note in
[the Advertising view's detail](net-le.md#packet-detail) has the numbers.

## Timing

Each end's timing on the piconet's 625 µs slot grid, fitted to every
member's hits. Below eight hits it is collecting. Hits that do not line up
on a grid beyond chance are refused as one, never forced onto it: given
enough periods to try, a dozen points will line up with almost anything,
and the panel would rather say `no slot grid` than find the grid it wanted
to find. The piconet's clock from the same grid belongs to the whole
piconet, so it is on [Classic 1](net-classic.md).

| Row | What it is | Limit (Core 5.4 Vol 2 Part B 2.2.5) |
|-----|------------|-------------------------------------|
| `Jitter max` | The furthest one end's packet strayed from that end's own average timing | 1 µs |
| `rms` | The same scatter, as an rms | none: a reading, not a verdict |
| `offset` | How far the slave's packets sit from the master's, `the slave after the master` or before | none |
| `packets` | How many packets each end has sent | |

Jitter is measured against each end's own average, which is how 2.2.5
states it. The grid belongs to neither end, so where each end sits on it
means little alone; the `offset` is the difference of the two averages,
and that the grid cannot move.

Under them, the residuals as a shape, the ±1 µs limits ruled, **stacked by
who sent them**: the master's in the piconet's colour, the slave's in the
ordinary ink, and the packets of unknown sender in the stale grey on top,
never put on a side. One hump is one timing; a slave answering a little
late on every slot stands as a hump of its own colour.

## The plots

Under MODULATION the index, and under CARRIER `f0`, are **plotted** over
the last minute: both ends on one time axis and one scale, each in its
colour, so whether the two read alike and whether either is moving shows
at a glance. Each point is the mean of a few packets, and the title says
how many (`index, means of 4 packets`): one header's reading is noisy, and a
line of single readings is a band that shows the noise and hides the drift.

The scale's ends are written on the left. A line that fills the plot across
`0.333` to `0.335` is a transmitter holding steady; one across `0.30` to
`0.34` is warming up. A slowly moving `f0` in the minutes after a device
switches on is exactly that, and not a reason to return the headphones.

## On a small panel

Narrower than three columns, the bench draws as many sections as fit, in
order, and names the rest (`+ TIMING on a wider panel`). Shorter than its
sections, a column shrinks its plots first (six rows, then three, then
none), keeps its readings whole, and says the plots are on a taller panel;
only then do readings give way from the bottom, and it says that too.

---

← [The NET section](net.md)
