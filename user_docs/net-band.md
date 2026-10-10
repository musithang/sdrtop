# The band · `NET 1` and `NET 2`

← [The NET section](net.md)

## Capability · `NET 1`

What the radio in your hands can do in this band, before a single sample
arrives. The verdict at the top (`5 OF 8 MODES`, `OUT OF BAND`) is composed
only from the facts drawn below it:

- **Tuner**: whether the tuning range covers the whole band, and with how
  much to spare either side.
- **Modes**: which Bluetooth and Wi-Fi modes the sample-rate ceiling can
  carry. Every mode is a line out to the rate it needs, on one log scale,
  and the radio's ceiling is a single orange rule down all of them: a green
  line that stops short of it fits, and one that runs past it turns red for
  exactly as far as it is short. 802.11b misses by a whisker on a HackRF;
  VHT80 misses by a postcode.
- **Retune** (`K`, focus `k`): times the radio's tuning call across the band.
  A call slower than the shortest BLE connection interval rules following a
  connection *by retuning* out (LE 3 follows without retuning, so it does
  not wait on this); a faster one is necessary but not proof, because the call
  does not include the synthesiser settling. The panel says which of the two
  it is and never "fast enough", because optimism is not a measurement.

---

## Survey · `NET 2`

What is in the band, measured one megahertz at a time. The band is measured
only while this view is open: the other views do not show it, and on the
old laptop sdrtop is tuned on, measuring it anyway cost the Bluetooth
receivers about a fifth of their time. Time spent elsewhere shows on the
Coexistence canvas as "nobody looked", so its seconds stay seconds.

### Occupancy *(focus `j`)*

The band's **duty cycle** per megahertz: the fraction of time something was
above the noise floor there. Duty is measured against a floor sdrtop
estimates from the band itself, and the floor has to pass its own checks
first; if it fails them, no duty is stated anywhere, because a duty against a
floor that is not a floor is a number about nothing.

A cell's coverage says what fraction of wall time it was actually watched: in
SURVEY about one position in the plan's hop count, in LOCK all of it. Coverage
is reported beside the duty, never multiplied into it. A cell never watched is
shown as not observed, which is different from a quiet one.

The profile's height is scaled to the room: the top of it is the first of 5,
10, 20, 50 or 100 % that holds the busiest cell, written on the dotted line
across the top (with half of it midway). A quiet office at 8 % gets bars that
fill the panel instead of a row of stubs, and the number on the line keeps
you from mistaking a tall bar for a busy band. Stepped back in time, the
bars are one plain colour: the history keeps duty, not power, so there is
nothing to colour them by, and pretending otherwise would be decoration.

The cursor (`←` `→` a megahertz at a time, `B` to the busiest cell) reads out
one cell, and `L` locks the receiver there.

### Coexistence *(focus `z`)*

The same band over time, as a heatmap under the occupancy profile, sharing
its frequency ruler. Decoded BLE packets and classic hits are marked over it
in their own colours, so "something is here" and "this is Bluetooth" read
from one picture. The colour runs on the same scale as the profile above it,
written in the footer (`colour 0–10 % busy`): an ordinary room used to sit in
the darkest tenth of a 0–100 % ramp, which made the whole history one
tasteful shade of navy. `↓` steps back in time and the profile above shows that
moment; `↑` forward; `N` back to now.

In SURVEY the classic receiver runs beside the band measurement on as many
channels as the measured decode load leaves room for: one more while the load
stays under 70 %, one fewer once it passes 80 %, starting from none. So the
key says what it is counting from. `■ BT 12` is every channel in view,
`■ BT 12 on 1 ch` is one channel because the load allows no more, and
`■ BT: not running, load` means the survey has the machine to itself, which
is not the same as a quiet band. On an old i3 that is about one channel at
8 Msps and none at 20; the classic views (`Classic 1` to `3`) always run
their full set.

### Decode health

The account of the feed and the decoders: blocks in, blocks lost, gaps, and
the BLE decode funnel (triggers, and how each one ended). Once the LE Coded
receiver has run this session, its funnel and its AuxPtrs are counted here
too; before that it is not mentioned, because this view never starts it. The funnel is also
one bar, good, CRC failed and gave up in their shares, and the decode load
is a bar against a rule at 100 %, red past it. Every count on the other NET
panels is only as complete as this panel says the feed was.

---

← [The NET section](net.md)
