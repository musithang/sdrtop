# Airtime · `Channel 3`

← [The NET section](net.md)

**What is spending this channel's time?** The [survey](net-wifi.md) says
how busy each channel is; [Frames](net-wifi-frames.md) shows the traffic
one frame at a time. This view sits between the two: one channel, second
by second, and what the time went on. The usual answer surprises people.
A few hundred quick OFDM frames a second can add up to a tenth of a
percent, while one old 802.11b beacon at 1 Mb/s, two milliseconds long,
ten times a second, quietly owns the place.

Lock the radio on a channel first, from the survey (`L`) or with `m` and
`← →`. Like every Wi-Fi view it needs exactly 20 Msps. It has no keys of
its own: nothing on it is selected or stepped.

## The time, second by second

The top panel is a column a second, the newest on the right, the axis
counting back from `now`. Each column is two bars, as on the survey:

- **Busy** (the left bar, the plain value colour): how much of that
  second the channel's busiest megahertz had anything in it, whatever
  made it.
- **Frames** (the right bar, stacked by format): the share of the second
  the frames read on the channel's centre said they would take.

The scale is fixed at 0 to 100 %, so a burst of contention looks like a
burst and a quiet minute looks quiet. The frames can stand taller than
the energy: a weak 802.11b beacon is read from its chips below the level
at which it counts as energy. Side by side, never nested, for the same
reason the survey gives.

The columns are the survey's own visits: locked, it keeps cutting the
channel's time into half-second visits, two to a column. A second a gap in
the feed cut into (heard less than 90 % of its span), or a second after
the first one heard in which no visit ended at all, is hatched `╱╱` in the
stale colour and left out of every number. A dropped block is not quiet
air, and it is not allowed to pass for it.

## The ledger

Under it, **what spent the time**: a row a format and rate (`802.11b
1 Mb/s`, `non-HT 24 Mb/s`, `HT MCS 7`), always in the standards' own order
(802.11b, a/g, n, ac, ax, be, then another channel's frames and the ones
not decoded) and never sorted by share, so a row you are reading does not
jump away from your finger.

| Column | What it says |
|--------|--------------|
| share | The airtime its frames stated, as a share of the time heard. `·` for a row whose frames stated none |
| frames/s | Its frames a second over the time heard |
| airtime each | The airtime a frame stated, averaged over the frames that stated one, or `not stated` |

A rate is shown only where a field read today names one: 802.11b's
header, a non-HT L-SIG, an HT-SIG's MCS. VHT, HE and EHT keep their rates
in fields not read yet, so their row is the format alone. The bar is
against the largest row; the percentage is the number to quote. Short of
rows, a format's rates fold into one (`non-HT · 4 rates`).

The ledger reads the frames the Frames view keeps, on this channel since
the stream last started or retuned, and says in its title how far back
that reaches (`over the last 6.8 s`) and how much of it a feed gap took
(`5.2 s heard of 6.0 s`). On a busy channel that is a few seconds; on a
quiet one, a minute and more.

**Who** spent it is the other half of the question, and the ledger's foot
says why it waits: one row a transmitter needs the MAC header, which is
not read yet. Grouping frames by their carrier offset instead would look
like devices and be nothing of the sort.

## The card: share of the time

On the right, the reading in big digits: the frames' share of the time
over the last 20 seconds that were heard whole, with its 1 sigma once
three are in. Under it, the energy's busy share on a line of its own, how
much of the time was heard, and, where the frames stand taller than the
energy, a line saying so (`over the energy: weak frames under its
threshold`). Then the same share split by format.

## The rhythm

At the card's foot, **the gaps between frames**: from the end of one
frame (the airtime it stated) to the start of the next, as a histogram
from 1 µs to 100 ms, ten bins a decade. The marks are SIFS (10 µs), DIFS
(28 µs, with the short slot) and 1 ms. It is how the medium breathes:

- A tall bin at **SIFS** is the exchanges: a frame and its
  acknowledgement, sixteen microseconds apart on paper.
- Sixteen? The OFDM formats in this band end with six microseconds of
  silence their stated airtime leaves out, the **signal extension**
  (802.11 clause 10.3.8). It is taken off before the gap is placed, after
  non-HT, HT and HE frames, so an exchange lands at SIFS where it belongs.
  802.11b has none and is placed as heard.
- A hump between **about 100 and 300 µs** on a channel full of 802.11b is
  contention: a long-slot DIFS of 50 µs and then backoff slots of 20.
- Further right are the pauses between bursts, and the beacons' rhythm.

Underneath, what fell outside the plot is counted: gaps **under 1 µs**
(nearly always a frame that began before the last one ended: another
channel's frame inside ours, or a collision) and those **past** 100 ms. A
gap after a frame not decoded is counted apart too, since nobody knows
whether that frame ended with an extension. EHT's own rule lives in the
802.11be amendment, which sdrtop has not read yet, so a gap after an EHT
frame is placed as heard rather than by a rule borrowed from another PHY.

When the card is short, the rhythm is the first thing to give way.

## When there is nothing to show

- **Not locked on a channel**: the survey is walking the band. The same
  plate as on Frames: `m` locks where you are, `← →` step the channel, or
  lock one on the Survey.
- **Listening**: nothing heard on this channel yet, or no second heard
  whole yet because the feed keeps dropping blocks. The card says which.
- **Not measured**: the radio is idle, at the wrong rate, or tuned off a
  channel centre. The plate says why, and which key helps.

## On a narrow screen

Under 140 columns the card gives way and the columns and the ledger take
the width. The reading rides in the top panel's title instead, in the
scale's place (`27.3 ± 12.3 % frames over 20 s`; the axis still says the
scale); the rhythm waits for a wider screen.
