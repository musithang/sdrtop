# The NET Section

← [Back](README.md)

**NET** is sdrtop's look at the 2.4 GHz band: who is on the air, what they
are saying, and how well their transmitters do it. It listens to Bluetooth
Low Energy advertising and to classic Bluetooth, and it measures the band
itself. It never transmits, never joins or follows a connection, and never
plays audio: the guest at the party who says nothing all evening and leaves
knowing everyone's address.

The section only appears on a radio that reaches the band and can sample fast
enough for its cheapest mode. On one that cannot, the section is hidden and
the log says why in one line.

Five views, one question each. The key is the number to press while the NET
section is active:

| Key | View | The question it answers |
|-----|------|--------------------------|
| `NET 1` | **Capability** | What can this radio reach and receive here? |
| `NET 2` | **Survey** | What is in this band, and who is spending the airtime? |
| `NET 3` | **Census** | Who is here? |
| `NET 4` | **BLE** | What is this device advertising, and is its transmitter any good? |
| `NET 5` | **Classic** | Who is running a classic Bluetooth piconet near me? |

The panels with controls announce them with a highlighted letter in the
title; the full list is in [Keyboard Shortcuts](keys.md#net-panel-focus-modes).
The menu shows a live line under every NET view, built by the same rules the
view itself uses, so it never promises more than the screen delivers.

---

## Reading any NET screen

A few things are the same on every NET panel, and they are the difference
between a number and a claim you can trust.

### Survey or lock (`m`)

The radio can see a slice of the band at a time, not all 83 MHz of it.
**SURVEY** steps the slice across the band and spends a fraction of the time
at each position; **LOCK** parks on one position and watches it without a
gap. `m` switches between the two, and every panel carries `[SURVEY]` or
`[LOCK]` in its title so a number is always read with how it was gathered.

A surveyed count is a sample of the band, not a complete record. A few
readings only exist in LOCK: a BLE device's advertising interval, for one,
needs arrivals that were never interrupted by a hop.

### The three silences

An empty panel always says which kind of empty it is:

- **Refused, and why.** The receiver cannot run here, for a stated reason:
  the view holds no Bluetooth channel, the sample rate is too low, LE 2M was
  asked for on an advertising channel where it is never sent.
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

The masked mode covers addresses only. An advertised device name and a
classic piconet's LAP are shown as they are in every mode, so check both
before sharing a screenshot: a perfectly masked `#17` sitting next to
"Viktor's AirPods" has not protected anybody.

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
tag then says so.

### Reasoned, not verified

Some of what NET measures has been checked against a real transmitter and
some has only been written from the specification and tested on synthetic
signals. Where it matters, the screen says which:

- A limit **read from the Bluetooth Core Specification** names its section,
  for example `Core 5.4 Vol 2 A 3.1.1` beside the classic modulation rows.
- BLE's modulation limits are recalled from test-specification documentation
  and not yet checked against the specification itself.
- The classic Bluetooth header decode is a port of `libbtbb`, and its
  section is headed "libbtbb port, unchecked on air". It has passed every
  test I could write for it and has never met a real classic transmitter,
  which are two different kinds of confidence.
- **"predicted, not followed"** beside a BLE connection's hop sequence means
  sdrtop worked out which channels the connection will use from its own
  parameters, and did not follow it there. It never does.

---

## Capability · `NET 1`

What the radio in your hands can do in this band, before a single sample
arrives. The verdict at the top (`5 OF 8 MODES`, `OUT OF BAND`) is composed
only from the facts drawn below it:

- **Tuner**: whether the tuning range covers the whole band, and with how
  much to spare either side.
- **Modes**: which Bluetooth and Wi-Fi modes the sample-rate ceiling can
  carry, and how far short the nearest one out of reach is.
- **Retune** (`K`, focus `k`): times the radio's tuning call across the band.
  A call slower than the shortest BLE connection interval rules following a
  connection out; a faster one is necessary but not proof, because the call
  does not include the synthesiser settling. The panel says which of the two
  it is and never "fast enough", because optimism is not a measurement.

---

## Survey · `NET 2`

What is in the band, measured one megahertz at a time.

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

The cursor (`←` `→` a megahertz at a time, `B` to the busiest cell) reads out
one cell, and `L` locks the receiver there.

### Coexistence *(focus `z`)*

The same band over time, as a heatmap under the occupancy profile, sharing
its frequency ruler. Decoded BLE packets and classic hits are marked over it
in their own colours, so "something is here" and "this is Bluetooth" read
from one picture. `↓` steps back in time and the profile above shows that
moment; `↑` forward; `N` back to now.

### Decode health

The account of the feed and the decoders: blocks in, blocks lost, gaps, and
the BLE decode funnel (triggers, and how each one ended). Every count on the
other NET panels is only as complete as this panel says the feed was.

---

## Census · `NET 3` *(focus `u`)*

One row per transmitter the BLE decoder has confirmed: an address is only
counted from a packet whose CRC passed, because a corrupted address would be
a device that does not exist.

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
it said them), its readings and the PDU types it sent.

---

## BLE · `NET 4`

Real BLE advertising packets, CRC-checked, as they arrive. The radio has to be
on an advertising channel (2402, 2426 or 2480 MHz); the header says which
channel the decoder has.

### The packet list *(focus `v`)*

Newest first: channel, type, address, the advertised name, address type,
length, CRC, SNR, carrier offset and age. A name is only read from a packet
whose CRC passed, and is made safe to print: a control character a device
puts in its name shows as a replacement mark, never as a command to your
terminal.

- `Enter` narrows the list to the selected packet's address, and back.
  **[FILTERED]** says the list is not everything. Selecting a device in the
  Census and switching here narrows the list to it for you.
- `H` holds the list still; the title counts what has arrived since.
- `P` switches between LE 1M and LE 2M. LE 2M is never sent on the three
  advertising channels, and the panels refuse it there rather than showing a
  quiet list.

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
  Channel Selection Algorithm #1 or #2, **predicted, not followed**.
- **Physics**: SNR, carrier offset in kHz and ppm, the offset at the start
  and end of the packet.
- **Modulation**: the transmitter's modulation index, deviation and drift
  against the specification's limits, each drawn as a bar with the limit
  marked. Read from the packet's own symbols; a packet whose CRC failed is
  not measured, because which symbols were ones decides where deviation is
  read.

With **no packet selected**, the detail shows the session's **frame error
rate against SNR**: for each 2 dB of SNR, what share of packets failed their
CRC, with its uncertainty. A bin with fewer than ten packets gives its count
and no rate. Filtered to one address, it is that device's own curve. A
receiver whose failures do not fall as SNR rises is failing for a reason that
is not noise, and this curve is where that shows.

---

## Classic · `NET 5`

Classic Bluetooth hops across 79 one-megahertz channels, 1600 times a second,
in a sequence a passive listener does not know in advance. What can be found
without joining is the **access code** at the start of every packet, which
carries the piconet master's LAP (the lower 24 bits of its address). sdrtop
watches as many channels as the view holds, up to `[net].bt_channels` (eight
by default, see [Configuration](config.md)), and takes only exact matches, so
a LAP on screen was sent.

A LAP names a **piconet**, not a device: every member of the piconet sends its
master's access code. The panels say piconet throughout.

### Hops *(focus `b`)*

Two answers:

- **WHERE**: all 79 channels, with bars of each channel's hits this session,
  in the colour of the piconet heard most there, and the channels watched
  now underlined. A piconet heard at an earlier survey position keeps its
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
more headers bring it down to two, and a DH1, DH3 or DH5 payload's own CRC
settles it to one. The column shows `32 left`, `2 left` or the value.

The selected piconet's detail, in as many sections as the panel has room for
(the rest are named on the last line):

- **Modulation**: the piconet's BR modulation index and deviation against
  0.28 to 0.35, read from the Core Specification, from every header's
  symbols.
- **Timing**: how far each hit lands from the piconet's own 625 µs slot grid,
  fitted to its hits, against the specification's 1 µs, with the spread drawn
  under it. Below eight hits it is collecting; hits that do not line up on a
  grid beyond chance are refused as one, never forced onto it. Given enough
  periods to try, a dozen points will line up with almost anything, and the
  panel would rather say "no grid" than find one it wanted to find.
- **Headers**: once the UAP is one value, what the piconet's headers say: the
  packet types (`POLL 3 · NULL 1 · DH1 1`), the logical transport addresses
  in use, and headers that did not decode. Before that, nothing is read or
  guessed.

---

## Taking the data away (`o`)

`o` anywhere in NET writes five files, one for each record the section keeps:

| File | One row per |
|------|-------------|
| `net-band-*.csv` | megahertz of the band: duty, its uncertainty, coverage, power |
| `net-census-*.csv` | counted device, in the order the Census shows them |
| `net-ble-*.csv` | packet, in the list's order, as it was shown (held or filtered) |
| `net-fer-*.csv` | SNR bin of the frame error curve, for all traffic and each device |
| `net-bt-*.csv` | classic hit, oldest first, with its slot residual and header |

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

How presets are written is in [Layout presets](presets.md).
