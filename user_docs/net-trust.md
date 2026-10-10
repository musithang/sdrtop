# What a NET number is worth

← [The NET section](net.md)

## What an offset is worth

Every frequency offset and ppm figure contains our own oscillator's error.
This is humbling, and it is also why the panels that show one carry a tag
that says what it is worth:

- **[RELATIVE]**: measured against our own oscillator, uncorrected. Good for
  comparing devices with each other, not for saying how far off one is.
- **[REFERENCED]**: corrected by a device you told sdrtop to trust (`T` in
  the Census, with how far its crystal can be off, in ppm). It rests on your
  word, and every panel and export names it as "user-stated".
- **[TRACEABLE]**: corrected against a standard station. Tune to one (WWV at
  2.5, 5, 10, 15 or 20 MHz) and press `y`; sdrtop measures its carrier and
  takes our error out of every reading at once.

A reference expires after fifteen minutes, because oscillators drift, and the
tag then says so. A trusted device you regret trusting does not have to wait
that long: `T` on it again lets it go, and every tag reads **[RELATIVE]** at
once. `T` on a different device replaces it. A station reference just
expires, or a fresh `y` renews it.

## Reasoned, not verified

Some of what NET measures has been checked against a real transmitter and
some has only been written from the specification and tested on synthetic
signals. Where it matters, the screen says which:

- A limit **read from the Bluetooth Core Specification** names its section,
  for example `Core 5.4 Vol 2 A 3.1.1` beside the classic modulation rows.
- BLE's limits are all read from the Core Specification: the modulation
  index and the deviations from `Core 5.4 Vol 6 A 3.1`, the drift and the
  drift rate from `3.3`, on LE 1M and LE 2M alike. The section heading
  names both.
- The classic Bluetooth header decode is a port of `libbtbb`, and its
  section is headed "libbtbb port, checked on air". For a long time it said
  "unchecked": it had passed every test I could write and never met a real
  transmitter. Then it met two, a phone playing music to a pair of
  headphones, with both addresses read off the devices themselves. The
  LAPs matched, and the UAPs narrowed to two candidates within half a
  minute. Then a reconnect's DM1 packets, the few link-manager messages
  sent before the link encrypts, passed their CRC and settled both, to
  exactly the devices' own. Their contents made sense too, down to the
  encryption request right before the CRCs stopped passing. Checked on the
  air: the header decode, the UAP narrowing and the DM1 payload. Not yet:
  DH payloads and DM3/DM5, because none arrived unencrypted and in basic
  rate.
- **An encrypted link stays at `2 left`**, and that is not a fault: the
  payload is encrypted, so no CRC can be checked without the key, and the
  true UAP is always one of the two. It resolves from the few unencrypted
  packets a connection sends while it is being set up, so switching the
  headphones off and on while sdrtop listens is the quickest way to a
  value.
- **A header's packet type shows every packet its code can be**, like
  `DM3/2-DH3`: the same code means one packet on a basic-rate link and
  another once the link has switched to EDR, which most audio links do,
  and the header alone cannot say which. Your headphones' music is the
  second name. The export's `packet_type` column says the same.
- **"predicted, not followed"** beside a `CONNECT_IND` in the packet detail
  means the channels there were worked out from its parameters. The
  connection itself is followed on [LE 3](net-le-connection.md), through
  the events whose channels the radio's window holds, and only those.

---

← [The NET section](net.md)
