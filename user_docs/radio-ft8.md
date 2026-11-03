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
station in the middle of the Pacific that nobody sent. Here it is only
believed with the sync visibly there and a plausible number of bits
repaired. That costs a little sensitivity; a station that was never there
costs more.

Measured on synthetic signals: half of them decode at about -21.2 dB SNR,
a little better than the figure the protocol's authors give for their own
decoder. On the same simulated cycles WSJT-X 3.0.2's command-line decoder
does about 0.2 dB better at its fast depth and 0.6 dB better at its normal
one; it has had a decade of tuning, and we are working on it. Off the air
it is closer than that: four minutes of a busy 20 m afternoon, the same
samples given to both, and of 254 messages sdrtop and WSJT-X at its deepest
setting both read 233, sdrtop alone 9 and WSJT-X alone 12 (leaving out the
ones WSJT-X guessed at from what a CQ usually says, which sdrtop does not
do). A crowded cycle of forty signals takes about six seconds on a modest
laptop, inside the fifteen it has.

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

Anything not heard is left blank: a sender that never said where it is has
no place, without your position nobody has a distance, and a cycle older
than the forty kept has no clock verdict on record. A blank is an answer; a
zero would be a lie.

## What it does not do

No transmitting, and no audio. Only FT8: no FT4, no WSPR, no JT65. Every
message type the protocol defines is read; a message whose check passed but
whose fields mean nothing in the protocol is counted, never guessed at. A Lab page will
look at the signal itself: one transmission's 79 tones, the clock's spread,
SNR against distance.
