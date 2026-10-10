# Long range · `LE Coded 1` *(focus `v`)*

← [The NET section](net.md)

**LE Coded** is the PHY BLE uses to reach further. Every bit goes out as
several symbols, wrapped in a convolutional code, so a receiver can get the
packet back from a signal LE 1M would lose:

| Scheme | Symbols per bit | Data rate |
|--------|-----------------|-----------|
| `S8` | 8 | 125 kbit/s |
| `S2` | 2 | 500 kbit/s |

The price is time. An advertisement that takes 0.4 ms on LE 1M takes over
a millisecond here, and a full one 17. Long range is the same sentence
spoken very slowly and very clearly, which works across a garden and is
exhausting in a meeting.

The LE 1M receiver cannot hear any of it (a different preamble, and every
bit coded), and running both on every block costs more than either is
worth. So LE Coded has a section and a receiver of its own, which runs in
LE 1M's place while this view is open. Like the Advertising view, it wants
the radio on an advertising channel, at a sample rate that is a whole
multiple of 4 Msps. Opening it while locked off the advertising channels
moves the radio onto the nearest one, once, and says so; the list says
when either condition is not met.

**Why most rows name nobody.** LE Coded advertises only [the extended
way](net-le.md#extended-advertising): an `ADV_EXT_IND` on the advertising
channel says almost nothing, not even who sent it, and its **AuxPtr**
points at an `AUX_ADV_IND` on one of the 37 data channels a few
milliseconds later, which carries the address, the name and the rest.
sdrtop follows the pointer: when that channel is in the radio's view it
listens there at the promised moment, and the packet it hears joins the
list under its own channel, named and addressed. The primary rows stay
anonymous, as the advertiser sent them.

## The packet list

Newest first, in [the Advertising list's
columns](net-le.md#the-packet-list-focus-v). The `TYPE` says what the packet
is and its scheme (`ADV_EXT_IND S8`, `AUX_ADV_IND S8`). `AUX_ADV_IND` and
`AUX_CHAIN_IND` share one type code with `ADV_EXT_IND`, and are told apart by
where they were heard. `↑↓` selects a packet, `H` holds the list.

## Packet detail

| Row | What it says |
|-----|--------------|
| `type` | The packet and its scheme |
| `packet` | Its channel, the CRC, and how many symbols the code had to repair (`FEC repaired 2 symbols`): what it took to get the bits, beside the bits. A strong packet needs none |
| event, from, TxPower | The advertising mode, the set (SID) and event (DID) it belongs to; the advertiser and its name; the power it states, if it states one |
| `aux` | What became of its AuxPtr, below |
| `pointed` | On an auxiliary packet instead: which `ADV_EXT_IND` pointed at it (`from the ADV_EXT_IND on ch 38`) |
| `SNR` | Read as on LE 1M, see [how the SNR is read](net-le.md#how-snr-is-read) |
| `f0` | The carrier at the start, from the preamble, from the channel's centre |
| the bars | On S=8 only, the transmitter against the LE Coded tests, below |
| `payload` | The bytes, as sent |

**What became of the AuxPtr**, each said as it happened:

| It says | Which means |
|---------|-------------|
| `heard 1.23 ms later` | Found, on the scheme it actually came in |
| `not in the radio's view` | The channel is outside the stretch of band the radio sees. That is most of the 37, so this is the common one |
| `listened, not heard` | In view, listened to at the promised moment, nothing there |
| `its samples were not held` | The feed lost the stretch where it would have been (see [stale, and feed loss](net-reading.md#stale-and-feed-loss)) |
| `waiting for its window` | The promised moment has not come yet |
| `no auxiliary packet promised` | The `ADV_EXT_IND` did not point anywhere |
| `aux not followed: …` | With why: an LE 2M aux at 20 Msps, for one, since there is no LE 2M receiver at that rate |

**The S=8 bench.** The average deviation (`df1 avg`, 225 to 275 kHz), the
share of symbols above 185 kHz (the suite asks 99.9 %), and the drift and
drift rate through the packet, each a bar with its limit, as on [the
Advertising detail](net-le.md#packet-detail). **S=2 has none of these**:
the test suite defines its LE Coded measurements on S=8 only, so the detail
says `not defined for S=2` rather than hold S=2 to a limit nobody wrote.

With **no packet selected**, the detail gives the session's account: how
many LE Coded packets were heard, the decoder's triggers and how each ended
(`CRC ok`, `CRC failed`, `gave up`), and every AuxPtr by how it ended.

## Load

The coded receiver is the most expensive thing in NET, and at 20 Msps on a
small machine it can cost more than real time. When it does, the header's
`decode` goes past 100 %, the list wears `[FEED LOSS]`, and the AuxPtr
account fills with lost samples. Those are honest counts of what was
missed, not of what was there: an empty patch of band and a patch nobody
listened to are different things, and the screen keeps them apart.

## On the air

**On the desk.** A phone advertising on LE Coded S=8 from nRF Connect,
14.5 s recorded at 2426 MHz and read back with nothing dropped:

| | |
|-|-|
| `ADV_EXT_IND`s on channel 38 | 49, every one through its CRC, no symbol repaired |
| Pointing at a channel in view (8 to 14) | 20, and all 20 auxiliary packets heard, each naming the phone |
| Pointing out of view | 29 |
| The phone's carrier | about 1 kHz high (0.5 ppm) |
| Its deviation | 250.6 kHz |

S=2 is decoded from packets built to the Core, and has not been heard on
the air yet: the phone only sends S=8. Until something sends S=2 within
reach, that half is reasoned rather than checked.

**Two rooms away.** The same phone in the kitchen, 26.5 s at 2480 MHz and
8 Msps: 76 of its `ADV_EXT_IND`s through their CRC, at SNRs from 8.5 down
to -1.6 dB. Until sdrtop took the radio's own DC offset off the samples,
it heard none of them. That offset sits at the tuned frequency, exactly
under every advertising packet in LOCK, and when it is stronger than the
packet, the frequency a receiver reads falls apart, coding gain or not. It
is now measured from the quiet stretches of the stream and taken off
before any receiver sees a sample, which also let half again as many LE 1M
packets through their CRC on the same recordings. The long-range PHY is
long-range again, which is a low bar, and it now clears it.

---

← [The NET section](net.md)
