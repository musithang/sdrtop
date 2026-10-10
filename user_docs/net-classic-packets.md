# Packets · `Classic 2`

← [The NET section](net.md)

One piconet, packet by packet, on the whole screen. If [the Piconets
view](net-classic.md) is the room, this is one conversation in it:
overheard, timed and measured, and still not understood, which is the
polite way round.

It shows the piconet selected on the Piconets view. `Enter` on the roster
brings it here, and so does `2` with a piconet selected. With the list
focused, `← →` step to the previous or next piconet in the roster's order.
With none heard, or none selected, the list says which of the two it is
rather than showing an empty table and letting you wonder.

The classic receiver runs here exactly as on the Piconets view: same
channels, same `← →` steps for a locked radio when nothing is focused. The
three classic views are three readings of one stream of hits, not three
receivers with three opinions.

## Who sent it

Classic Bluetooth is strict about turns. The master starts its
transmissions in the even slots, the slave in the odd ones, both counted on
the master's clock (Core 5.4 Vol 2 Part B 2.2.5):

```
                 ├── 625 µs ──┤
master's CLK1:   │     0      │     1      │     0      │     1      │
who starts:      │    M ▶     │    ◀ S     │    M ▶     │    ◀ S     │
```

So once a header has been read at one clock (the UAP resolved and the
clock pinned, see [Piconets](net-classic.md#piconets-focus-c)), the clock's
lowest bit says who sent the packet: `M ▶` for the master, `◀ S` for the
slave. Before that, and for an ID packet, which has no header to read at
all, the row gets a dot. A packet whose sender is not known is counted
apart from both sides, as **not yet placed**, instead of being handed to
whichever side seemed likelier.

The master wears the piconet's own colour, the one its chip has on the hop
chart and in the roster. The slave wears the ordinary ink, which no
piconet's colour ever is, so the two ends never look alike.

## The packet list *(focus `v`)*

Newest first, one row a packet, the last 1000 of this piconet kept:

| Column | What it is |
|--------|------------|
| `AGE` | How long ago the block it arrived in was read. Tenths of a second under ten seconds, and no finer: the time is the block's, not the packet's own |
| `CH` | The classic channel it was heard on |
| `DIR` | Who sent it, as above |
| `TYPE` | The header's packet type, in every reading its code allows (`DM3/2-DH3`, see [Reasoned, not verified](net-trust.md#reasoned-not-verified)); `ID` for an access code alone; `(no UAP)` or `(no clock)` for a header that was captured but cannot be read yet |
| `LT` | The logical transport address |
| `F A S` | The FLOW, ARQN and SEQN bits |
| `CLK` | CLK1-6, the clock the header was read at: even is the master's, odd the slave's |
| `ΔSLOT` | Slots since the packet before it on the same stream |
| `SLOT µs` | How far it landed from the piconet's slot grid |
| `MOD` | Its own modulation index |
| `f0 kHz` | Its own initial carrier offset, relative to our oscillator until a reference makes it absolute |
| `PAYLOAD` | What became of its payload, in the words below |
| `LMP` | What a payload that passed carries, the link manager's message above all, see [below](#the-link-managers-messages) |

**`ΔSLOT` is the link's rhythm.** A master and a slave taking turns read
`+1 +1 +1`; a three-slot packet is followed by `+3`; a quiet spell shows as
a jump. Inquiry and paging run on half slots, and those show as `+0.5`,
`+1.5`. After a stream restart (a rate change, `Space` off and on) the
column is blank, and so is `SLOT µs` for a packet from before the grid was
fitted: times from another run of the stream are on another clock, and
subtracting them would produce a number, just not a meaningful one.

**`MOD` and `f0` are one header's readings**, noisier than [the
bench's](net-classic-bench.md) pooled ones, and printed to the places their
own uncertainty allows, a dash where it allows none. Against the BR limits
they follow the bench's colours: red outside, amber when the value is too
close to the line for its uncertainty to say which side it is on. `f0` is
judged only once a reference makes it absolute. Everything comfortably
inside keeps the ordinary ink, so a problem is the one thing that stands
out.

The payload column, word for word:

| Shows | Means |
|-------|-------|
| a dash | Nothing to check. POLL, NULL and ID carry no payload |
| `✓ CRC` | Read, and its CRC passed |
| `✗ CRC` | Read, and its CRC failed. Why, it does not say. An encrypted payload fails exactly like a damaged one, and sdrtop has neither the key nor a crystal ball |
| `· FEC failed` | A DM payload whose error-correcting code could not be undone. A damaged capture does that, and so does a payload that was never basic rate: an EDR packet read as its basic-rate twin (the `2-DH3` of a `DM3/2-DH3`) ends up here or at `✗ CRC`, and the packet alone cannot say which it was |
| `· cut short` | The capture ended before the packet did: the edge of a block, or a block the feed lost |
| `· type not read` | A type sdrtop has no payload reader for: HV, DV, EV, FHS and AUX1 |
| `· bad length` | A payload header whose length cannot hold a CRC |
| `· clock not known` | The header was not read at one clock, so there is no whitening to undo |

**Moving around.** `↓` scrolls into the past, and the first press holds
the list where it is, so the rows stop sliding away while you read them; `↑`
goes back up, `H` holds or lets it run, and `End` is live again. Held, the
title says what the pause is costing (`[+37 NEW]`). The last line counts
the session's packets by side, and the ones not yet placed.

## The link manager's messages

Before two classic devices exchange a single note of music, their link
managers have a meeting: what each supports, which version of the Core it
was built to, its name, how to pair, whether to encrypt. Each item on that
agenda is an **LMP message**, and it rides in a DM1 whose payload header
says LLID 3 (Core 5.4 Vol 2 Part C 2.3). When such a payload passes its
CRC, the `LMP` column reads it. Two rows a pair of headphones and a phone
actually sent, the columns between `TYPE` and `PAYLOAD` left out:

```
 AGE▴   CH DIR   TYPE   PAYLOAD  LMP
 35 s   58 M ▶   DM1    ✓ CRC    M: features_req_ext  page 2 · max page 2 · 1f 03 00 00 00 00 00 00
 35 s   57 ◀ S   DM1    ✓ CRC    M: features_res  ff fe 0f fe d8 3f 5b 87
```

The name is Part C Table 5.1's, lower-cased and without the `LMP_`. The
parameters are in words wherever Part C 5.2 gives them a meaning
(`supervision_timeout  32000 slots (20.0 s)`, `set_afh  on · 20 of 79
used`), and in hex where it does not; the feature masks above are
bit fields, and stay hex. Nothing is read from a payload whose CRC failed.
On a narrow terminal the message is cut, and the cut is marked `…`.

- **`M:` is who called the meeting, not who is talking.** The prefix is the
  message's transaction ID: who *began* the exchange (Part C 2.4), not who
  sent this packet. The second row above is the slave answering the
  master's question: `DIR` says the slave sent it, the prefix still says
  `M:`. Bluetooth keeps the minutes by the chair, not by the speaker.
- **The other end's version and maker.** `version_res` names the Core
  version and the company from the Bluetooth SIG's own lists, the same
  dated snapshot the [address modes](net-reading.md#addresses-i) use. A
  number the snapshot does not have stays a number (`company 0xfffe (not in
  the SIG list)`), never a guess.
- **Keys are named, never shown.** A random number, a key, a commitment or
  a nonce reads as what it is (`au_rand  16-byte random number`), never as
  its bytes. The bench measures a link; collecting what a key search needs
  is somebody else's hobby.
- **A name, an address and an unknown body follow `i`.** `name_res` carries
  part of the device's name, `slot_offset` a whole address, and an opcode
  Table 5.1 does not list carries who knows what. In **oui** the address
  shows only its "who"; in **masked** the name is its length, the address
  its "who" and a number, and the unknown body a byte count, as on [the
  rest of NET](net-reading.md#addresses-i).
- **What the table cannot vouch for is said.** A body longer or shorter than
  Table 5.1 gives (`(length 4, Table 5.1: 9)`), one that ends early (`cut
  short`), an opcode the table does not list, or lists as retired: each in
  its own words.
- **The direction check.** Some messages go one way only, by the table. One
  seen going the other way, by its slot's parity, turns the warning colour
  and quotes the rule (`Table 5.1: C → P only`). That is as much a check on
  the `DIR` reading as on the devices: on a healthy link it should never
  light up, and if it does, suspect us first.
- **Everything else that passed** shows its kind and length only (`L2CAP
  start, 23 bytes`), and LLID 3 in anything but a DM1 is named, not read.

**Why so few.** A link manager is chatty for about a second, while the link
is being set up, and then goes quiet behind encryption: once the link
encrypts, its messages are encrypted with everything else and stop passing
their CRC. Headphones that connected an hour ago have nothing left to tell
you. Switch them off and on while sdrtop listens; it is the cheapest way to
make two devices introduce themselves again, and the same trick that
settles [an encrypted link's UAP](net-trust.md#reasoned-not-verified). Even
then, only the messages that land on a watched channel, in a block the
receiver kept up with, are read, so seeing only some of the meeting is
normal, not a fault.

**`l` keeps them.** A busy link pushes an LMP message out of the packet
list within seconds, so each piconet also keeps its last 256 LMP messages
apart, and `l` switches the list to them and back:

| | Every packet | `l`: LMP only |
|-|--------------|---------------|
| Title | `… · UAP 0x67` | `… · UAP 0x67 · LMP only` |
| Rows | the last 1000 packets | the last 256 LMP messages |
| `ΔSLOT` | slots since the row below | blank: the row below is no longer the packet before |
| Over the rows | nothing | what the link has done since: `since the last: 412 payloads checked, none passing` |
| Last line | packets by side | `8 LMP messages · 8 kept` |

The "since the last" line shows while the list is at its newest message,
and names no cause, for the same reason `✗ CRC` names none: encrypted and
damaged look alike from here. Scrolling and holding work as in the full
list, and `End`, `H` and `← →` keep the choice. An empty log says why it is
empty: only a DM1 whose CRC passes is read, and an encrypted link shows its
LMP only at a reconnect.

---

← [The NET section](net.md)
