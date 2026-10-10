# Piconets · `Classic 1`

← [The NET section](net.md)


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

## Hops *(focus `b`)*

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

## Piconets *(focus `c`)*

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
  verified](net-trust.md#reasoned-not-verified)), when it was heard and on which of the
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
  broadcast), how many [link manager messages](net-classic-packets.md#the-link-managers-messages)
  were read and which came last (`8 messages · last
  encryption_key_size_req, 12 s ago`, or `none read`), and how the hunt for
  the clock stands. Before that, nothing is read or guessed.


`Enter` opens [the Packets view](net-classic-packets.md) on the selected piconet,
and `3` [the Bench](net-classic-bench.md). An inquiry code or a page is not a
piconet either could open, so their whole account stays here. A short panel
keeps what fits whole and names the rest.

---

← [The NET section](net.md)
