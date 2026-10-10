# Reading any NET screen

← [The NET section](net.md)

A few things are the same on every NET panel, and they are the difference
between a number and a claim you can trust. Read this page once and every
view after it is shorter.

## SAT: when the radio is shouting

When `SAT 4.4 %` appears first on the header's band line, that share of the
samples is pinned at the converter's limit. It is the same reading and the
same colours as every SAT in sdrtop:

| SAT | Shown |
|-----|-------|
| under 1 % | not at all |
| 1 % to 5 % | amber |
| 5 % and over | red |

A clipped packet is a distorted one. It fails its CRC while its SNR still
looks fine, so while SAT shows, the failures say more about the radio than
about the device. Lower the LNA or VGA, or move the radio further from the
transmitter. A BLE remote held next to a HackRF at LNA 32 does exactly
this, and is very sure of itself about it.

A clip lasts a fraction of a second and you may look a moment later, so
once the reading falls back below 1 %, a clip that reached 5 % leaves
`⚠ last clip 3s` in its place: red for six seconds, then grey, then gone
after half a minute. It is the same line the Command Rail keeps under its
SAT, drawn by the same code, so the two never disagree about when the radio
last shouted.

## Survey or lock (`m`)

The radio sees a slice of the band at a time, not all 83 MHz of it.

| Mode | What the radio does | Good for |
|------|---------------------|----------|
| **SURVEY** | Steps the slice across the band, a fraction of the time at each position | Who is anywhere in the band |
| **LOCK** | Parks on one position and watches it without a gap | Anything that needs an unbroken run |

`m` switches between the two, and every panel carries `[SURVEY]` or
`[LOCK]` in its title, so a number is always read with how it was gathered.
A surveyed count is a sample of the band, not a complete record. A few
readings only exist in LOCK: a BLE device's advertising interval, for one,
needs arrivals that were never interrupted by a hop.

What SURVEY walks depends on the view. The BLE list, the Census and LE Coded
are fed by an advertising decoder, so there it rotates the three
advertising channels and nothing else. Everywhere else it walks the whole
band, and the BLE decoder takes whichever advertising channel is in view at
each stop, not just the channel the stop happens to be centred on. (It used
to take the latter, which at 8 Msps meant channel 37 was never heard in a
survey at all: the band's layout and Bluetooth's had simply never been
introduced.)

## Stepping while locked

`←` and `→` move a locked radio without typing a frequency:

| On | A step is |
|----|-----------|
| Advertising, Census, LE Coded | The next advertising channel: 37, 38, 39 and round again. Anywhere in between is an expensive way to hear nothing |
| The classic views | One block along: the span, or the most channels the view watches at once (`[net].bt_channels`) if that is fewer |
| Everything else | One span along |

Steps wrap round at the band's ends. Surveying, the survey owns the tuning,
and the keys just remind you that `m` locks.

A lock carries across views, so a Survey locked on a Wi-Fi channel would
arrive at the Advertising view parked where no advertising ever comes.
Opening an advertising view off the three channels therefore moves the
radio to the nearest of them, once, and the log says so. Tune somewhere
else afterwards (a data channel, to catch secondary advertising, say) and
it stays where you put it: the instrument does not overrule its user twice.

## The header

In NET the strip under the radio's name is the band, 2400 to 2483 MHz, and
the lit stretch is what the radio can see right now: the tuning, give or
take half the span. The classic channels being watched are drawn over it in
the classic colour, and the channel the BLE decoder is on is a `●` in the
BLE colour (white and pink on the default theme), the same marks and inks
the coexistence history puts on their hits. In SURVEY the lit stretch walks
the band with the survey.

The line under it says the same in words:

| It says | Meaning |
|---------|---------|
| `● BLE 38 adv` | The BLE decoder is on advertising channel 38 |
| `● BLE 3 data` | It is on data channel 3, where advertising never comes: the usual reason for a quiet list |
| `● CODED 38 adv` | The LE Coded receiver, on channel 38 |
| `■ BT 5–11` | The classic channels being watched |
| `Wi-Fi ch 6` | No decoder is running, so the tuning is named the way everyone else reads 2.4 GHz |
| `decode 81%` | How much of real time the decoders need; see below |

## The three silences

An empty panel always says which kind of empty it is:

| Silence | What it means |
|---------|---------------|
| **Refused, and why** | The receiver cannot run here, for a stated reason: the view holds no Bluetooth channel, the sample rate is too low |
| **Listening, nothing heard yet** | The receiver runs; the room has been quiet, or not quiet for long |
| **Not listening** | Nothing is running for this panel right now: RX is stopped, or the view that feeds it is not open |

sdrtop never shows a bare empty table, or a zero that means "we did not
look". Those three look identical as a blank rectangle, and only one of
them is about the room.

## Stale, and feed loss

| Tag | Meaning |
|-----|---------|
| **[STALE]** | RX has stopped; the panel shows what it had |
| **[FEED LOSS]** | The sample feed dropped blocks inside the stretch of time this panel's numbers cover. Its counts are lower bounds from then on |

The header's `decode` figure is how much of real time the decoders need.
Over 100 % they cannot keep up and blocks are dropped, which the `gaps`
count and the feed-loss tags then show. The [Survey's decode
health](net-band.md#decode-health) has the whole account.

## Addresses (`i`)

`i` switches how addresses are shown, on every panel and in every export at
once:

| Mode | Example | What it shows |
|------|---------|---------------|
| **full** | `d1:9a:7e:91:27:9e` | The whole address |
| **oui** | `Apple ..09:be`, `RPA ..4e:12` | Who it belongs to, and enough of the rest to tell two apart: the IEEE registrant for a public address, the address's kind for a random one |
| **masked** | `Apple #17` | The same "who", and a session number instead of any part of the address |

In **oui**, where a random address's manufacturer data names a company, the
company takes the kind's place, marked with where it came from:
`Apple·mfr ..09:be`. "Company" is whose data format it is, not who made the
device. In **masked**, the numbers are handed out in the order addresses
are heard and are not derived from them, so nothing in a screenshot can be
turned back into an address.

The masked mode covers everything that would give a device away, not just
its address, because a perfectly masked `#17` sitting next to "Viktor's
AirPods" would have protected nobody:

| In masked | Shows as |
|-----------|----------|
| An advertised name | its length, `name, 16 chars` |
| Manufacturer and service data | its size, `23 bytes`, beside the company or service it belongs to, since some of those payloads carry an identifier of their own |
| A classic piconet's LAP | its place in the roster, `#3`, the same number on the hop lanes and in the export |
| A resolved UAP | `found` |
| A classic link manager's name, address or unreadable body | as above; see [Classic 2](net-classic-packets.md#the-link-managers-messages) |
| An inquiry code | still `GIAC`: it is nobody's address |

The **oui** mode still shows names, LAPs and bytes. It is for telling
devices apart; masked is for letting other people look.

The registrant and company names come from dated snapshots of the IEEE's
and the Bluetooth SIG's registries. Whenever the mode is not **full**, the
title of every panel showing addresses says so and gives both dates. A
panel that shows only classic LAPs says so in **masked** alone, since
**oui** leaves a LAP as it is, and a tag claiming otherwise would be wrong.

---

← [The NET section](net.md)
