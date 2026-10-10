# What a NET number is worth

← [The NET section](net.md)

Two questions sit behind every figure in NET: what is it measured
*against*, and has the code that measured it ever met a real transmitter?
This page answers both, so the rest of the pages do not have to keep
clearing their throat.

## What an offset is worth

Every frequency offset and ppm figure contains our own oscillator's error.
An SDR's crystal is good for its price, which is a carefully worded way of
saying some ppm off, and at 2.4 GHz every ppm is 2.4 kHz. So the panels that show an offset carry a tag that says what it is worth:

| Tag | Corrected by | Good for |
|-----|--------------|----------|
| **[RELATIVE]** | Nothing: measured against our own oscillator | Comparing devices with each other, not saying how far off one is |
| **[REFERENCED]** | A device you told sdrtop to trust: `T` in [the Census](net-le.md#census--le-1-focus-u), with how far its crystal can be off, in ppm | As good as your word, and every panel and export names it as "user-stated" |
| **[TRACEABLE]** | A standard station: tune to one (WWV at 2.5, 5, 10, 15 or 20 MHz) and press `y`; sdrtop measures its carrier and takes our error out of every reading at once | Saying how far off a device actually is |

A reference expires after fifteen minutes, because oscillators drift, and
the tag then says so. A trusted device you regret trusting does not have to
wait that long: `T` on it again lets it go, and every tag reads
**[RELATIVE]** at once. `T` on a different device replaces it. A station
reference just expires, or a fresh `y` renews it.

## Reasoned, not verified

Some of what NET measures has been checked against a real transmitter, and
some has only been written from the specification and tested on synthetic
signals. Both are tested; only one has met the air. Where it matters, the
screen says which.

**Limits are cited where they are drawn.** A limit read from the Bluetooth
Core Specification names its section beside it: `Core 5.4 Vol 2 A 3.1.1`
over the classic modulation rows, and for BLE the modulation index and the
deviations from `Core 5.4 Vol 6 A 3.1`, the drift and its rate from `3.3`,
on LE 1M and LE 2M alike.

**What has met the air**, and what has not yet:

| What | On the air | With what |
|------|------------|-----------|
| Classic header decode | checked | A phone playing music to a pair of headphones, both addresses read off the devices themselves |
| Classic UAP narrowing | checked | The same pair: two candidates within half a minute |
| Classic DM1 payload and its CRC | checked | A reconnect's link-manager messages settled both UAPs, to exactly the devices' own |
| Classic LMP messages, by name | checked | `features_req_ext` and `features_res` read off the same pair |
| Classic DH payloads, DM3 and DM5 | **not yet** | None arrived unencrypted and in basic rate |
| BLE connection following | checked | A TV box and its remote, Algorithm #1, see [LE 3](net-le-connection.md#on-the-air) |
| LE Coded S=8 | checked | A phone advertising from nRF Connect, see [LE Coded](net-le-coded.md#on-the-air) |
| LE Coded S=2 | **not yet** | Decoded from packets built to the Core; the phone only sends S=8 |

The classic header decode is a port of `libbtbb`, and its section is headed
"libbtbb port, checked on air". For a long time it said "unchecked": it had
passed every test I could write and never met a real transmitter, which is
a bit like a driving instructor who has read every manual. Then it met two.
The LAPs matched, the UAPs narrowed, and a reconnect's DM1 packets, the few
link-manager messages sent before the link encrypts, passed their CRC.
Their contents made sense too, down to the encryption request right before
the CRCs stopped passing.

**Three things that look like faults and are not:**

- **An encrypted link stays at `2 left`.** The payload is encrypted, so no
  CRC can be checked without the key, and the true UAP is always one of the
  two. It resolves from the few unencrypted packets a connection sends while
  it is being set up, so switching the headphones off and on while sdrtop
  listens is the quickest way to a value.
- **A header's packet type shows every packet its code can be**, like
  `DM3/2-DH3`. The same code means one packet on a basic-rate link and
  another once the link has switched to EDR, which most audio links do, and
  the header alone cannot say which. Your headphones' music is the second
  name. The export's `packet_type` column says the same.
- **"predicted, not followed"** beside a `CONNECT_IND` in the packet detail
  means the channels there were worked out from its parameters. The
  connection itself is followed on [LE 3](net-le-connection.md), through
  the events whose channels the radio's window holds, and only those.

---

← [The NET section](net.md)
