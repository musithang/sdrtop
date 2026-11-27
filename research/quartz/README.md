<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../../user_docs/pics/monas/monas-mark-on-dark-512.png">
    <img src="../../user_docs/pics/monas/monas-mark-on-light-512.png" alt="Monas" width="72">
  </picture>
</p>

<h1 align="center">The flaw in the quartz</h1>

<p align="center">
  <b>Every Bluetooth device near you, placed by the error in its crystal.</b><br>
  <sub>Phones change their Bluetooth address to stay anonymous. Their quartz cannot change.</sub>
</p>

<p align="center">
  <img src="pics/quartz.jpg" alt="The quartz page: a new Bluetooth address linked to an old one that went quiet 2 s before, 0.5 sigma apart in quartz, everything else stepped back" width="900">
</p>

## What you are looking at

Every Bluetooth LE device keeps time with a quartz crystal, and no two are
cut exactly alike. Each one transmits a little off frequency, by an amount
that is its own. The page places every device it hears by that error
(across, in parts per million) and by how far it swings its frequency for a
bit (up and down).

Phones change their address every few minutes so they cannot be followed.
When an old address goes quiet and a new one starts on the same spot, the
page draws a line: **NEW ADDRESS, SAME QUARTZ?** The question mark is
honest. How honest is in [METHOD.md](METHOD.md), with a number.

## Try it

You need a HackRF One with a 2.4 GHz antenna, and Python 3. Other radios
that reach 2.4 GHz and run Monas's Bluetooth views (a PlutoSky R2, a
PortaPack H4M in HackRF mode) should work too; I have only tried the HackRF.

1. **Get monasd**, 0.8.2 or newer: the
   [installer](../../README.md) with `--hackrf`, or from the root of this
   repository, `cargo install --locked --path crates/monasd`.
2. **Start it** on a copy of the config here (monasd writes its settings back
   when it stops, so give it a file of its own):

   ```sh
   cp config.toml ~/quartz.toml
   monasd --config ~/quartz.toml --device hackrf --mode net_census
   ```

3. **Start the page** in a second terminal, from this folder, and open
   <http://127.0.0.1:7380/>:

   ```sh
   python3 -I server.py 7380
   ```

The first dots appear within half a minute. Watch your own phone: put it
near the antenna and wait for its address to change.

## If it does not work

- **monasd finds no radio.** It needs libhackrf: the installer's `--hackrf`
  puts it in. If you have your distribution's HackRF tools, `hackrf_info`
  should list the radio.
- **The radio is busy.** One program at a time: close SDR++, GQRX,
  `hackrf_transfer` or anything else that has it open.
- **No dots.** Check the antenna. In a quiet room, more gain helps: in your
  copy of the config, `gain = "LNA=32,VGA=30"`.
- **Windows.** monasd runs on Linux and macOS only.
- **No radio at all.** Open <http://127.0.0.1:7380/?fixture> instead: twenty
  minutes of my flat, anonymised, played back (add `&speed=10` if you are
  impatient). It needs only `server.py`, no monasd.

## Privacy, and the law

**What it reads.** Bluetooth LE advertising, which every device sends in the
clear to anyone in range. The page uses two physical properties of each
packet, its frequency error and its modulation index, and nothing of what the
packet says.

**What it keeps from you.** monasd masks addresses at the source: it answers
"Apple #12", not an address, and a name's length, not the name. The page
draws only a device's maker, cuts the serial number off your radio's name,
stores nothing, and fetches nothing from anywhere else. These are defaults in
open code, not locks. Change them and what you do with it is on you.

**The rules.** In the EU a device's Bluetooth address can be personal data
(see, for example, the Spanish data protection authority's
[guidance on Wi-Fi and Bluetooth tracking](https://www.aepd.es/guides/wi-fi-tracking-technologies-guidance-for-data-controllers.pdf)).
The GDPR leaves purely personal use alone, but the EU Court reads "purely"
narrowly (Ryneš, C-212/13; Buivids, C-345/17): putting other people's
identifiable data online is not personal use. So:

- watch your own devices;
- share a capture only after `anonymize.py` (it keeps no address, no name,
  no serial number and no time of day);
- do not post screenshots in which other people's devices can be told
  apart by more than their maker;
- never use it to follow a person. Tracking someone without their consent
  is against the law nearly everywhere, whatever the tool.

This is not legal advice, and the law differs from country to country.

## In this folder

| File | |
|---|---|
| `index.html`, `server.py`, `config.toml` | The page, the little server that joins it to monasd, the radio's settings |
| `METHOD.md` | How the numbers are made, the rule, and how often it would be wrong |
| `fixture.json` | Twenty minutes of a census, anonymised |
| `capture.py`, `anonymize.py` | How that was made, for your own |
| `chance.py` | The method's number, from any recording |
| `test/` | `node test/static.mjs` and `node test/replay.mjs index.html fixture.json`: PASS or FAIL |
