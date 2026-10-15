# Survey · `Channel 1` *(focus `u`)*

← [The NET section](net.md)

**Which Wi-Fi channel is busy, and which is quiet?** The question every
router app answers by counting networks, which is a bit like judging a
motorway by counting the car dealerships beside it. This view measures the
time instead.

A receiver of 20 Msps hears one Wi-Fi channel at a time, so the survey
visits channels 1 to 13 in turn, half a second on each (five beacon
intervals, so every access point in range speaks up at least once a
visit), and keeps every visit from the moment the view opens. A pass takes
about seven seconds. Locked on one channel, the survey keeps cutting that
channel's time into half-second visits, so its column stays live. Each
channel gets a column of two bars:

- **Busy** (the left bar, in the plain value colour): how much of the time
  the channel's busiest megahertz had anything in it at all. Wi-Fi,
  Bluetooth, a microwave oven, somebody's baby monitor: whatever a Wi-Fi
  station would hear and politely wait for. It is the same measure the
  [Bluetooth band survey](net-band.md#survey--band-2) makes, with its floor
  and threshold derived rather than guessed.
- **Frames** (the right bar, stacked by format): the share of the time the
  frames read on the channel's own centre said they would take, by format,
  in the theme's Wi-Fi colours (802.11a/g, n, ac, ax, be and b each have
  one, and every Wi-Fi view uses the same).

The two stand side by side rather than one inside the other because they
measure different things, and the second is not always smaller: an 802.11b
beacon can be read from its chips well below the level at which it counts
as energy. A bar sticking out of its own container would be a lie told
in good faith.

The scale is fixed at 0 to 40 %, so a column can be compared with the one
beside it and with itself a minute ago. The short tick above a busy bar is
one standard error of the visits' mean, drawn from the third visit on (two
visits have no spread worth drawing). Channels 1, 6 and 11 are labelled
bright: they are the three whose 20 MHz do not overlap. A channel the
survey has not reached yet is a dotted floor, not a zero.

## The card: least busy

The card names the least busy channel only when the visits can carry it:
the quietest channel, and every channel within two standard errors of it,
are tied. One channel is named when it stands alone, a short run when a
few do, and when all thirteen tie it says **not told apart yet**, and how
many more passes would set the first one apart at the spread the visits
have now. Wi-Fi traffic comes in bursts, so this is the usual answer for
the first minute or two; a survey that named a winner after six half
seconds would be guessing with a straight face.

Nothing is named before every channel has been visited, or before each has
the three visits a spread needs. And it never says which channel to use:
it knows how busy the air is, not what your devices can do.

**Network names** have their line at the foot of the card, waiting for the
MAC header to be read.

## Keys

| Key | What it does |
|-----|--------------|
| `u` | Focus the survey |
| `←` `→` | Select a channel: the card turns to it (busy with its spread, its frames by format, how many were read on its centre and heard there from other channels) |
| `L` | Lock the radio on the selected channel |
| `C` | Start the survey again: every visit dropped |
| `m` | Survey, or lock where you are, as everywhere in NET |

Locked, the locked channel's column stays live and every other is held at
its last pass in the stale colour, and the card is the locked channel's.
`Esc` leaves the focus and the card goes back to the survey's answer.

The channel's frames one by one, each with its format and signalling, and
the account of every preamble found are on `Channel 2`,
[the Frames view](net-wifi-frames.md).
