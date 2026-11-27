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

## Every offset carries your radio's error

The offsets are measured against your own receiver's crystal, which is off
too, by some ppm of its own, and every point on the page moves by that
same amount. So the page's x axis is **relative**, as
[the NET trust page](../../user_docs/net-trust.md#what-an-offset-is-worth)
explains. It cannot say how far off a device is in absolute terms. It does
not need to: "same quartz?" compares two devices heard by the same radio in
the same session, and the radio's error cancels in the difference.

## The rule

Phones and many other devices change their Bluetooth address every few
minutes (a resolvable or non-resolvable private address, RPA or NRPA), so
that the address cannot be used to follow them. The crystal does not change.
The page draws a dashed line from an old address to a new one when all of
this holds:

1. Both addresses are rotating ones (RPA or NRPA).
2. Both carry the same company in their advertising data (or both none).
3. The old one has fallen silent: not heard for over 20 s.
4. The new one started within 30 s of the old one's last packet. A rotation
   is a swap: the old address stops and the new one starts.
5. Their offsets and indices are within 2 combined sigma, on both axes
   together: `hypot(Δppm / σppm, Δindex / σindex) <= 2`, each σ the two
   devices' sigmas combined.

When exactly one old address qualifies, the line is labelled
**NEW ADDRESS, SAME QUARTZ?**. When several do, they are all drawn, fainter,
and the label says how many: **3 CRYSTALS MATCH**. The question mark is
meant. This is evidence, not identification.

## What it does not prove

- Devices of the same model can carry crystals cut close together. Two of
  them swapping addresses in the same half minute would satisfy the rule.
- A device that advertises several sets at once rotates them together, so
  one old address can be followed by several new ones, each linked to it.
- A rotation slower than 30 s, or a device out of range in between, is not
  linked at all.

## How often two different devices meet by chance

Two rotating addresses of the same company that were both heard over a
common stretch of 10 s or more are two transmitters, because a device that
rotates stops the old address before starting the new one. For every such
pair, `chance.py` asks whether the rule's fifth condition would hold.

In the sample recording (`fixture.json`: fifteen minutes, one flat, one
evening, 35 addresses, a HackRF at LNA 32 and VGA 24):

| | Pairs heard together | Within 2 sigma |
|---|---|---|
| All companies | 41 | 1 |
| Apple, Inc. | 39 | 1 |

One in 41, about 2 %. That is a small sample, so read it as a bound, not a
rate: with one hit in 41 tries, the chance per pair is below 13 % at 95 %
confidence (the exact binomial bound). A room with more devices of one model would give a better number,
and possibly a worse one.

The recording carries **[FEED LOSS]** almost throughout: at 20 Msps this
machine's Bluetooth decoding let some blocks go. That makes the packet counts
lower bounds; the offset and the index are measured on each packet that did
arrive, and are not moved by the ones that did not.

In the same recording the page found three rotations: two new addresses
each fit exactly one old one (0.5 and 1.97 sigma, 1 s and 0 s after it went
quiet), and one fit two (**2 CRYSTALS MATCH**).

Reproduce it:

```sh
python3 -I chance.py fixture.json
```

Run it on your own capture (see the README) to get your room's number.
