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

It needs a radio that reaches the band and can sample fast enough for its
cheapest mode. On one that cannot, the views are still in the menu, greyed,
and the menu's card says why in one sentence; they just do not open.

## The views

In the menu, NET is **Lab › Bluetooth › 2.4 GHz**: four groups, the band's
overview first, each with its own numbers. The number is the key to press
while you are in that group, so `Classic 2` is Classic's second view.
**Lab › Wi-Fi › 2.4 GHz** sits beside it, and its first group, Channel, has
the channel survey, Frames and Airtime. Both need a radio that reaches the
band; Wi-Fi also needs 20 Msps, a whole Wi-Fi channel, and is greyed, with
the reason, on a radio that cannot sample that fast.

```
Bluetooth › 2.4 GHz
  Overview     1 Radio check       2 Bluetooth survey
  Low Energy   1 LE devices        2 Advertising        3 One LE link
  Classic      1 Piconets nearby   2 Packets            3 Both ends
  Long range   1 Long-range devices
Wi-Fi › 2.4 GHz
  Channel      1 Channel survey    2 Frames             3 Airtime
```

Twelve views, one question each:

| Key | View | The question it answers |
|-----|------|--------------------------|
| `Overview 1` | **[Radio check](net-band.md#radio-check--overview-1)** | What can this radio reach and receive here? |
| `Overview 2` | **[Bluetooth survey](net-band.md#bluetooth-survey--overview-2)** | What is in this band, and who is spending the airtime? |
| `Low Energy 1` | **[LE devices](net-le.md#le-devices--low-energy-1-focus-u)** | Who is here? |
| `Low Energy 2` | **[Advertising](net-le.md#advertising--low-energy-2)** | What is this device advertising, and is its transmitter any good? |
| `Low Energy 3` | **[One LE link](net-le-connection.md)** | What are two connected devices saying to each other, event by event? |
| `Classic 1` | **[Piconets nearby](net-classic.md)** | Who is running a classic Bluetooth piconet near me? |
| `Classic 2` | **[Packets](net-classic-packets.md)** | What is this one piconet saying, packet by packet? |
| `Classic 3` | **[Both ends](net-classic-bench.md)** | How good is each end of it, side by side? |
| `Long range 1` | **[Long-range devices](net-le-coded.md)** | Who advertises on LE Coded, and where did each advertisement point? |
| `Channel 1` | **[Channel survey](net-wifi.md)** | Which Wi-Fi channel is busy, and which is quiet? |
| `Channel 2` | **[Frames](net-wifi-frames.md)** | What is on this channel, frame by frame? |
| `Channel 3` | **[Airtime](net-wifi-airtime.md)** | What is spending this channel's time? |

The panels with controls announce them with a highlighted letter in the
title; the full list is in [Keyboard Shortcuts](keys.md#net-panel-focus-modes).
The menu's card has a live line for every NET view, built by the same rules
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

1. **`Overview 1`**, Radio check, to see whether the radio can do this at all. If it says
   `OUT OF BAND`, the rest of this page is reading for pleasure.
2. **`Overview 2`**, and let the Bluetooth survey run for a minute: the room's Wi-Fi, its
   Bluetooth, and whatever else shares 2.4 GHz with them.
3. **`Low Energy 1`**, LE devices, for who is advertising. Expect phones, watches, and a few
   things you forgot were wireless.
4. **`Classic 1`**, with a pair of headphones playing, then `Enter` for its
   packets and `3` for both ends side by side.

## If you wrote your own NET preset

Three panels changed name or went away while this section took its current
shape. A preset of your own that names them needs updating:

| Was | Now |
|-----|-----|
| `net_ble_rf` | `net_ble_detail` (the packet detail) |
| `net_bt_census` | `net_bt_piconets` (the piconet roster) |
| the `net_coexist` preset | part of `net_survey`; the `net_coexist` panel itself is unchanged |

The built-in views moved too: the census (now LE devices) and the BLE view
(now Advertising) went from `section = "net"` to `"le"`, and the three
classic views to `"classic"`. A preset of yours still filed under `"net"`
keeps working: that is the band's id, which the menu titles **Overview**,
under Bluetooth. Move it with `section` and `slot` if you want it beside its
kind. On a radio that cannot reach the band, all of these are greyed
together, with the reason, and none of them opens.

How presets are written is in [Layout presets](presets.md).

---

← [Back](README.md)
