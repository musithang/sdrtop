# FT8 · Radio › Digital modes › FT8

← [User guide](README.md)

Every fifteen seconds, on the minute and every quarter of it, thousands of
radio amateurs around the world send each other about thirteen seconds of
eight tones, a few hertz apart, too quiet to hear. That is **FT8**: a
callsign, a locator, a signal report, and an exchange that is over in about
a minute and a half. It is designed to get through when nothing else does,
and it does, down to about 20 dB under the noise.

This view decodes it and puts it on a map. The decoder is sdrtop's own,
written from the protocol's definition (Franke, Somerville and Taylor, "The
FT4 and FT8 Communication Protocols", QEX 2020) with no library: our own
message unpacking, our own error correction, our own demodulator. There is
no sound, as everywhere in sdrtop, and no transmitting: this listens.

## Before you listen

**Tell sdrtop where you are.** Open the menu, **Options › Station**, and type
your latitude and longitude, and your callsign if you have one. The map is
drawn around that position, and without one there is no map at all: the
panel says so rather than guessing. The call is what marks a message as
addressed to you. Both are kept in the config's
[`[station]`](config.md) block and survive a change of radio, since you are
not the radio.

**An HF radio.** FT8 lives on the short-wave bands, 160 m to 10 m, and on
6 m and 2 m. The Airspy HF+ Discovery is the radio this was first heard on,
on 40 m one October evening; anything that reaches the bands and gives
sdrtop IQ samples will do.

## Getting there

Open the menu, choose **Radio**, and go in: **Digital modes › FT8 ›
World**. Opening the view retunes nothing. Then tune to an FT8 frequency:
`←` `→` step between the conventional ones the radio reaches (40 m is
7.074 MHz, 20 m 14.074), or `f` types one. Start RX with `Space` and wait a
cycle or two.

The tuned frequency is the **dial**, as on any radio: FT8 is upper sideband,
and the signals sit 0 to 3 kHz above it. The strip at the top says which
band that is (`40 m · 7.074 MHz`), or, in warning ink, that the frequency is
not one of FT8's and `←` `→` will step to one. The conventional list is
WSJT-X's, taken as data; operators agree on it, the protocol does not.

The decoder wants 12 000 samples a second, and takes them from whatever rate
the radio runs by decimating, and resampling where it has to. A rate it has
no route from is said on the strip (`pick another rate with S`). An FT8
band is 3 kHz wide, so the lowest rate your radio offers is plenty.

## The view

### The strip

Across the top, one line:

- **The band**, as above.
- **The last minute's cycles**, each with its start, how many decodes it
  gave, and `E` or `O` for even or odd (even cycles start on the minute and
  the half minute). A cycle the stream broke in says **cut**: never shown as
  a quiet one. A cycle the decoder had no time for, because it was still on
  the one before, says **skip**.
- **The cycle being heard**: a bar filling over its fifteen seconds, and
  `decoding` while the decoder works on the one before. `feed broke 3×` in
  warning ink says how often the sample feed broke this session; each break
  costs the cycle it falls in.
- **The clock**, on the right. More on it [below](#the-clock).

### The map

The world around you, on an azimuthal map: you are in the middle, and every
straight line from you is the shortest way round the Earth, so the direction
on the map is the direction to point an antenna. `z` steps the range: the
whole world on a logarithmic scale (the near ground opened out, the far side
squeezed toward the rim), then 1 500, 3 000 and 9 000 km to the edge, true
to scale. On the shorter ranges the Maidenhead fields are drawn and named.

**A station is where it said it was.** FT8 messages often carry the
sender's locator, and that is where its dot goes: the middle of that
four-character square. A station that never sent one is not on the map, and
nothing is guessed. Dots are coloured by how fresh they are (this cycle, the
last 45 s, earlier, older) and red if the station called you; this cycle's
get a line from you, and you are the diamond, in a colour no station
wears.

The land is lit by the sun for the cycle being heard: day, night, and the
**grey line** between them, the band of twilight where the low bands
famously travel far.

The title names the **best DX**, the farthest station heard inside the
map's time window, and how far.

`m` focuses the map and hands it a few more keys: the arrows move from
station to station across the map, `Enter` zooms on the chosen one, and
switches turn the rays, the grid, the rings, the sun and the names on or
off, set how far back the map reaches, keep only this band's stations, or
draw the long way round to the chosen station. They are all in
[the keyboard guide](keys.md#ft8-map-focus-mode). Whatever a switch hides,
the title says, so a sparse map is never mistaken for a quiet band.

### The band

The 3 kHz above the dial as a river, a row a cycle, newest at the top: every
decode is a bar on its tone, as tall as its SNR, with its call beside it
where there is room. Energy that rose above the noise and did not decode is
shaded where it was, so you can see what the decoder could not read. The
dashed strip at the band's low end is the radio's own DC, where nothing
sits.

### The decodes

Cycle by cycle, newest first, each cycle under a rule that says its time,
even or odd, how many it gave, and the band (so decodes from before a band
change say which band they were on). In each row: the SNR, the DT, the tone
in hertz above the dial, the message, and the distance and bearing where
the sender's position is known (`no locator` where it is not). Your call is
red, `CQ` green, the sender bold, and a `▶` marks a message to you.

`u` shows only those, and the title says the list is filtered. `↑` `↓`
choose a station down the list; the map rings it, the river and the list
light every row of it, and the card shows it. `c` forgets everything heard.

### The station

The chosen station's newest decode, as received:

| Row | What it says |
|-----|--------------|
| **Where** | Its locator, how far and in which direction, the short way and the long way round, and `▶ called you` if it did |
| **The message** | In quotes, as sent, with its cycle |
| **SNR** | Signal over noise in 2500 Hz, the way FT8 reports it, so the figure compares with the report the other station sends |
| **DT** | How far its first tone started from where a cycle's should, 0.5 s in, to the millisecond |
| **Tone** | Tone 0's frequency above the dial, with its spread, one standard deviation |
| **Drift** | How far the frequency moved over the 12.64 s of the transmission |
| **Sync** | How many of the 21 Costas tones (the three 7-tone sync patterns) stood on their own tone |
| **Path** | `steady` or `fading`, and the coherence that says which: of the signal's power, the share one steady carrier explains. Near 1 when the path held still for the 12.64 s; under 0.7 it was fading, its strength and phase wandering as the ionosphere moved, and the subtraction follows it more closely (below) |
| **LDPC** | How many iterations the error correction took, how many of the 174 bits it repaired, and whether it needed the slower ordered-statistics search (OSD) |
| **Block, pass** | How many symbols were read together, and which subtraction pass found it (below) |
| **Heard** | How often since when, and its SNR over those times |

## How a cycle is read

When a cycle's fifteen seconds are in, the decoder looks for the sync
patterns across the band and at every start time, takes the best few hundred
places, and reads each: soft decisions over one, two or three symbols at a
time, then the LDPC code, then the 14-bit check. Every decoded signal is
then rebuilt and **subtracted**, and the band searched again, up to three
passes, so a weak signal under a strong one gets its turn. The rebuilt
signal follows the strong one's fading: on a path the card calls fading it
is followed closely, on a steady one smoothly, since close following also
takes a little of the neighbours with it.

Near the threshold the sync patterns alone place a signal a few
milliseconds and up to a hertz off, and that is where most weak decodes
used to be lost. So a candidate that looks like a signal and does not
decode is tried twice more: where all 79 symbols put the most energy, and
then where the tones chosen there fit best, coherently, over the whole
transmission.

The ordered-statistics search is powerful enough to "decode" noise into a
message whose check passes, which is how a decoder ends up reporting a
station in the middle of the Pacific that nobody sent. More often still it
misreads a real signal: a faded or half drowned transmission gives soft
bits that are partly wrong, and now and then a wrong message near them
passes the check. So a message found this way is held against what was
received: once its tones are known, sdrtop looks at every symbol and asks
how much of the signal's energy sits on a tone other than the one the
message says was sent. A true message differs only where the signal faded
into the noise; a wrong one is contradicted where the signal is plainly
there. It must also show its sync (more of it, the stronger the signal)
and a plausible number of bits repaired. One kind is taken at less: when
the search had nothing to try, because the most reliable bits gave the
message by themselves, more than half its sync is in place and next to
nothing is said against it, it is believed whatever it repaired. That is
a strong signal with a neighbour across some of its symbols, and such a
message was wrong 13 times in 2 100 where a searched one was wrong one
time in five. On simulated signals, weak and
strong, steady and fading, this lets through about one false message in
7 700 decodes, where testing the sync and the repairs alone let through
one in 900, and it believes 2.6 % more true ones. A station that was never
there costs more than a weak one missed.

One misreading says nothing against itself: a transmission read a little
off its own place, half a symbol early and a fraction of a tone high, where
every symbol holds two of its true tones and a wrong message can agree with
one of them each time. A much stronger neighbour that shares a tone with it
pulls the search there. Off the air on 20 m this put a station in
Antarctica on the map, 2 Hz from a Dutch one calling CQ, with a neighbour
8 dB stronger 39 Hz above. So a message the ordered-statistics search
finds in the tones of a transmission at least 6 dB stronger, decoded in
the same pass, is not believed yet: the stronger one is taken out first
and the place is read again. A true message is still there, usually
clearer; the misreading is not.

Some transmissions are whole in their sync and not in their message. An
operator who changes the message a second or two into a transmission sends
the start of one message and the rest of another; a station that starts
late or stops early stands over part of a neighbour. Those symbols are
plain and wrong, and they mislead the error correction more than noise
does. So a candidate whose sync is plainly there (14 of its 21 sync
symbols) and which still does not decode is read twice more, with its
first twelve data symbols taken as unknown and then its last twelve, and
the code fills them in from the rest. The card says `start unread` or
`end unread` on such a decode. The first one found off the air was a
station that had called CQ in the cycle before: its first ten symbols were
still that CQ, the rest answered a caller.

Measured on synthetic signals: half of them decode at about -21.2 dB SNR,
a little better than the figure the protocol's authors give for their own
decoder. On the same simulated cycles WSJT-X 3.0.2's command-line decoder
does about 0.2 dB better at its fast depth and 0.6 dB better at its normal
one; it has had a decade of tuning, and we are working on it. Off the air
it is closer than that: four minutes of a busy 20 m afternoon, the same
samples given to both, and of 254 messages sdrtop and WSJT-X at its deepest
setting both read 233, sdrtop alone 9 and WSJT-X alone 12 (leaving out the
ones WSJT-X guessed at from what a CQ usually says, which sdrtop does not
do). A crowded cycle of forty signals takes about three seconds on a modest
laptop, inside the fifteen it has: the candidates are read on every core
the machine has (up to four), with exactly the decodes one core would give.

**About SNR.** Once a message is decoded, sdrtop knows which tone every
symbol was sent on, and measures the signal's power there, half a symbol
at a time, against the noise in the tones that were not sent. Piece by
piece, not over the whole transmission at once, because on a real path the
signal fades and its phase wanders through the 12.64 s: added up as one
coherent sum, a fading signal reads several dB weaker than it arrived. An
early version did exactly that, and reported stations at -29.8 dB, which no
decoder can read (it is beyond what information theory allows FT8). On
simulated signals, fading by up to 2 Hz or not at all, the figure is now
within half a dB of the truth.

Off the air it still runs about 1 dB below WSJT-X's for the same signals.
The likeliest reason is where the noise is taken: sdrtop measures it right
beside each transmission, WSJT-X across the whole band, and in a busy band
that comes out a little kinder. Close, but not the same number, so compare
sdrtop's reports with sdrtop's.

## The clock

FT8 needs your clock right to within a second or so: a station's DT is
measured against your idea of when the cycle began. So the decodes judge it:
the median DT of the last few cycles' decodes is how late (or early) this
receiver stamps its samples against everybody else's transmissions.
`clock and path +0.05 s ±0.04 late, by 10 decodes` means about fifty
milliseconds late, give or take forty, judged by ten decodes.

It says **clock and path** because it measures both: this computer's clock
and the delay of the receive path (the radio, USB, buffering), and no
receiver can tell those apart. Under a second is fine and green; past a
second FT8 starts to lose decodes and it turns to a warning; past two it
stops altogether. Fewer than five decodes and it does not judge at all. If
it says you are late by seconds, check that your computer's clock is synced
before blaming the radio.

## The FT8 Lab

World shows who is out there. The Lab shows what the decoder made of them:
one transmission tone by tone, the band over the last hour, and your own
station as FT8 judges it. It is in the menu as **Lab › FT8**, under the
heading *Measure a mode*, or one key away: `L` goes from World to the Lab
and back, each time to the view you last left, with the same station
chosen. The Lab runs on World's decoder, so nothing starts over when you
cross. Three views, `1` `2` `3`:

| Key | View | The question it answers |
|-----|------|--------------------------|
| `1` | **Signal** | What did that one transmission actually look like? |
| `2` | **Band** | Where is the band open to, and is that changing? |
| `3` | **Receiver** | Can I trust my own station? |

World's keys mostly work here too: `↑` `↓` choose a station, `←` `→` step
the band, `o` writes the decodes, `c` forgets everything. The map's keys
stay on World, since the Lab has no map. Under 150 columns each view's side
column gives way, and the panels that are left take the room.

What none of it is, so nobody has to find out the hard way:

- **The SNR is sdrtop's**, in 2500 Hz, measured as [above](#how-a-cycle-is-read).
  Not dBm, not S units: the radio's samples are not calibrated to the
  antenna, and a number that looked like a power at the antenna would be
  one we made up.
- **The clock is the clock as heard**: this computer's and the receive
  path's together, as on [the strip](#the-clock).
- **No transmitter's frequency error.** Every station picks its own tone
  anywhere in the band, so there is nothing to compare it with, and your
  radio's own frequency error cannot be checked from FT8 either.

### Signal (`1`)

The chosen station's newest transmission, as the decoder saw it. Pick
another with `↑` `↓`.

- **The ladder**, across the top: the 79 symbols across, the eight tones up
  with a guard tone either side (where a neighbour or a dirty transmitter
  shows), each cell that tone's energy over the noise on the waterfall's
  colours. The tone the code sent is a dark line through it. Above it the
  three Costas arrays; below it `×` where a sync symbol's strongest tone was
  not its own, `▲` where the code repaired a bit, and each symbol's
  **margin**, its winning tone over the runner-up. The least of them is the
  closest call the decode survived.
- **The track**: tone 0 through the 12.64 s against the line the decode
  fitted, so drift shows as a slope. Each point carries its own spread, and
  a point too weak to place is not drawn but counted.
- **The power**, half a symbol at a time over the transmission's mean,
  coloured like the ladder: a fade through the transmission shows as
  itself, and the path is judged steady or fading.
- **The numbers**: everything on World's card, then every transmission of
  the same station the list still holds.

Once a station's decodes have left the list (the newest 500 stay), its
trace is gone too, and the view says so rather than drawing an older one.

### Band (`2`)

The last hour as this antenna heard it, or as much of the hour as the
session has; nothing is kept between sessions.

- **The reach**: time across, distance up on World's logarithmic scale, a
  dot where decodes fell, coloured by their SNR. An opening is a cloud
  climbing; the skip zone is the empty band under it. Only stations that
  sent a locator are placed, the rest are counted in the title. Under it,
  on the same clock, decodes a cycle (even above the line, odd below; a
  `×` on the line where no cycle was read at all, in the critical colour
  where the stream broke and the warning colour where the decoder was
  still busy; a quiet stretch and a deaf one are not the same thing), the
  noise under the band, and the farthest station of each quarter hour.
- **The noise under the band** is in dBFS of the radio's word per Hz, so it
  only compares with itself while nothing in the radio changes. Touch the
  attenuator or a gain stage and a `┊` marks where the scale changed; a
  cycle in which the radio turned its own gain down (its peak reached the
  ceiling) is a `×`, its level refused. It is a trend line for one
  evening, not a calibrated noise figure.
- **The slots**: the band in 50 Hz slots, how often each was busy over the
  last ten minutes, even and odd apart, and the quietest of each named.
  Quiet *here*, that is: a slot empty at your antenna can be wall to wall
  3 000 km away, which is the one thing about FT8 everyone learns by
  calling on top of somebody.
- **The bearings**: the hour's placed stations in sixteen directions, a bar
  for how many, coloured by their median best SNR. The busiest direction is
  named in the title from twenty stations up; under that it is a handful,
  not a direction.
- **SNR against distance**: each station at its best decode. No curve is
  fitted through it, because the dots carry no model to fit.
- **The hour in numbers**: stations each quarter hour, decodes a cycle, and
  the share calling CQ.

Distances and bearings need your position (Options › Station); without it
the panels say so and keep what needs none.

### Receiver (`3`)

Your own station, judged by the stations it hears.

- **Clock as heard**: the hour's DTs as a histogram from -1 to +1 s, the
  median marked, and the same verdict the strip gives, over the hour rather
  than the last few cycles. Decodes past a second are counted at the foot.
- **Sensitivity**: the hour's decodes by SNR, with the level where this
  decoder reads half of what is sent (-21.2 dB, measured on simulated
  cycles) drawn across it. A decode under the line is not a mistake: the
  line is where half get through, not where they all stop.
- **Decoder**: what became of the candidates over the cycles kept (in sync,
  read to a codeword, decoded, by which pass), and how hard the held decodes
  were to read. The share that needed the ordered-statistics search is said
  plainly, since that is where [false decodes](#how-a-cycle-is-read) come
  from; a band where a third need it is a band to read with an eyebrow
  raised.
- **Load**: each cycle's decode time as a small chart against the fifteen
  seconds it has, with the median and the slowest in words. A cycle skipped
  because the decoder was still busy is a full column in warning colour,
  one the stream broke in a `×`; under them, what the sample feed lost. On this machine, now: a measurement,
  not a benchmark.
- **The radio**: each part opens with a dot, filled when there is
  something to say and hollow when there is not. The peak against the
  radio's ceiling in the RF bench's own words and colours, and how many of the hour's cycles reached it; on a radio whose
  gain stages are digital (the HF+), a reminder that they place the level
  and do not set the SNR. Then the strongest signal anywhere in the capture
  outside the FT8 band, the thing that pushes a radio's own gain control
  while the decoder never sees it. Then the frequency correction in force,
  if any, and the timing bench's figures.

## Taking the decodes away

`o` writes every decode the view still holds (the newest 500) to
`ft8-YYYYMMDD-HHMMSS.csv`, in the same place and with the same header as
[the NET exports](net-export.md#where-they-go). The header says the band
and dial, and your locator and call, since the distances are measured from
you.

One row per decode, oldest first, whatever the list's filter:

| Column | What it is |
|--------|------------|
| `utc`, `cycle` | The cycle's start, and even or odd |
| `band`, `dial_hz` | Where the radio was tuned |
| `tone_hz`, `tone_sigma_hz` | Tone 0 above the dial, and its spread |
| `snr_db`, `dt_s`, `drift_hz` | As on the card |
| `coherence` | The path's, as on the card: under 0.7 it was fading |
| `sync_of_21`, `ldpc_iterations`, `bits_repaired`, `osd_order` | As on the card; `osd_order` blank where it was not needed |
| `block`, `pass` | As on the card |
| `message`, `from`, `to`, `for_me` | What was sent, by whom, to whom, and whether to you |
| `locator` | The locator this message carried, if any |
| `station_locator`, `lat`, `lon`, `km`, `bearing_deg` | The last locator the sender sent itself, as known when the file was written, its middle, and its distance and bearing from you |
| `clock_median_dt_s`, `clock_spread_s`, `clock_decodes` | The clock verdict the cycle was read with |
| `margin_min_db` | The closest call among the data symbols: the winning tone over the runner-up, dB, as the FT8 Lab's ladder shows it under "least" |
| `held`, `floor_db_hz` | Whether the radio held its own level inside the cycle (the peak reached its ceiling), and the noise under the band in dBFS of the radio's word per Hz. Blank floor when held, when a setting changed mid-cycle or when the cycle was not decoded: those levels compare with nothing |
| `unread` | `start` or `end` where the message was read with that stretch of the transmission taken as unknown (see [how a cycle is read](#how-a-cycle-is-read)); blank otherwise |

Anything not heard is left blank: a sender that never said where it is has
no place, without your position nobody has a distance, a cycle older than
the forty kept has no clock verdict on record, and one older than the hour
has no `held` and no floor. A blank is an answer; a zero would be a lie.

## What it does not do

No transmitting, and no audio. Only FT8: no FT4, no WSPR, no JT65. Every
message type the protocol defines is read; a message whose check passed but
whose fields mean nothing in the protocol is counted, never guessed at. The
Lab measures what it hears; it does not tell you whom to call.
