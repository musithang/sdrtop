# Bluetooth Low Energy · `LE 1` and `LE 2`

← [The NET section](net.md)

BLE devices spend their lives shouting short adverts on three channels
(37, 38 and 39, at 2402, 2426 and 2480 MHz) in the hope that somebody
cares. sdrtop cares, professionally. **LE 1** counts who is shouting, **LE
2** reads each shout and measures the voice it came in.

## Census · `LE 1` *(focus `u`)*

One row per transmitter the BLE decoder has **confirmed**: an address is
only counted from a packet whose CRC passed, because a corrupted address
would be a device that does not exist, and the room has enough of the real
ones. Surveying, the Census rotates the three advertising channels itself,
an equal share of time on each, so it fills on its own, with no need to
keep the packet list open beside it.

| Column | What it is |
|--------|------------|
| `ADDRESS` | The address as the [`i` mode](net-reading.md#addresses-i) shows it |
| `KIND` | Its kind: public, static, RPA and the rest |
| `SEEN` | How long ago it was last heard |
| `PKTS` | Packets from it |
| `BEST SNR` | The strongest it has been heard. An SNR, not an RSSI: a ratio to the noise, not a power in dBm the radio has no calibration for |
| `CRC` | The share of its packets that passed. A ceiling: a packet whose address was corrupted is nobody's failure, so it cannot be counted against anyone |
| `CFO` | Its carrier offset in ppm, worth what [the offset tag](net-trust.md#what-an-offset-is-worth) says |
| `MEAN SNR` | The mean, with its uncertainty |
| `TYPES` | How many advertising PDU types it has sent |
| `MOD` | Its modulation index, refined over every packet that allowed one |
| `INTERVAL` | Its advertising interval, timed in LOCK only: it needs one channel listened to without a break. A reading from an earlier LOCK stays in SURVEY, marked as not updating |

`S` sorts by the next column, `R` reverses, and the title says what orders
the table. `T` trusts the selected device as the frequency reference: you
are asked how far its crystal can be off, and every offset in the app is
then **[REFERENCED]** against it. Choose a device whose crystal you have
reason to believe in; a cheap tracker tag makes a confident but poor
standard.

Under the table every device's clock error sits on a null meter, worst
first. Select a device and it gets a dial of its own and a detail block:
when it was first and last heard, what it advertised (its name, TX power
and company, as it said them), its readings, its advertising timing (the
interval, where it sits on the 0.625 ms grid, and the random delay), and
the PDU types it sent. On a wide terminal the detail sits beside the dial;
on a narrower one it goes under it, and on a short one the dial steps aside
for a single meter rather than squeezing the table.

An empty table says which empty it is: the room was quiet, or nothing was
listening. The two look identical as a blank table, and only one of them
is good news.

**An extended advertiser** is counted from its auxiliary packet, the one
that carries its address and its name, because its `ADV_EXT_IND` names
nobody. So it appears once one of its pointers has been followed to a
channel in view ([extended advertising](#extended-advertising)), its types
say `extended`, and its interval stays unknown: an auxiliary packet comes
when its primary says, not on an interval of its own.

## Advertising · `LE 2`

Real BLE advertising packets, CRC-checked, as they arrive. The radio has to
be on an advertising channel; the header says which channel the decoder has,
and whether it is one ([the header](net-reading.md#the-header)).

### The packet list *(focus `v`)*

Newest first:

| Column | What it is |
|--------|------------|
| `CH` | The channel it was heard on |
| `TYPE` | The PDU type: `ADV_IND`, `SCAN_REQ`, `CONNECT_IND` and the rest |
| `ADDRESS` | The advertiser's address, as `i` shows it |
| `NAME` | The name it advertised, if any; in **masked**, only its length |
| `ATYP` | `pub` or `rnd`, as the header's TxAdd bit says |
| `LEN` | The PDU's length in bytes |
| `CRC` | `ok` or `bad`. A failed packet is listed so the failures can be seen, but its name is not read and it counts for no device in the Census |
| `SNR` | Its SNR, read as below |
| `CFO` / `PPM` | Its carrier offset in kHz and in ppm |
| `AGE` | How long ago it arrived |

A name is only read from a packet whose CRC passed, and is made safe to
print: a control character a device puts in its name shows as a replacement
mark (`�`), never as a command to your terminal. If a speaker out there
ever names itself with an escape sequence, it will not be your terminal
that finds out what it does.

| Key | What it does |
|-----|--------------|
| `Enter` | Narrows the list to the selected packet's address, and back. **[FILTERED]** says the list is not everything. Selecting a device in the Census and coming here narrows it for you. On a `CONNECT_IND`, opens that connection on [LE 3](net-le-connection.md) |
| `t` | Narrows to one kind, then the next, then every kind again: **CONNECT** (a connection being set up), **SCAN** (`SCAN_REQ` and `SCAN_RSP`) or **ADV** (the advertising that is nearly everything else). It works with `Enter`, so one device's CONNECTs are two keys away |
| `H` | Holds the list still; the title counts what has arrived since |

The list keeps the newest 200 of *each* kind, not 200 overall, so a lone
`CONNECT_IND` is still there after the advertising flood that followed it.

In SURVEY the line under the list gives each advertising channel's packet
count and CRC pass rate; in LOCK, the one channel's.

**The list hears LE 1M**, and the frame says so. There used to be a key for
LE 2M, and it has gone on purpose. The decoder listens for the advertising
access address, so parked on a data channel it hears secondary advertising
and never a connection, whose packets carry an address of their own. LE 2M
lives almost entirely in connections and never on the three advertising
channels, so that key was either refused or listened very carefully to
nothing. LE 2M is back where it lives: a connection followed on
[LE 3](net-le-connection.md) that moves to LE 2M is received on it from the
moment the two devices agreed, at a sample rate that is a multiple of
8 Msps (8 or 16). At 20 Msps there is no LE 2M receiver, and its events say
**no receiver** rather than pretend to have listened.

### Extended advertising

An advert too big for the three crowded channels leaves a forwarding
address instead:

```
channel 37, 38 or 39               a data channel, a little later
ADV_EXT_IND ── AuxPtr ──────────▶  AUX_ADV_IND
(names nobody)                      (the address, the name, the payload)
```

sdrtop follows the pointer as it does on [LE Coded](net-le-coded.md): when
the channel it names is in view, the auxiliary packet joins the list, named
and addressed, under its own channel. One sent on LE 2M is not followed at
20 Msps, and says why: there is no LE 2M receiver at that rate. A retune,
the survey's or yours, drops whatever was waiting for its window, and says
that too.

### Packet detail

The selected packet, spelled out:

| Section | What it holds |
|---------|---------------|
| **Packet** | Type, channel, length, PHY, CRC, addresses, the ChSel bit |
| **Extended** | For an extended advertising packet: the event and set it belongs to, its TxPower, the packet that pointed at it, and what became of its own AuxPtr, worded as on LE Coded |
| **Advertised** | Every structure it carried: flags, name, TX power, service UUIDs, service data, manufacturer data with the company named, and anything malformed, at the octet where it stopped making sense |
| **Connection** | For a `CONNECT_IND`: the parameters the two devices agreed, and the first channels the connection will hop to |
| **Physics** | SNR, the carrier offset in kHz and ppm, and the carrier at the start and the end of the packet |
| **Modulation** | Index, deviation and drift against the specification's limits |

**Connection.** The agreed interval, latency, timeout, channel map and
sleep-clock accuracy, and the first channels the connection will hop to,
worked out with Channel Selection Algorithm #1 or #2 and marked
**predicted, not followed**: they come from the parameters, not from
hearing the hops. Which algorithm applies takes both ChSel bits, this
packet's and the one of the advertising packet it answered; with that one
not heard, the detail says it does not know rather than pick. A connection
being followed says so here, and `Enter` on a `CONNECT_IND` in the list
opens it on LE 3.

**Physics.** The start of the packet is the preamble's mean frequency (the
test suite's f0), the end the last ten bits before the CRC.

<a id="how-snr-is-read"></a>**How the SNR is read.** In LE 1M's band, as the
packet's power over the noise in a quiet stretch just before it, 130 to
30 µs before (a transmitter's carrier is often up before its preamble),
with the radio's DC offset measured there and taken out of both. It is the
same scale for LE 1M and LE Coded. A packet heard straight after another,
with no quiet stretch before it, shows no SNR rather than a low one. LE
2M's is still the receiver's own estimate. Before this, the envelope was
read, and that counted anything that moves it as noise: a phone 35 dB above
the noise read 18 at the tuned centre, where the radio's DC sits under the
packet. An SNR that modest, for a phone that close, should have been a
clue sooner.

**Modulation.** Each reading is drawn as a bar with its limit marked, and
read from the packet's own symbols. A packet whose CRC failed is not
measured, because which symbols were ones decides where deviation is read.

`df2 avg` is the average deviation the packet reached while alternating,
held against the specification's 185 kHz floor (370 on LE 2M). The floor is
on the *minimum*, and both obvious ways of reading one from live traffic
measured the noise instead: the largest reading could never fail, and the
smallest nearly always did. A transmitter's alternating peaks are all the
same peak, so the average stands in for them. That makes an average under
the floor a real finding, and one over it not a promise.

Both deviations are read as the test suite defines them, from whatever bits
the packet carried, the way [a classic piconet's are](net-classic-bench.md#modulation).
On LE 1M the packet is taken again from the raw samples and timed from the
access address; on LE 2M it is read through the receiver's own filter.

The **drift** rows are the test suite's too: the carrier is read over every
ten bits of the PDU, `drift` is the block furthest from the start, and
`drift rate` the steepest change over five blocks (50 µs on LE 1M). The
suite sends a `1010` for this, whose ten bits average to the carrier;
ordinary traffic does not balance like that, so each bit's own modulation,
as the packet itself shows it, comes out first. Both are maxima, and a
maximum of noisy blocks finds the noise too. With nothing drifting at all,
noise alone reads about this much:

| SNR | Drift from noise alone | Drift rate from noise alone |
|-----|------------------------|-----------------------------|
| 40 dB | about 1 kHz | about 20 Hz/µs |
| 30 dB | about 3 kHz | about 50 Hz/µs |
| 20 dB | about 9 kHz | about 150 Hz/µs |

The `±` says how noisy the blocks were, and past the panel's resolution the
row shows a dash rather than a drift the transmitter may not have. On LE 2M
the preamble is not kept, so the first block stands in for the start.

### The error curve

With **no packet selected**, the detail shows the session's **frame error
rate against SNR**: for each 2 dB of SNR, the share of packets that failed
their CRC, with its uncertainty. A bin with fewer than ten packets gives its
count and no rate, since three packets make an anecdote, not a rate.
Filtered to one address, it is that device's own curve. A receiver whose
failures do not fall as SNR rises is failing for a reason that is not
noise, and this curve is where that shows.

---

← [The NET section](net.md)
