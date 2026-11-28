# The method

How the quartz page gets its two numbers, what its "same quartz?" rule says,
and how often it would say it about two devices that are not the same. If
you only want to run it, the [README](README.md) is enough.

## Two numbers per device

A Bluetooth LE device keeps time with a quartz crystal, and crystals are not
cut exactly alike. The error shows up on the air: the device transmits a
little off the channel's nominal frequency. The amount differs from device to
device, drifts slowly with temperature, and does not change when the device
changes its address.

- **Carrier offset, in ppm.** Monas measures it on every packet that passes
  its CRC and converts it to parts per million of the channel it was heard
  on. Parts per million and not hertz, because a device advertises on three
  channels 78 MHz apart, and the same crystal reads 3 % more hertz on the
  highest than on the lowest; ppm from the three can be combined, hertz
  cannot. The packets are combined with their uncertainties, so the sigma
  shrinks as a device keeps talking.
- **Modulation index.** How far the transmitter swings its frequency for a
  bit, against the symbol rate, from the settled runs of bits in each packet
  that has them. It depends mostly on the radio chip and how it is tuned,
  not on the crystal, so it can separate devices the offset alone would not.

Both come from monasd's census (`/v1/net/census`, in
[the API reference](../../user_docs/api.md)); the terminal shows the same
two as `CFO` and `MOD` in [the Census](../../user_docs/net-le.md).

## Not a new idea

Tracking Bluetooth LE devices by the imperfections of their radios, the
carrier frequency offset among them, was studied at scale by Givehchian and
colleagues at UC San Diego:
[*Evaluating Physical-Layer BLE Location Tracking Attacks on Mobile Devices*](https://wcsng.ucsd.edu/files/sp22_paper.pdf),
IEEE Symposium on Security and Privacy 2022
([IEEE Xplore](https://ieeexplore.ieee.org/document/9833758)). On 647
devices they found it workable but not always reliable: about half had a
fingerprint of their own, temperature and cheap receivers blurred it, and
some devices shared one. This page shows the same effect with an
off-the-shelf radio and two numbers per device, and puts the same limits in
front of the viewer.

## Every offset carries your radio's error

The offsets are measured against your own receiver's crystal, which is off
too, by some ppm of its own, and every point on the page moves by that
same amount. So the page's x axis is **relative**, as
[the NET trust page](../../user_docs/net-trust.md#what-an-offset-is-worth)
explains. It cannot say how far off a device is in absolute terms. It does
not need to: "same quartz?" compares two devices heard by the same radio in
the same session, and the radio's error cancels in the difference, as far as
it stays put. It does not quite: the old address's reading is averaged over
its whole life and the new one's comes up to 30 s later, and an
unreferenced receiver drifts a little with its own temperature in between.
That drift is not in the sigma.

## The rule

Phones and many other devices change their Bluetooth address every few
minutes (a resolvable or non-resolvable private address, RPA or NRPA; some
also change a static random address), so that the address alone is harder
to follow. The crystal's error does not change with the address.
The page draws a dashed line from an old address to a new one when all of
this holds:

1. Both are addresses a device can change: RPA, NRPA or static random.
2. Both carry the same company in their advertising data (or both none).
3. The old one has fallen silent: not heard for over 20 s.
4. The new one started within 30 s of the old one's last packet, or up to
   2 s before it (the census gives times in whole seconds). A change of
   address is a swap: the old address stops and the new one starts.
5. As they were heard at the swap, the old one's last minute against the
   new one's first, their offsets and indices are within 2 combined sigma,
   on both axes together: `hypot(Δppm / σppm, Δindex / σindex) <= 2`, each
   σ the two windows' sigmas combined. A window with fewer than 10 packets
   is no measurement (its sigma would be wide enough to match anything)
   and matches nothing.

**Why a minute, not a whole life.** A crystal drifts: a phone warms in a
pocket, on a charger, in a hand. In the sample recording the long-lived
devices' own offsets moved by a median of about 0.06 ppm over their life,
some by 0.3 to 0.9 ppm, while the census's sigma for each was about 0.01.
Compared over their whole lives, an old address and its successor would sit
many sigma apart for drift alone. Compared over the minute either side of
the swap, the drift has no time to grow, and the sigma is what the packets
of that minute carry. Monas's census gives running means; the page takes
the one before the window out of the one at its end
(`(m₂n₂ − m₁n₁) / (n₂ − n₁)`, its sigma scaled by `√(n₂ / (n₂ − n₁))`).

When exactly one old address qualifies, the line is labelled
**NEW ADDRESS, SAME QUARTZ?**. When several do, they are all drawn, fainter,
and the label says how many: **3 CRYSTALS MATCH**. The question mark is
meant. This is evidence, not identification.

## Several addresses at once

A device can also advertise under several addresses at the same time: a
phone runs more than one advertising set, each with an address of its own.
Nothing goes quiet, so the rule above has nothing to link; the points simply
sit together. The page rings them when:

1. They are addresses a device can change, of the same company, all heard
   now.
2. Every pair was heard together for 10 s or more, and over all the time it
   shared (at least 10 packets each) is within 2 combined sigma.

All the time they shared, not a minute: one device's drift moves both of its
addresses alike, so a longer stretch only narrows the sigma. A group is
grown from the address with the most packets, and a member joins only if it
is close to every one already in, never by a chain. On the ring the page
writes **3 ADDRESSES, ONE QUARTZ?**; pointing at a member says how many
others it was heard with.

## What it does not prove

- Devices of the same model can carry crystals cut close together. Two of
  them swapping addresses in the same half minute would satisfy the rule.
- A device that advertises several sets at once rotates them together, so
  one old address can be followed by several new ones, each linked to it.
- A rotation slower than 30 s, or a device out of range in between, is not
  linked at all.
- Two devices of one model, heard together, can agree within 2 sigma and be
  ringed as one. The ring is the same question as the line, about addresses
  heard at once.

## How often two different devices meet by chance

Two changeable addresses of the same company heard over a common stretch of
10 s or more are taken here as two transmitters, because a device that
changes its address stops the old one before starting the new one. For
every such pair, `chance.py` compares the packets of the last minute they
shared (at most 60 s, at least 10 packets each), as the rule's fifth
condition compares a swap. Some such pairs are one device with two
advertising sets (the ring above is for them), so the count is an upper
bound on chance, not an estimate of it.

In the sample recording (`fixture.json`: fifteen minutes, one flat, one
evening, 35 addresses, a HackRF at LNA 32 and VGA 24):

| | Pairs heard together | Within 2 sigma |
|---|---|---|
| All companies | 105 | 6 |
| Apple, Inc. | 98 | 6 |

Six in 105, about 6 %. That is a small sample, so read it as a bound, not a
rate: with six hits in 105 tries, the chance per pair is below 12 % at 95 %
confidence (the exact binomial bound, 12.0 %). The bound treats the pairs as
independent, and they are not quite: the 105 pairs are made of 31
changeable addresses, each in several. And some of the six may be one device
after all, advertising two sets at once. A room with more devices of one
model would give a better number, and possibly a worse one.

The recording carries **[FEED LOSS]** almost throughout: at 20 Msps this
machine's Bluetooth decoding let some blocks go. That makes the packet counts
lower bounds; the offset and the index are measured on each packet that did
arrive, and are not moved by the ones that did not.

In the same recording the page found eight changes of address: seven new
addresses fit exactly one old one each (from 0.54 to 1.89 sigma, 0 to 1 s
after it went quiet; three of them static random addresses), and one fit two
(**2 CRYSTALS MATCH**). Compared over whole lives instead of the minute at
the swap, it found three. It also ringed addresses heard at once twelve
times over the fifteen minutes, nearly all among a dozen Apple addresses
within 0.4 ppm of each other: a crowd the quartz alone does not sort out.

Reproduce it:

```sh
python3 -I chance.py fixture.json
```

Run it on your own recording (the README says how to make one) to get your
room's number.
