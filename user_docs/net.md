# The NET Section

← [Back](README.md)

**NET** is sdrtop's look at the 2.4 GHz band: who is on the air, what they
are saying, and how well their transmitters do it. It listens to Bluetooth
Low Energy advertising and to classic Bluetooth, and it measures the band
itself. It never transmits, never joins a connection (it follows the ones
it hears being set up, from the sidelines), and never plays audio: the guest
at the party who says nothing all evening and leaves knowing everyone's
address.

The section only appears on a radio that reaches the band and can sample fast
enough for its cheapest mode. On one that cannot, the section is hidden and
the log says why in one line.

It lives in three sections of the menu: **NET** for the band itself, **LE**
for Bluetooth Low Energy and **Classic** for classic Bluetooth. They come and
go together, because one radio requirement admits all three, and everything
on this page holds in all of them. Eight views, one question each; the key is
the number to press while that section is active:

| Key | View | The question it answers |
|-----|------|--------------------------|
| `NET 1` | **Capability** | What can this radio reach and receive here? |
| `NET 2` | **Survey** | What is in this band, and who is spending the airtime? |
| `LE 1` | **Census** | Who is here? |
| `LE 2` | **Advertising** | What is this device advertising, and is its transmitter any good? |
| `LE 3` | **Connection** | What are two connected devices saying to each other, event by event? |
| `Classic 1` | **Piconets** | Who is running a classic Bluetooth piconet near me? |
| `Classic 2` | **Packets** | What is this one piconet saying, packet by packet? |
| `Classic 3` | **Bench** | How good is each end of it, side by side? |

The panels with controls announce them with a highlighted letter in the
title; the full list is in [Keyboard Shortcuts](keys.md#net-panel-focus-modes).
The menu shows a live line under every NET view, built by the same rules the
view itself uses, so it never promises more than the screen delivers.

---

## Reading any NET screen

A few things are the same on every NET panel, and they are the difference
between a number and a claim you can trust.

### SAT: when the radio is shouting

When `SAT 4.4 %` appears first on the header's band line, that share of the
samples is pinned at the converter's limit: the same reading and colours as
every SAT in sdrtop, amber from 1 %, red from 5 %, and absent below 1 %.
A clipped packet is a distorted one. It fails its CRC while its SNR still
looks fine, so while SAT shows, the failures say more about the radio than
about the device. Lower the LNA or VGA, or move the radio further from the
transmitter. A BLE remote held next to a HackRF at LNA 32 does exactly
this, and is very sure of itself about it.

A clip lasts a fraction of a second and you may look a moment later, so
once the reading falls back below 1 %, a clip that reached 5 % leaves
`⚠ last clip 3s` in its place: red for six seconds, then grey, then gone
after half a minute. It is the same line the Command Rail keeps under its
SAT, drawn by the same code, so the two never disagree about when the
radio last shouted.

### Survey or lock (`m`)

The radio can see a slice of the band at a time, not all 83 MHz of it.
**SURVEY** steps the slice across the band and spends a fraction of the time
at each position; **LOCK** parks on one position and watches it without a
gap. `m` switches between the two, and every panel carries `[SURVEY]` or
`[LOCK]` in its title so a number is always read with how it was gathered.

A surveyed count is a sample of the band, not a complete record. A few
readings only exist in LOCK: a BLE device's advertising interval, for one,
needs arrivals that were never interrupted by a hop.

What SURVEY walks depends on the view. The BLE list and the Census are fed
by the advertising decoder, so there it rotates the three advertising
channels and nothing else. Everywhere else it walks the whole band, and the
BLE decoder takes whichever advertising channel is in view at each stop,
not just the channel the stop happens to be centred on. (It used to take
the latter, which at 8 Msps meant channel 37 was never heard in a survey
at all: the band's layout and Bluetooth's had simply never been
introduced.)

### Stepping while locked

`←` and `→` move a locked radio without typing a frequency. On the BLE and
Census views a step is the next advertising channel, 37, 38, 39 and round
again, because that is the only place advertising happens and anywhere in
between is an expensive way to hear nothing. On the other views it is one
block of the band along: the span, or on the classic views the most channels
it watches at once (`[net].bt_channels`) if that is fewer, wrapping round at
the ends. Surveying, the survey owns the tuning, and the keys
just remind you that `m` locks.

A lock carries across views, so a Survey locked on a Wi-Fi channel would
arrive at the Advertising view parked where no advertising ever comes. Opening an
advertising view off the three channels therefore moves the radio to the
nearest of them, once, and the log says so. Tune somewhere else afterwards
(a data channel, to catch secondary advertising, say) and it stays where you
put it.

### The header

In NET the strip under the radio's name is the band, 2400 to 2483 MHz, and
the lit stretch is what the radio can see right now: the tuning, give or
take half the span. The classic channels being watched are drawn over it in
white, and the channel the BLE decoder is on is a pink `●`, the same marks
and inks the coexistence history puts on their hits. In SURVEY the lit
stretch walks the band with the survey.

The line under it says the same in words: `● BLE 38 adv` or `● BLE 3 data`
(a data channel is where advertising never comes, which is the usual reason
for a quiet list), `■ BT 5–11` for the classic channels, and with no decoder
running, the Wi-Fi channel number, because that is how everyone else reads
2.4 GHz.

### The three silences

An empty panel always says which kind of empty it is:

- **Refused, and why.** The receiver cannot run here, for a stated reason:
  the view holds no Bluetooth channel, the sample rate is too low.
- **Listening, nothing heard yet.** The receiver runs; the room has been
  quiet, or not quiet for long.
- **Not listening.** Nothing is running for this panel right now: RX is
  stopped, or the view that feeds it is not open.

sdrtop never shows a bare empty table or a zero that means "we did not look".

### Stale, and feed loss

Every panel turns **[STALE]** when RX stops. **[FEED LOSS]** means the sample
feed dropped blocks inside the stretch of time that panel's numbers cover: its
counts are lower bounds from then on. The header's `decode` figure is how much
of real time the decoders need; over 100 % they cannot keep up and blocks are
dropped, which the `gaps` count and the feed-loss tags then show.

### Addresses (`i`)

`i` switches how addresses are shown, on every panel and in every export at
once:

- **full**: the whole address, `d1:9a:7e:91:27:9e`.
- **oui**: who it belongs to and enough of the rest to tell two apart: the
  IEEE registrant for a public address (`Apple ..09:be`), or the address's
  kind for a random one (`RPA ..4e:12`). Where a random address's
  manufacturer data names a company, the company takes the kind's place,
  marked with where it came from: `Apple·mfr ..09:be`. "Company" is whose
  data format it is, not who made the device.
- **masked**: the same "who", and a session number instead of any part of
  the address (`Apple #17`). The numbers are handed out in the order
  addresses are heard and are not derived from them, so nothing in a
  screenshot can be turned back into an address.

The masked mode covers everything that would give a device away, not just
its address. An advertised name shows only its length (`name, 16 chars`),
because a perfectly masked `#17` sitting next to "Viktor's AirPods" would
have protected nobody. Manufacturer and service data show their size
(`23 bytes`) beside the company or service they belong to, since some of
those payloads carry an identifier of their own. A classic piconet's LAP
becomes its place in the roster (`#3`, the same number on the hop lanes and
in the export), and a resolved UAP just says `found`. Inquiry codes stay
named (`GIAC`), because they are nobody's address. The **oui** mode still
shows names, LAPs and bytes: it is for telling devices apart, and masked is
for letting other people look.

The registrant and company names come from dated snapshots of the IEEE's and
the Bluetooth SIG's registries. Whenever the mode is not **full**, the title
of every panel showing addresses says so and gives both dates.

### What an offset is worth

Every frequency offset and ppm figure contains our own oscillator's error.
This is humbling, and it is also why the panels that show one carry a tag
that says what it is worth:

- **[RELATIVE]**: measured against our own oscillator, uncorrected. Good for
  comparing devices with each other, not for saying how far off one is.
- **[REFERENCED]**: corrected by a device you told sdrtop to trust (`T` in
  the Census, with how far its crystal can be off, in ppm). It rests on your
  word, and every panel and export names it as "user-stated".
- **[TRACEABLE]**: corrected against a standard station. Tune to one (WWV at
  2.5, 5, 10, 15 or 20 MHz) and press `y`; sdrtop measures its carrier and
  takes our error out of every reading at once.

A reference expires after fifteen minutes, because oscillators drift, and the
tag then says so. A trusted device you regret trusting does not have to wait
that long: `T` on it again lets it go, and every tag reads **[RELATIVE]** at
once. `T` on a different device replaces it. A station reference just
expires, or a fresh `y` renews it.

### Reasoned, not verified

Some of what NET measures has been checked against a real transmitter and
some has only been written from the specification and tested on synthetic
signals. Where it matters, the screen says which:

- A limit **read from the Bluetooth Core Specification** names its section,
  for example `Core 5.4 Vol 2 A 3.1.1` beside the classic modulation rows.
- BLE's limits are all read from the Core Specification: the modulation
  index and the deviations from `Core 5.4 Vol 6 A 3.1`, the drift and the
  drift rate from `3.3`, on LE 1M and LE 2M alike. The section heading
  names both.
- The classic Bluetooth header decode is a port of `libbtbb`, and its
  section is headed "libbtbb port, checked on air". For a long time it said
  "unchecked": it had passed every test I could write and never met a real
  transmitter. Then it met two, a phone playing music to a pair of
  headphones, with both addresses read off the devices themselves. The
  LAPs matched, and the UAPs narrowed to two candidates within half a
  minute. Then a reconnect's DM1 packets, the few link-manager messages
  sent before the link encrypts, passed their CRC and settled both, to
  exactly the devices' own. Their contents made sense too, down to the
  encryption request right before the CRCs stopped passing. Checked on the
  air: the header decode, the UAP narrowing and the DM1 payload. Not yet:
  DH payloads and DM3/DM5, because none arrived unencrypted and in basic
  rate.
- **An encrypted link stays at `2 left`**, and that is not a fault: the
  payload is encrypted, so no CRC can be checked without the key, and the
  true UAP is always one of the two. It resolves from the few unencrypted
  packets a connection sends while it is being set up, so switching the
  headphones off and on while sdrtop listens is the quickest way to a
  value.
- **A header's packet type shows every packet its code can be**, like
  `DM3/2-DH3`: the same code means one packet on a basic-rate link and
  another once the link has switched to EDR, which most audio links do,
  and the header alone cannot say which. Your headphones' music is the
  second name. The export's `packet_type` column says the same.
- **"predicted, not followed"** beside a `CONNECT_IND` in the packet detail
  means the channels there were worked out from its parameters. The
  connection itself is followed on [LE 3](#connection--le-3-focus-e), through
  the events whose channels the radio's window holds, and only those.

---

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
the BLE decode funnel (triggers, and how each one ended). The funnel is also
one bar, good, CRC failed and gave up in their shares, and the decode load
is a bar against a rule at 100 %, red past it. Every count on the other NET
panels is only as complete as this panel says the feed was.

---

## Census · `LE 1` *(focus `u`)*

One row per transmitter the BLE decoder has confirmed: an address is only
counted from a packet whose CRC passed, because a corrupted address would be
a device that does not exist. Surveying, the Census rotates the three
advertising channels itself, so it fills on its own, with no need to have
the BLE list open beside it.

| Column | What it is |
|--------|------------|
| ADDRESS / KIND | the address as the `i` mode shows it, and its kind (public, static, RPA, ...) |
| SEEN | how long ago it was last heard |
| PKTS | packets from it |
| BEST SNR / MEAN SNR | the strongest it has been heard, and the mean with its uncertainty. An SNR, not an RSSI |
| CRC | the share of its packets that passed. A ceiling: a packet whose address was corrupted is nobody's failure |
| CFO | its carrier offset in ppm, corrected as the offset tag says |
| TYPES | how many advertising PDU types it has sent |
| MOD | its modulation index, refined over every packet that allowed one |
| INTERVAL | its advertising interval, from arrivals in LOCK only, with whether it sits on the 0.625 ms grid |

`S` sorts by the next column, `R` reverses, and the title says what orders
the table. `T` trusts the selected device as the frequency reference: you are
asked how far its crystal can be off, and every offset in the app is then
**[REFERENCED]** against it.

Under the table every device's clock error sits on a null meter, worst first.
Select a device and it gets a dial of its own and a detail block: when it was
first and last heard, what it advertised (its name, TX power and company, as
it said them), its readings and the PDU types it sent. On a wide terminal
the detail sits beside the dial; on a narrower one it goes under it, and on
a short one the dial steps aside for a single meter rather than squeezing
the table.

---

## Advertising · `LE 2`

Real BLE advertising packets, CRC-checked, as they arrive. The radio has to be
on an advertising channel (2402, 2426 or 2480 MHz); the header says which
channel the decoder has, and whether it is one ([the header](#the-header)).

### The packet list *(focus `v`)*

Newest first: channel, type, address, the advertised name, address type,
length, CRC, SNR, carrier offset and age. A name is only read from a packet
whose CRC passed, and is made safe to print: a control character a device
puts in its name shows as a replacement mark, never as a command to your
terminal.

- `Enter` narrows the list to the selected packet's address, and back.
  **[FILTERED]** says the list is not everything. Selecting a device in the
  Census and switching here narrows the list to it for you.
- `t` narrows it to one kind of packet, then the next, then every kind
  again: **CONNECT** (a connection being set up), **SCAN** (`SCAN_REQ` and
  `SCAN_RSP`) or **ADV** (the advertising that is nearly everything else).
  The frame names the kind, and it works together with `Enter`, so one
  device's CONNECTs are two keys away. The list keeps the newest 200 of
  each kind, not 200 overall, so a lone CONNECT_IND is still there after
  the advertising flood that followed it.
- `H` holds the list still; the title counts what has arrived since.

The list hears **LE 1M**, and the frame says so. There used to be a key for
LE 2M, and it has gone on purpose. The decoder listens for the advertising
access address, so parked on a data channel it hears secondary advertising
and never a connection, whose packets carry an address of their own. LE 2M
lives almost entirely in connections, and never on the three advertising
channels, so the key was either refused or listened very carefully to
nothing. LE 2M is back where it lives, in connections: a connection
followed on [LE 3](#connection--le-3-focus-e) that moves to LE 2M is
received on it from the moment the two devices agreed, by itself, as it
should be.

In SURVEY the line under the list gives each advertising channel's packet
count and CRC pass rate; in LOCK, the one channel's.

### Packet detail

The selected packet, spelled out:

- **Packet**: type, channel, length, PHY, CRC, addresses, the ChSel bit.
- **Advertised**: every structure it carried: flags, name, TX power, service
  UUIDs, service data, manufacturer data with the company named, and
  anything malformed at the octet where it stopped making sense.
- **Connection**, for a `CONNECT_IND`: the parameters the two devices agreed
  (interval, latency, timeout, channel map, sleep-clock accuracy) and the
  first channels the connection will hop to, predicted from them with
  Channel Selection Algorithm #1 or #2, **predicted, not followed**. Which
  of the two takes both ChSel bits, this packet's and the advertising
  packet's it answered; with that one not heard, the detail says it does not
  know rather than pick. A connection being followed says so here. `Enter`
  on a `CONNECT_IND` in the list opens the connection itself on LE 3.
- **Physics**: SNR, carrier offset in kHz and ppm, and the carrier at the
  start and end of the packet: the start is the preamble's mean frequency
  (the test suite's f0), the end the last ten bits before the CRC.
- **Modulation**: the transmitter's modulation index, deviation and drift
  against the specification's limits, each drawn as a bar with the limit
  marked. Read from the packet's own symbols; a packet whose CRC failed is
  not measured, because which symbols were ones decides where deviation is
  read. `df2 avg` is the average deviation the packet reached while
  alternating, held against the specification's 185 kHz floor (370 on LE
  2M). The floor is on the *minimum*, and both obvious ways of reading one
  from live traffic measured the noise instead: the largest reading could
  never fail, and the smallest nearly always did. A transmitter's
  alternating peaks are all the same peak, so the average stands in for
  them, which means an average under the floor is a real finding and one
  over it is not a promise. Both are read as the test suite defines them,
  from whatever bits the packet carried, the way a classic piconet's are
  (see [the bench](#bench--classic-3-focus-c) below); on LE 1M the packet is taken again
  from the raw samples and timed from the access address, on LE 2M read
  through the receiver's own filter.

  The **drift** rows are the test suite's too: the carrier is read over
  every ten bits of the PDU, and `drift` is the block furthest from the
  start, `drift rate` the steepest change over five blocks (50 µs on
  LE 1M). The suite sends a `1010` for this, whose ten bits average to the
  carrier; ordinary traffic does not balance like that, so each bit's own
  modulation, as the packet itself shows it, comes out first. Both are
  maxima, and a maximum of noisy blocks finds the noise too: with nothing
  drifting at all, noise alone reads about 1, 3 and 9 kHz of drift and 20,
  50 and 150 Hz/µs of rate at 40, 30 and 20 dB above the noise. The `±`
  says how noisy the blocks were, and past the panel's resolution the row
  shows a dash rather than a drift the transmitter may not have. On LE 2M
  the preamble is not kept, so the first block stands in for the start.

With **no packet selected**, the detail shows the session's **frame error
rate against SNR**: for each 2 dB of SNR, what share of packets failed their
CRC, with its uncertainty. A bin with fewer than ten packets gives its count
and no rate. Filtered to one address, it is that device's own curve. A
receiver whose failures do not fall as SNR rises is failing for a reason that
is not noise, and this curve is where that shows.

---

## Connection · `LE 3` *(focus `e`)*

Every connection whose `CONNECT_IND` sdrtop hears is followed, and this
view shows one of them: the one opened with `Enter` on its `CONNECT_IND` in
the Advertising list, or the newest. `← →` step through the others.

**Without retuning.** The radio stays where it is and listens to the band it
already has: at 20 Msps on 2426 MHz that is advertising channel 38 and data
channels 7 to 14, which the top line counts (`8 in view (7-14)`). The
`CONNECT_IND` says when the connection's events will be and which channel
each will hop to (Core 5.4 Vol 6 Part B 4.5.3, 4.5.8), so sdrtop listens to
the events that land in its window and writes down the rest as out of view.
With all 37 channels in use that is about one event in five. It is like
following a conversation through a wall that lets every fifth sentence
through: you learn who talks, how fast and in what language, rather less
of the gossip.

**Catching one.** The `CONNECT_IND` has to be heard, on the advertising
channel in view; a device that connects picks whichever channel it last
heard the other on, so it can take a few reconnects. A connection already
running when you started listening cannot be picked up halfway, yet.

**The top lines** are the connection's parameters now in force (the
`CONNECT_IND`'s, with every update since), the two addresses (masked with
`i` like every other), when it was set up, the PHY each way, and whether
it is encrypted.

**Every event has one of four accounts:**

- **followed**: a packet with the connection's access address was heard;
- **missed**: its channel was in view and nothing was heard. Not a claim
  about why: a Peripheral may skip events, and a Central may have nothing
  to send;
- **not in view**: its channel is outside the band; runs of these fold into
  one row (`36-33  not in view (4)`), so the events heard lead the list;
- **feed lost**: the samples were not there to listen to.

**Who sent it**: the Central opens each event at its anchor point and the two
take turns 150 µs apart (4.5.1, 4.1.1), so the first packet at the anchor
is the Central's (`C→P`, in the connection's colour) and its answer the
Peripheral's (`P→C`). A packet neither rule accounts for gets a dot.

**What it said.** Link-layer control PDUs are named from Table 2.20 and their
parameters read in words: versions, features, the PHY requests, channel
maps, connection updates, terminate reasons. Each is read only at the length
its own table entry gives, so an encrypted packet is not mistaken for one.
The keys and random numbers of encryption setup are named, never printed.
From `LL_START_ENC_REQ` on, the link is encrypted and only the length and
the CRC are left. L2CAP payloads are not read at all.

**Changes the two agree on** take effect at an *instant*, an event counter
named in the PDU: a new channel map, a move to LE 2M, a new interval. sdrtop
applies each it hears. One sent while its channel was out of view cannot be
heard, and the connection then goes quiet in the window: after the
supervision timeout with its events in view still silent, it is marked
**lost after event N**, not followed on a stale schedule. `LL_TERMINATE_IND`
ends it with its reason; a move to LE Coded, or a subrate change, is
**not followed**, and says so.

### Measured

- **clock**: the Central's clock against this radio's, in ppm, from a line
  through the anchors heard. Without a frequency reference it is relative,
  and says so; with one, it is held against the sleep clock accuracy the
  Central declared in its `CONNECT_IND` (inside, at the edge, outside).
- **T_IFS**: the turns heard, pooled, against 150 ± 2 µs (4.2.1).
- **CRC**: packets and passes per data channel heard.

**On the air**, a TV box and its BLE remote, the remote taken out of its
batteries and put back: the box's `CONNECT_IND` set ChSel, the remote's
`ADV_DIRECT_IND` did not, so the link hopped by Algorithm #1, as the Core
says it must when either bit is 0 (the view reads both). Every event in view
was followed, both ends placed: `LL_FEATURE_REQ`, then `LL_ENC_REQ` and
`LL_START_ENC_REQ` by event 10, after which the link was encrypted; T_IFS
150.31 ±0.04 µs over 26 turns, the box's clock −5.1 ±0.1 ppm against the
radio's. At event 138 the two changed their timing in a PDU nobody else
could read, and the view said so: lost after event 138. A device that
reconnects over classic Bluetooth (most earbuds) never shows here at all.
Keys: `↑↓` scroll, `End` back to the newest, `← →` the previous or next
connection.

---

## Piconets · `Classic 1`

Classic Bluetooth hops across 79 one-megahertz channels, 1600 times a second,
in a sequence a passive listener does not know in advance. What can be found
without joining is the **access code** at the start of every packet, which
carries the piconet master's LAP (the lower 24 bits of its address). sdrtop
watches as many channels as the view holds, up to `[net].bt_channels` (eight
by default, see [Configuration](config.md)), and takes only exact matches, so
a LAP on screen was sent.

A LAP names a **piconet**, not a device: every member of the piconet sends its
master's access code. The panels say piconet throughout.

With one exception, and it is usually the busiest row. The specification
keeps 64 LAPs, 0x9E8B00 to 0x9E8B3F, for **inquiry**: a device looking for
others sends one of them, and so does every other device looking. The
roster and the lanes name them by their own abbreviations, `GIAC` (general
inquiry, 0x9E8B33, the one you will actually see), `LIAC` (limited, 0x9E8B00)
and `DIAC` (the dedicated rest). A GIAC row hopping over dozens of channels
is not a very chatty piconet; it is your phone asking the room who is there.
Its UAP is fixed at `DCI` (0x00) by the specification rather than worked
out, and it gets no modulation, timing or header sections: those would be
every searching device's at once, averaged into one confident fiction.

The other row that is not a piconet is a **page**: one device calling
another to connect. The caller sends the *called* device's own access code
as short ID packets, with no header, in trains that hop 3200 times a second,
so the spacing between two of its hits is often an odd number of half slots
(312.5 µs, 937.5 µs, ...). A piconet's packets start on whole slots and
never do that. When a LAP has had no header after any of at least 16 hits
*and* its spacings include odd half slots beyond what chance gives, the
roster calls it `paged`: that LAP belongs to the device being called, not to
anyone's piconet. With either sign missing it stays a plain piconet row; a
page that was answered carries headers and is missed rather than named
wrongly. The `pace` line in the roster's detail shows the counts either way,
and the Classic export's `lap_kind` says `paged` too.

### Hops *(focus `b`)*

Two answers:

- **WHERE**: all 79 channels, with bars of each channel's hits this session,
  in the colour of the piconet heard most there, and the channels watched
  now bracketed underneath (`╰──╯`, or `▲` for a single one). A piconet heard at an earlier survey position keeps its
  place.
- **WHEN**: one lane per piconet, a tick at each hit's time. `+` / `-` zoom
  from half a second to a minute, `←` `→` move back and forward in time,
  `End` returns to now. If the window reaches further back than the hits
  sdrtop keeps, it says "older hits not kept" rather than drawing quiet lanes.

Each piconet wears one colour here and in the roster, and one selection
(`↑` `↓` in either panel) drives both.

### Piconets *(focus `c`)*

One row per LAP: when it was heard, hits, channels, and its **UAP**, the next
8 bits of the master's address. The UAP is not sent. Bluetooth keeps it to
itself, and it has to be worked out from the headers that follow the access
code, a little like a crossword where every clue has two answers: a first header leaves 32 candidates,
more headers bring it down to two, and a data packet's own payload CRC
settles it to one: DH1, DH3 and DH5 as sent, DM1, DM3 and DM5 through their
error-correcting code first. The same payload also settles which of the
piconet's possible clocks the header was sent at, and from then on its
headers are read at that clock and no other. A header that could still be
read two ways is left unread rather than read the likelier way. The column
shows `32 left`, `2 left` or the value.

The selected piconet is spelled out under the roster, as a whole: the rule
of the three classic views is that the piconet as one thing is here, its
packets on Classic 2 and each of its two ends on Classic 3.

- **PICONET**: its LAP, its **UAP** and what that rests on ("resolved by a
  payload CRC" and how many CRCs pass under it, or the candidates left, two
  being where an encrypted link stays, see [Reasoned, not
  verified](#reasoned-not-verified)), when it was heard and on which of the
  79 channels, and its slot **clock**. The clock comes from the piconet's
  625 µs slot grid, fitted to every member's hits: slots that run long on
  our clock mean a master whose crystal runs slow. Without a frequency
  reference it is shown relative to our own oscillator, like every other
  ppm here; with one it is corrected and said to be inside, at the edge of
  or outside the specification's 20 ppm. This assumes the radio's tuner and
  its sample clock share one crystal, which is true of a HackRF and an
  RTL-SDR. The Classic export carries it as `clock_ppm`. Beside it, the
  grid's rms: every member's hits together, so it is not a jitter (the two
  ends' offset from each other is in it; Classic 3 has each end's). Last, one
  line of pooled readings, the modulation `index` and the carrier `f0`,
  every member's.
- **HEADERS**: once the UAP is one value, what the piconet's headers say
  over the session: how many were read of those captured, how many did not
  decode under the resolved UAP (a rising count is how a wrong UAP would
  show), the packet types, the logical transport addresses in use (`0` is
  broadcast), and how the hunt for the clock stands. Before that, nothing
  is read or guessed.


`Enter` opens [the Packets view](#packets--classic-2) on the selected piconet,
and `3` [the Bench](#bench--classic-3-focus-c). An inquiry code or a page is not a
piconet either could open, so their whole account stays here. A short panel
keeps what fits whole and names the rest.

---

## Packets · `Classic 2`

One piconet, packet by packet: its packet list, the whole screen. If the
Piconets view is the room, this is one conversation in it: overheard, timed
and measured, and still not understood, which is the polite way round. It
is the piconet selected in the Piconets view: `Enter` on the roster brings it
here, and so does `2` with a piconet selected. `← →`, with the list
focused, step to the previous or next piconet in the roster's order. With
none heard, or none selected, the list says which of those it is.

The classic receiver runs here exactly as on the Piconets view, same channels,
same `← →` steps for a locked radio when nothing is focused, so the three
classic views are three readings of one stream of hits.

### Who sent it

The master starts every transmission in an even slot and the slave in an odd
one, both counted on the master's clock (Core 5.4 Vol 2 Part B 2.2.5, read
from the SIG's own copy). So once a header has been read at one clock (the
UAP resolved, the clock pinned, see [Piconets](#piconets-focus-c)), the
clock's lowest bit says who sent it: `M ▶` for the master, `◀ S` for the
slave. Before that, and for an ID packet with no header at all, the row gets
a dot. Never a guess: a packet whose sender is not known is counted apart
from both, as **not yet placed**.

The master wears the piconet's own colour, the one its chip has on the hop
chart and in the roster; the slave wears the ordinary ink, which no
piconet's colour ever is, so the two ends never look alike.

### The packet list *(focus `v`)*

Newest first, one row a packet, the last 1000 of this piconet kept:

| Column | What it is |
|--------|------------|
| `AGE` | How long ago the block it came in was read; to a tenth of a second, because that is the block's time, not the packet's |
| `CH` | The classic channel it was heard on |
| `DIR` | Who sent it, as above |
| `TYPE` | The header's packet type, in every reading its code allows (`DM3/2-DH3`, see [Reasoned, not verified](#reasoned-not-verified)); `ID` for an access code alone; `(no UAP)` or `(no clock)` for a header captured and not yet readable |
| `LT` | The logical transport address |
| `F A S` | The FLOW, ARQN and SEQN bits |
| `CLK` | CLK1-6, the clock the header was read at: even the master's, odd the slave's |
| `ΔSLOT` | Slots since the packet before it on the same stream |
| `SLOT µs` | How far it landed from the piconet's slot grid |
| `MOD` | Its own modulation index |
| `f0 kHz` | Its own initial carrier, relative to our oscillator until a reference makes it absolute |
| `PAYLOAD` | What became of its payload, in the words below |

`ΔSLOT` is the link's rhythm: a master and slave taking turns read
`+1 +1 +1`, a three-slot packet is followed by `+3`, and a quiet spell shows
as a jump. After a stream restart (a rate change, `Space` off and on) it is
blank, and so is `SLOT µs` for a packet from before the grid was fitted:
times from another run of the stream are on another clock.

`MOD` and `f0` are single readings from one header, noisier than [the
bench's](#bench--classic-3-focus-c)
and printed to the places their own uncertainty allows, a dash where it
allows none. A value outside its limit turns amber or red, as on the bench;
everything inside keeps the ordinary ink, so a problem is the thing that
stands out.

The payload column, word for word:

- **a dash**: nothing to check. POLL, NULL and ID carry no payload.
- **`✓ CRC`**: read, and its CRC passed.
- **`✗ CRC`**: read, and its CRC failed. It does not say why. An encrypted
  payload fails exactly like a damaged one, and the air alone cannot tell
  them apart, so the screen does not pretend to.
- **`· FEC failed`**: a DM payload whose error-correcting code could not be
  undone. A damaged capture does that, and so does a payload that was never
  basic rate: an EDR packet read as its basic-rate twin (the `2-DH3` of a
  `DM3/2-DH3`) comes out here or as `✗ CRC`, and the packet alone cannot say
  which it was.
- **`· cut short`**: the capture ended before the packet did, at the edge of
  a block or a block the feed lost.
- **`· type not read`**: a type sdrtop has no payload reader for (voice
  packets, FHS).
- **`· bad length`**: a payload header whose length cannot hold a CRC.
- **`· clock not known`**: the header was not read at one clock, so there is
  no whitening to undo.

`↓` scrolls into the past, holding the list at its newest packet first so
the rows do not slide away while you read them; `↑` goes back up, `H` holds
or lets it run, `End` is live again. Held, the title says what the pause is
costing (`[+37 NEW]`). The last line counts the session's packets by end,
and the ones not yet placed.

---

## Bench · `Classic 3` *(focus `c`)*

The same piconet, each end of it side by side: three columns, MODULATION,
CARRIER and TIMING, every reading two rows, `▶` the master's in the
piconet's colour and `◀` the slave's in the ordinary ink, each with its bar
on the same row, held against the limit the column's heading cites. The
heading also says what the readings rest on (`258 hdr`, `1204 hits`). A side
with nothing yet says so in words, never a zero. `3` opens it on the
piconet selected in the Piconets view; `← →`, with the bench focused, step
through the piconets.

- **Modulation**: each end's BR modulation index and deviation against
  0.28 to 0.35, read from the Core Specification, from every header's
  symbols. Read the way the SIG's test suite defines them, which took some
  doing. A tester commands the device to send `00001111` and `10101010`
  and reads particular bits of them; sdrtop cannot ask a stranger's
  headphones for anything, so it reads every bit whose two neighbours make
  it one of those bits (same either side, or opposite either side), which
  for this kind of modulation is the same measurement, and a test holds it
  to the suite's own figures. Each header is taken again from the raw
  samples and read at bit centres timed from the packet's own sync word.
  The receiver that finds the packets guesses where a bit's centre is,
  which is fine for finding packets and was not fine for measuring them:
  until this was built it read a perfectly healthy transmitter's
  `df2/df1` as about 0.45, which is how this whole section learned to
  check itself against a reference first. The suite's own recommended
  filter turned out to be the wrong one for traffic (it is built for a
  tester reading its own test patterns, and bends ordinary traffic by a
  few percent), so the reading uses a wider one. The price is the next
  channel: up to 20 dB below the transmitter being measured it moves the
  figures by less than 1 %, and a header with anything louder next door at
  the same moment is not read at all, just counted ("headers not read: the
  next channel was busy"). A weak signal makes `df2` scatter, one reading
  per bit at its centre carrying all the noise; the `±` beside it says how
  much, and at 20 dB above the noise it also leans a percent or so, which
  the `±` does not cover. Stronger is better, as with most things in
  radio.
- **Carrier**: each end's initial carrier (f0, from the four preamble bits)
  and its drift, read as the test suite defines them from the access code
  and header of every header measured. The drift and its rate are the worst
  header's, since the limits are on every packet; f0 is the mean, relative
  to our own oscillator like the clock below, and held against the
  specification's ±75 kHz only once a reference makes it absolute. The
  drift is held to the 40 kHz every packet type has to meet: a one-slot
  packet's 25 kHz is over its whole length, which a header alone cannot
  show. Every bit read here is either known (the whole access code) or
  decided again from the measurement itself, never taken from the part of
  the receiver that finds packets, because one wrong bit there looked like
  30 kHz of drift. Noise makes drift too, as it does on BLE: see the
  Modulation note in the Advertising view's detail.
- **Timing**: each end's timing on the piconet's 625 µs slot grid, fitted to
  every member's hits. Below eight hits it is collecting; hits that do not
  line up on a grid beyond chance are refused as one, never forced onto it.
  Given enough periods to try, a dozen points will line up with almost
  anything, and the panel would rather say "no grid" than find one it
  wanted to find. (The piconet's clock from the same grid is the whole
  piconet's, so it is on Classic 1.)

  Each end's **jitter** is its own packets' scatter about their own average
  timing, which is how 2.2.5 states it, held against its 1 µs. The grid
  belongs to neither end, so where each end sits on it means little alone;
  how far the slave's packets sit from the master's (`offset`) is the
  difference of the two averages, and that the grid cannot move. Under it,
  the residuals as a shape, the ±1 µs limits ruled, **stacked by who sent
  them**: the master's in the piconet's colour, the slave's in the ordinary
  ink, and the packets of unknown sender in the stale grey on top, never
  put on a side. One hump is one timing; a slave answering a little late on
  every slot stands as a hump of its own colour.

Under MODULATION the index and under CARRIER f0 are **plotted** over the
last minute: both ends on one time axis and one scale, each in its colour,
so whether the two read alike and whether either is moving shows at a
glance. Each point is the mean of a few packets (the title says how many),
because one header's reading is noisy and a line of single readings is a
band that shows the noise and hides the drift. The scale's ends are written
on the left: a line that fills the plot across `0.333` to `0.335` is a
transmitter holding steady, one across `0.30` to `0.34` is warming up. A
slowly moving f0 in the minutes after a device switches on is exactly that.

Narrower than three columns, the bench draws as many sections as fit, in
order, and names the rest ("+ TIMING on a wider panel"). Shorter than its
sections, a column shrinks its plots first (six rows, then three, then
none), keeps its readings whole, and says the plots are on a taller panel;
only then do readings give way from the bottom, and it says that too.

---

## Taking the data away (`o`)

`o` anywhere in NET writes six files, one for each record the section keeps:

| File | One row per |
|------|-------------|
| `net-band-*.csv` | megahertz of the band: duty, its uncertainty, coverage, power (from NET 2 only: elsewhere the band is not being measured, and the file says so instead) |
| `net-census-*.csv` | counted device, in the order the Census shows them |
| `net-ble-*.csv` | packet, in the list's order, as it was shown (held or filtered) |
| `net-fer-*.csv` | SNR bin of the frame error curve, for all traffic and each device |
| `net-bt-*.csv` | classic hit, oldest first, with its slot residual and header; `lap_kind` says piconet or inquiry code |
| `net-coded-*.csv` | LE Coded packet, in the same columns as `net-ble`; its `df1` and carrier columns are the LE Coded tests' (S=8 only), and its `note` says so |

They go to `~/.local/share/sdrtop/` (or `$XDG_DATA_HOME/sdrtop/`), named with
the second they were taken, and a second export in the same second is
refused rather than overwriting the first. If you are exporting twice a
second, it was probably the first one you wanted.

Every file opens with the same header of `#` lines: the sdrtop version, when
it was exported, the device and its serial, the tuning, SURVEY or LOCK, how
the addresses are shown (with the registry dates), what the offsets are
worth, and how long the session has run. A `note` line says what the file is
short of, or, when it has no rows, which of the silences it is.

A reading with an uncertainty is two columns, the value and its sigma. A field
the screen would show as a dash is **blank** in the file, never a zero.

---

## If you wrote your own NET preset

Three panels changed name or went away while this section took its current
shape. A preset of your own that names them needs updating:

| Was | Now |
|-----|-----|
| `net_ble_rf` | `net_ble_detail` (the packet detail) |
| `net_bt_census` | `net_bt_piconets` (the piconet roster) |
| the `net_coexist` preset | part of `net_survey`; the `net_coexist` panel itself is unchanged |

The built-in views moved too: Census and the BLE view (now Advertising)
went from `section = "net"` to `"le"`, the three classic views to
`"classic"`. A preset of yours still filed under `"net"` keeps working and
stays in NET; move it with `section` and `slot` if you want it beside its
kind. All three sections are hidden together on a radio that cannot reach
the band.

How presets are written is in [Layout presets](presets.md).
