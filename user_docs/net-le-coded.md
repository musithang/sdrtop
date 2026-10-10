# Long range · `LE Coded 1` *(focus `v`)*

← [The NET section](net.md)


**LE Coded** is the PHY BLE uses to reach further: every bit goes out as
eight symbols (S=8, 125 kbit/s) or two (S=2, 500 kbit/s), wrapped in a
convolutional code, so a receiver can get the packet back from a signal
LE 1M would lose. The price is time: an advertisement that takes 0.4 ms on
LE 1M takes over a millisecond here, and a full one 17.

The LE 1M receiver cannot hear any of it (a different preamble, and every
bit coded), and running both on every block costs more than either is
worth, so LE Coded has a section and a receiver of its own, which runs in
LE 1M's place while this view is open. Like the Advertising view it wants
the radio on an advertising channel, and moves it there if it is not.

**Why most rows name nobody.** LE Coded advertises only the extended way:
an `ADV_EXT_IND` on the advertising channel says almost nothing, not even
who sent it, and points (its **AuxPtr**) at an `AUX_ADV_IND` on one of the
37 data channels a few milliseconds later, which carries the address, the
name and the rest. sdrtop follows the pointer: when that channel is in the
radio's view it listens there at the promised moment, and the packet it
hears joins the list under its own channel, named and addressed. The
primary rows stay anonymous, as the advertiser sent them.

## The packet list

Newest first, in the Advertising list's columns: the TYPE is what the
packet is (`ADV_EXT_IND`, `AUX_ADV_IND`, `AUX_CHAIN_IND`, which share one
type code and are told apart by where they were heard) and the scheme
(`S8` or `S2`). `↑↓` selects a packet, `H` holds the list.

## Packet detail

- **packet**: channel, CRC, and how many symbols the code had to repair:
  what it took to get the bits, beside the bits. A strong packet needs
  none.
- **event**: the advertising mode, and the set (SID) and event (DID) it
  belongs to; **from**, the advertiser and its name; **TxPower**, when it
  states one.
- **aux**: what became of the AuxPtr, each said as it happened:
  - *heard N ms later*, on the scheme it actually came in;
  - *not in the radio's view*: the channel is outside the stretch of band
    the radio sees, which is most of the 37, so this is the common one;
  - *listened, not heard*: in view, listened to, nothing there;
  - *its samples were not held*: the feed lost the stretch where it would
    have been (see [stale, and feed loss](net-reading.md#stale-and-feed-loss));
  - *waiting for its window*; *no auxiliary packet promised*; *not
    followed*, with why (an LE 2M aux at 20 Msps, for one: there is no LE
    2M receiver at that rate).

  An auxiliary packet says which `ADV_EXT_IND` pointed at it instead.
- **SNR** (read as on LE 1M, see [how the SNR is read](net-le.md#how-snr-is-read)),
  and **f0**: the carrier at the start, from the preamble.
- On **S=8**, the transmitter against the LE Coded tests: the average
  deviation (225 to 275 kHz), the share of symbols above 185 kHz (the suite
  asks 99.9 %), and the drift through the packet, each a bar with its limit
  as on the Advertising detail. **S=2 has none of these**: the test suite
  defines its LE Coded measurements on S=8 only, so the detail says so
  rather than holding S=2 to a limit nobody wrote.

With **no packet selected**, the detail gives the session's account: the
decoder's triggers and how each ended, and every AuxPtr by how it ended.

**Load.** The coded receiver is the most expensive thing in NET, and at
20 Msps on a small machine it can cost more than real time. When it does,
the header's `decode` goes past 100 %, the list wears `[FEED LOSS]`, and
the AuxPtr account fills with *samples not held*. Those are honest counts of
what was missed, not of what was there.

**On the air**, a phone advertising on LE Coded S=8 from nRF Connect, 14.5 s
recorded at 2426 MHz and read back with nothing dropped: 49 `ADV_EXT_IND`s
on channel 38, every one through its CRC with no symbol repaired; 20 of
them pointed at a channel in view (8 to 14) and all 20 auxiliary packets
were heard, each naming the phone; the other 29 pointed out of view. The
phone's carrier sat about 1 kHz high (0.5 ppm), its deviation 250.6 kHz.
S=2 is decoded from packets built to the Core, and has not been heard on
the air yet: the phone only sends S=8.

**Two rooms away**, the same phone in the kitchen, 26.5 s at 2480 MHz and
8 Msps: 76 of its `ADV_EXT_IND`s through their CRC, at SNRs from 8.5 down
to -1.6 dB. Until sdrtop took the radio's own DC offset off the samples,
it heard none of them. That offset sits at the tuned frequency, exactly
under every advertising packet in LOCK, and when it is stronger than the
packet the frequency a receiver reads falls apart, coding gain or not. It
is now measured from the quiet stretches of the stream and taken off
before any receiver sees a sample, which also let half again as many LE 1M
packets through their CRC on the same recordings. The long-range PHY is
long-range again.

---

← [The NET section](net.md)
