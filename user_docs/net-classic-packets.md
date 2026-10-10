# Packets · `Classic 2`

← [The NET section](net.md)


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

## Who sent it

The master starts every transmission in an even slot and the slave in an odd
one, both counted on the master's clock (Core 5.4 Vol 2 Part B 2.2.5, read
from the SIG's own copy). So once a header has been read at one clock (the
UAP resolved, the clock pinned, see [Piconets](net-classic.md#piconets-focus-c)), the
clock's lowest bit says who sent it: `M ▶` for the master, `◀ S` for the
slave. Before that, and for an ID packet with no header at all, the row gets
a dot. Never a guess: a packet whose sender is not known is counted apart
from both, as **not yet placed**.

The master wears the piconet's own colour, the one its chip has on the hop
chart and in the roster; the slave wears the ordinary ink, which no
piconet's colour ever is, so the two ends never look alike.

## The packet list *(focus `v`)*

Newest first, one row a packet, the last 1000 of this piconet kept:

| Column | What it is |
|--------|------------|
| `AGE` | How long ago the block it came in was read; to a tenth of a second, because that is the block's time, not the packet's |
| `CH` | The classic channel it was heard on |
| `DIR` | Who sent it, as above |
| `TYPE` | The header's packet type, in every reading its code allows (`DM3/2-DH3`, see [Reasoned, not verified](net-trust.md#reasoned-not-verified)); `ID` for an access code alone; `(no UAP)` or `(no clock)` for a header captured and not yet readable |
| `LT` | The logical transport address |
| `F A S` | The FLOW, ARQN and SEQN bits |
| `CLK` | CLK1-6, the clock the header was read at: even the master's, odd the slave's |
| `ΔSLOT` | Slots since the packet before it on the same stream |
| `SLOT µs` | How far it landed from the piconet's slot grid |
| `MOD` | Its own modulation index |
| `f0 kHz` | Its own initial carrier, relative to our oscillator until a reference makes it absolute |
| `PAYLOAD` | What became of its payload, in the words below |
| `LMP` | What a payload that passed carries: the link manager's message, by name, see [below](#the-link-managers-messages) |

`ΔSLOT` is the link's rhythm: a master and slave taking turns read
`+1 +1 +1`, a three-slot packet is followed by `+3`, and a quiet spell shows
as a jump. After a stream restart (a rate change, `Space` off and on) it is
blank, and so is `SLOT µs` for a packet from before the grid was fitted:
times from another run of the stream are on another clock.

`MOD` and `f0` are single readings from one header, noisier than [the
bench's](net-classic-bench.md)
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

## The link manager's messages

Before two classic devices exchange a single note of music, their link
managers have a chat: what each supports, which version of the Core it was
built to, its name, how to pair, and whether to encrypt. Each of those is
an **LMP message**, and it rides in a DM1 whose payload header says LLID 3
(Core 5.4 Vol 2 Part C 2.3). When such a payload passes its CRC, the `LMP`
column reads it: the message's name from Table 5.1 of Part C, lower-cased
and without the `LMP_`, and its parameters in words where Part C 5.2 gives
them meaning (`supervision_timeout  32000 slots (20.0 s)`, `set_afh  on ·
20 of 79 used`), in hex where it does not. Nothing is read from a payload
whose CRC failed.

- **`M:` or `S:`** in front is the message's transaction ID: who *began*
  the exchange (Part C 2.4), not who sent this packet. That is `DIR`'s
  job. The slave's `accepted` to the master's request is sent by the slave
  and still reads `M:`.
- **The other end's version and maker**: `version_res` names the Core
  version and the company from the Bluetooth SIG's own lists, the same
  dated snapshot the [address modes](net-reading.md#addresses-i) use. A number the
  snapshot does not have stays a number (`company 0xfffe (not in the SIG
  list)`), never a guess.
- **Keys are named, never shown.** A random number, a key, a commitment or
  a nonce reads as what it is (`au_rand  16-byte random number`), never as
  its bytes. The bench measures a link; it does not collect what a key
  search needs.
- **A name, an address and an unknown body follow `i`.** `name_res` carries
  part of the device's name, `slot_offset` a whole address, and an opcode
  Table 5.1 does not list carries who knows what. In **oui** the address
  shows only its "who", and in **masked** the name is its length, the
  address its "who" and a number, and the unknown body a byte count, as on
  [the rest of NET](net-reading.md#addresses-i).
- **What the table cannot vouch for is said.** A body longer or shorter
  than Table 5.1 gives (`(length 4, Table 5.1: 9)`), one that ends early
  (`cut short`), an opcode it does not list or lists as retired: each in its
  own words.
- **The direction check.** Some messages go one way only, by the table. One
  seen going the other way, by its slot's parity, turns the warning colour
  and quotes the rule (`Table 5.1: C → P only`). It is a check on the `DIR`
  reading as much as on the devices: on a healthy link it should never
  light up.
- Other payloads that passed show only their kind and length (`L2CAP start,
  23 bytes`), and LLID 3 in anything but a DM1 is named, not read.

**Why so few.** A link manager talks mostly while a connection is being
set up, and once the link encrypts, its messages are encrypted with
everything else and stop passing their CRC. So a pair of headphones that
connected an hour ago may show none at all, and the fix is the one from
[Reasoned, not verified](net-trust.md#reasoned-not-verified): switch them off and on
while sdrtop listens. Even then, only the messages that land on a watched
channel, in a block the receiver kept up with, are read.

**`l` keeps them.** A busy link pushes an LMP message out of the packet list
within seconds, so each piconet also keeps its last 256 LMP messages apart.
`l` switches the list to them and back (the title says `LMP only`); the
scrolling and the hold work the same, and `End`, `H` and `← →` keep the
choice. `ΔSLOT` is blank there, since the packet before is no longer the
one above. Over the newest message, a line says what the link has done
since: `since the last: 412 payloads checked, none passing`. It names no
cause, for the reason `✗ CRC` names none, above. The
last line counts every message read on the piconet, and how many are kept.

---

← [The NET section](net.md)
