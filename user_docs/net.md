# The NET Section

← [Back](README.md)

**NET** is sdrtop's look at the 2.4 GHz band: who is on the air, what they
are saying, and how well their transmitters do it. It listens to Bluetooth
Low Energy advertising and to classic Bluetooth, and it measures the band
itself. It never transmits, never joins a connection (it follows the ones
it hears being set up, from the sidelines), and never plays audio: the guest
at the party who says nothing all evening and leaves knowing everyone's
address.

The section only appears on a radio that reaches the band and can sample fast
enough for its cheapest mode. On one that cannot, the section is hidden and
the log says why in one line.

It lives in four sections of the menu: **NET** for the band itself, **LE**
for Bluetooth Low Energy, **Classic** for classic Bluetooth and **LE Coded**
for BLE's long-range PHY. They come and go together, because one radio
requirement admits all four, and the pages on reading a screen hold in all
of them. Nine views, one question each; the key is the number to press while
that section is active:

| Key | View | The question it answers |
|-----|------|--------------------------|
| `NET 1` | **[Capability](net-band.md#capability--net-1)** | What can this radio reach and receive here? |
| `NET 2` | **[Survey](net-band.md#survey--net-2)** | What is in this band, and who is spending the airtime? |
| `LE 1` | **[Census](net-le.md#census--le-1-focus-u)** | Who is here? |
| `LE 2` | **[Advertising](net-le.md#advertising--le-2)** | What is this device advertising, and is its transmitter any good? |
| `LE 3` | **[Connection](net-le-connection.md)** | What are two connected devices saying to each other, event by event? |
| `Classic 1` | **[Piconets](net-classic.md)** | Who is running a classic Bluetooth piconet near me? |
| `Classic 2` | **[Packets](net-classic-packets.md)** | What is this one piconet saying, packet by packet? |
| `Classic 3` | **[Bench](net-classic-bench.md)** | How good is each end of it, side by side? |
| `LE Coded 1` | **[Long range](net-le-coded.md)** | Who advertises on LE Coded, and where did each advertisement point? |

The panels with controls announce them with a highlighted letter in the
title; the full list is in [Keyboard Shortcuts](keys.md#net-panel-focus-modes).
The menu shows a live line under every NET view, built by the same rules the
view itself uses, so it never promises more than the screen delivers.

Three pages hold for every view, and are worth a read before any of them:

- **[Reading any NET screen](net-reading.md)**: survey or lock, the header,
  the three silences, stale and feed loss, and how addresses are shown (`i`).
- **[What a NET number is worth](net-trust.md)**: what an offset in ppm
  rests on, and what has been checked on the air against what has only
  been reasoned.
- **[Taking the data away](net-export.md)**: `o`, and the files it writes.

---

## If you wrote your own NET preset

Three panels changed name or went away while this section took its current
shape. A preset of your own that names them needs updating:

| Was | Now |
|-----|-----|
| `net_ble_rf` | `net_ble_detail` (the packet detail) |
| `net_bt_census` | `net_bt_piconets` (the piconet roster) |
| the `net_coexist` preset | part of `net_survey`; the `net_coexist` panel itself is unchanged |

The built-in views moved too: Census and the BLE view (now Advertising)
went from `section = "net"` to `"le"`, the three classic views to
`"classic"`. A preset of yours still filed under `"net"` keeps working and
stays in NET; move it with `section` and `slot` if you want it beside its
kind. All three sections are hidden together on a radio that cannot reach
the band.

How presets are written is in [Layout presets](presets.md).

---

← [Back](README.md)
