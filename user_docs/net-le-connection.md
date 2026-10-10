# Connection · `LE 3` *(focus `e`)*

← [The NET section](net.md)


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

## Measured

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

← [The NET section](net.md)
