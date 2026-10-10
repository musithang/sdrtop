# Bluetooth Low Energy · `LE 1` and `LE 2`

← [The NET section](net.md)

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

**An extended advertiser** is counted from its auxiliary packet, the one
that carries its address and its name: its `ADV_EXT_IND` names nobody. So
it appears once an AuxPtr of its has been followed to a channel in view
([extended advertising](#the-packet-list-focus-v)), its types say
`extended`, and its interval stays unknown, because an auxiliary packet
comes when its primary says, not on an interval of its own.

---

## Advertising · `LE 2`

Real BLE advertising packets, CRC-checked, as they arrive. The radio has to be
on an advertising channel (2402, 2426 or 2480 MHz); the header says which
channel the decoder has, and whether it is one ([the header](net-reading.md#the-header)).

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
followed on [LE 3](net-le-connection.md) that moves to LE 2M is
received on it from the moment the two devices agreed, by itself, as it
should be, at a sample rate that is a multiple of 8 Msps (8 or 16). At
20 Msps there is no LE 2M receiver, and its events say **no receiver**
rather than pretend to have listened.

In SURVEY the line under the list gives each advertising channel's packet
count and CRC pass rate; in LOCK, the one channel's.

**Extended advertising** is followed here as it is on
[LE Coded](net-le-coded.md): an `ADV_EXT_IND` on the
advertising channel points at an `AUX_ADV_IND` on a data channel, and when
that channel is in view the auxiliary packet joins the list, named and
addressed, under its own channel. One sent on LE 2M is not followed at
20 Msps, and says why: there is no LE 2M receiver at that rate. A retune
(the survey's, or yours) drops whatever was waiting for its window, and
says that too.

### Packet detail

The selected packet, spelled out:

- **Packet**: type, channel, length, PHY, CRC, addresses, the ChSel bit.
- **Extended**, for an extended advertising packet: the event and set it
  belongs to, its TxPower, the packet that pointed at it, and what became
  of its own AuxPtr, worded as on LE Coded.
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

  <a id="how-snr-is-read"></a>**How the SNR is read.** In LE 1M's band, as
  the packet's power over the noise in a quiet stretch just before it (130
  to 30 µs before: a transmitter's carrier is often up before its
  preamble), with the radio's DC offset measured there and taken out of
  both. It is the same scale for LE 1M and LE Coded. A packet heard straight
  after another, with no quiet stretch before it, shows no SNR rather than a
  low one. LE 2M's is still the receiver's own estimate. Before this, the
  envelope was read, and that counted anything that moves it as noise: a
  phone 35 dB above the noise read 18 at the tuned centre, where the radio's
  DC sits under the packet.
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
  (see [the bench](net-classic-bench.md) below); on LE 1M the packet is taken again
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

← [The NET section](net.md)
