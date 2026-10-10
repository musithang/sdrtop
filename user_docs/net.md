# The NET Section

← [Back](README.md)

**NET** is sdrtop's look at the 2.4 GHz band: who is on the air, what they
are saying, and how well their transmitters do it. It listens to Bluetooth
Low Energy and to classic Bluetooth, and it measures the band itself. It
never transmits, never joins a connection (it follows the ones it hears
being set up, from the sidelines), and never plays audio: the guest at the
party who says nothing all evening and leaves knowing everyone's address.

Decoding is the easy half. Every packet is then treated like any other
transmitter on the bench: modulation index, drift, crystal error, timing,
each with its uncertainty and held against the specification's limits. A
device list with extra steps would have been quicker to write, and much
less interesting to read.

The section only appears on a radio that reaches the band and can sample
fast enough for its cheapest mode. On one that cannot, the section is
hidden and the log says why in one line.

## The views

NET lives in four sections of the menu, which come and go together because
one radio requirement admits all four. The number is the key to press while
that section is active:

```
NET        1 Capability    2 Survey
LE         1 Census        2 Advertising    3 Connection
Classic    1 Piconets      2 Packets        3 Bench
LE Coded   1 Long range
```

Nine views, one question each:

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
The menu shows a live line under every NET view, built by the same rules
the view itself uses, so it never promises more than the screen delivers.

## Before any of them

Three pages hold for every view, and are worth a read first:

- **[Reading any NET screen](net-reading.md)**: survey or lock, the header,
  the three silences, stale and feed loss, and how addresses are shown
  (`i`).
- **[What a NET number is worth](net-trust.md)**: what an offset in ppm
  rests on, and what has been checked on the air against what has only
  been reasoned.
- **[Taking the data away](net-export.md)**: `o`, and the files it writes.

## A first evening

If you have never opened NET before, this order wastes the least time:

1. **`NET 1`**, to see whether the radio can do this at all. If it says
   `OUT OF BAND`, the rest of this page is reading for pleasure.
2. **`NET 2`**, and let the survey run for a minute: the room's Wi-Fi, its
   Bluetooth, and whatever else shares 2.4 GHz with them.
3. **`LE 1`**, for who is advertising. Expect phones, watches, and a few
   things you forgot were wireless.
4. **`Classic 1`**, with a pair of headphones playing, then `Enter` for its
   packets and `3` for each end on the bench.

## If you wrote your own NET preset

Three panels changed name or went away while this section took its current
shape. A preset of your own that names them needs updating:

| Was | Now |
|-----|-----|
| `net_ble_rf` | `net_ble_detail` (the packet detail) |
| `net_bt_census` | `net_bt_piconets` (the piconet roster) |
| the `net_coexist` preset | part of `net_survey`; the `net_coexist` panel itself is unchanged |

The built-in views moved too: Census and the BLE view (now Advertising)
went from `section = "net"` to `"le"`, and the three classic views to
`"classic"`. A preset of yours still filed under `"net"` keeps working and
stays in NET; move it with `section` and `slot` if you want it beside its
kind. All of these sections are hidden together on a radio that cannot
reach the band.

How presets are written is in [Layout presets](presets.md).

---

← [Back](README.md)
