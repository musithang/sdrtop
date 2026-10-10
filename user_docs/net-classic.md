# Piconets · `Classic 1`

← [The NET section](net.md)

Classic Bluetooth hops across 79 one-megahertz channels, 1600 times a
second, in an order a passive listener is not told. It is a conversation
held while running through a building, and sdrtop is standing in a few of
the rooms.

What can be found without joining is the **access code** at the start of
every packet, which carries the piconet master's LAP: the lower 24 bits of
its address. sdrtop watches as many channels as the view holds, up to
`[net].bt_channels` (eight by default, see [Configuration](config.md)), and
takes only exact matches, with no bit errors forgiven. So a LAP on screen
was sent, not reconstructed from something that looked a bit like one.

A LAP names a **piconet**, not a device: every member of a piconet sends its
master's access code. The panels say piconet throughout, because that is
what was heard.

## Who else is on the list

Not every row is a piconet, and the `KIND` column says which:

| `KIND` | What it is |
|--------|------------|
| `piconet` | A master and whoever is talking to it |
| `inquiry` | A device looking for others, on one of the reserved codes |
| `paged` | A device being called by another, which has not answered yet |

**Inquiry.** The specification keeps 64 LAPs, 0x9E8B00 to 0x9E8B3F, for
devices looking for others, and every device looking uses one of the same
few. The roster names them by their abbreviations: `GIAC` (general
inquiry, 0x9E8B33, the one you will actually see), `LIAC` (limited,
0x9E8B00) and `DIAC` (the dedicated rest). A GIAC row hopping over dozens of
channels is not an unusually chatty piconet; it is your phone asking the
room who is there. Its UAP is fixed at `DCI` (0x00) by the specification
rather than worked out, and it gets no modulation, timing or header
sections: those would be every searching device's at once, averaged into
one confident fiction.

**Paging.** One device calling another to connect sends the *called*
device's own access code, as short ID packets with no header, in trains
that hop 3200 times a second. A piconet's packets start on whole slots;
a page's come twice as fast, so the gap between two of its hits is often an
odd number of half slots:

```
a piconet:   |       |       |       |       |      whole 625 µs slots
a page:      |   |   |   |   |   |   |   |   |      every 312.5 µs
```

When a LAP has had no header after any of at least 16 hits, *and* its
spacings include odd half slots beyond what chance gives, the roster calls
it `paged`: that LAP belongs to the device being called, not to anyone's
piconet. With either sign missing it stays a plain piconet row. A page that
was answered carries headers, so it is missed rather than named wrongly,
which is the right way round to be wrong. Selected, an inquiry or a page
row gets a short detail of its own instead of the piconet's sections: what
the code means, its hits, and a `pace` line with the counts behind the
verdict (`20 of 40 close spacings on odd half slots`). The Classic export's
`lap_kind` says `paged` too.

## Hops *(focus `b`)*

Two answers, one above the other:

- **WHERE**: all 79 channels, with a bar of each channel's hits this
  session, in the colour of the piconet heard most there. The channels
  watched now are bracketed underneath (`╰──╯`, or `▲` for a single one). A
  piconet heard at an earlier survey position keeps its place.
- **WHEN**: one lane per piconet, a tick at each hit's time. `+` and `-`
  zoom from half a second to a minute, `←` `→` move back and forward in
  time, and `End` returns to now. If the window reaches further back than
  the hits sdrtop keeps, it says "older hits not kept" rather than drawing
  quiet lanes and letting you conclude the piconet had a nap.

Each piconet wears one colour here and in the roster, and one selection
(`↑` `↓` in either panel) drives both.

## Piconets *(focus `c`)*

One row per LAP, the most recently heard first:

| Column | What it is |
|--------|------------|
| `LAP` | The access code's address part, or `GIAC`, `LIAC`, `DIAC` |
| `KIND` | Piconet, inquiry or paged, as above |
| `LAST` | How long ago it was last heard |
| `HITS` | Access codes found, all channels together |
| `CH` | How many of the 79 channels it was heard on |
| `UAP` | `32 left`, `2 left`, or the value, see below |
| `FIRST` | How long ago it was first heard |

**The UAP is the part Bluetooth does not say.** It is the next 8 bits of
the master's address, it is never sent, and it has to be worked out from
the headers that follow the access code. It is a crossword where every clue
has two answers:

```
a first header       32 candidates
more headers          → 2 left        as far as headers alone can go
a payload CRC         → 1, and with it the piconet's clock
```

The CRC is a data packet's own: DH1, DH3 and DH5 as sent, DM1, DM3 and DM5
through their error-correcting code first. The same payload settles which
of the piconet's possible clocks the header was sent at, and from then on
its headers are read at that clock and no other. A header that could still
be read two ways is left unread rather than read the likelier way.

The selected piconet is spelled out under the roster as a whole. The rule
of the three classic views: the piconet as one thing is here, its packets
on [Classic 2](net-classic-packets.md), and each of its two ends on
[Classic 3](net-classic-bench.md).

**PICONET** is what it is and how sure that is:

| Row | What it says |
|-----|--------------|
| `LAP` | The address part heard, the master's |
| `UAP` | The value and what it rests on (`resolved by a payload CRC · 12 CRCs pass under it`), or the candidates left. Two is where an encrypted link stays, see [Reasoned, not verified](net-trust.md#reasoned-not-verified) |
| `heard` | Hits, first and last heard |
| `channels` | How many of the 79, and which (`5 of 79: 2-5, 17`) |
| `clock` | The master's crystal, from the slot grid; below |
| the last row | Pooled readings, the modulation `index` and the carrier `f0`, every member's together |

**The clock** comes from the piconet's 625 µs slot grid, fitted to every
member's hits: slots that run long on our clock mean a master whose crystal
runs slow. Until there are enough hits it says so (`collecting: 5 of 8
hits to fit a slot grid`), and `no slot grid` when they do not line up at
625 µs beyond chance. Without a
frequency reference it is shown relative to our own oscillator, like every
other ppm in NET; with one it is corrected and said to be inside, at the
edge of, or outside the specification's 20 ppm. That assumes the radio's
tuner and its sample clock share one crystal, which is true of a HackRF and
an RTL-SDR. The Classic export carries it as `clock_ppm`.

Beside it is the grid's rms. That is every member's hits together, so it is
not a jitter: the two ends' offset from each other is in it.
[Classic 3](net-classic-bench.md) has each end's own.

**HEADERS** appears once the UAP is one value, and says what the piconet's
headers said over the session:

| Row | What it says |
|-----|--------------|
| `read` | How many were read of those captured, and how many did not decode under the resolved UAP: a rising count is how a wrong UAP would show |
| `types` | The packet types, most frequent first |
| `LT_ADDR` | The logical transport addresses in use (`0` is broadcast) |
| `LMP` | How many [link manager messages](net-classic-packets.md#the-link-managers-messages) were read and which came last (`8 messages · last encryption_key_size_req, 12 s ago`), or `none read` |
| `CLK1-6` | How the hunt for the clock stands, out of 64 hypotheses |

Before the UAP is one value, nothing is read or guessed: the section says
how many headers were captured and why they wait.

`Enter` opens [the Packets view](net-classic-packets.md) on the selected
piconet, and `3` [the Bench](net-classic-bench.md). An inquiry code or a
page is not a piconet either of them could open, so its whole account stays
here. A panel too short for everything keeps what fits whole and names the
rest (`+ HEADERS on a taller panel`), rather than cutting a section in half.

---

← [The NET section](net.md)
