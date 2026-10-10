# Taking the data away (`o`)

← [The NET section](net.md)

A screen is for looking; a file is for arguing about later. `o` anywhere in
NET writes six files, one for each record the section keeps:

| File | One row per |
|------|-------------|
| `net-band-*.csv` | Megahertz of the band: duty, its uncertainty, coverage, power. From NET 2 only: elsewhere the band is not being measured, and the file says so instead |
| `net-census-*.csv` | Counted device, in the order the Census shows them |
| `net-ble-*.csv` | Packet, in the list's order, as it was shown (held or filtered) |
| `net-fer-*.csv` | SNR bin of the frame error curve, for all traffic and for each device |
| `net-bt-*.csv` | Classic hit, oldest first, with its slot residual and header; `lap_kind` says `piconet`, `paged`, or the inquiry code (`GIAC`, `LIAC`, `DIAC`) |
| `net-coded-*.csv` | LE Coded packet, in the same columns as `net-ble`; its `df1` and carrier columns are the LE Coded tests' (S=8 only), and its `note` says so |

The classic file holds the hits and their headers. The [link manager's
messages](net-classic-packets.md#the-link-managers-messages) are on screen
only.

## Where they go

To `~/.local/share/sdrtop/`, or `$XDG_DATA_HOME/sdrtop/` where that is set,
each named with the second it was taken (`net-bt-20261010-204107.csv`). A
second export in the same second is refused rather than overwriting the
first. If you are exporting twice a second, it was probably the first one
you wanted.

## What every file says about itself

A column of ppm values, six months later, with no record of which radio
measured them, in which mode and against what reference, is not data. It
is a set of numbers someone will misread. So every file opens with the same
header of `#` lines, which a spreadsheet, gnuplot and `pandas.read_csv` all
skip by default. One, written from a test's radio:

```
# sdrtop 0.6.5
# exported     2026-10-10T20:41:07Z
# device       HackRF One  serial 0000000000000000
# tuning       2426.000 MHz   20.000 Msps   8 bit  fs=128
# mode         NET / SURVEY
# addresses    full
# reference    unreferenced, no reference established
# session      00:00:00
```

| Line | What it records |
|------|-----------------|
| `sdrtop` | The version that wrote it |
| `exported` | When, in UTC |
| `device` | The radio and its serial |
| `tuning` | Frequency, sample rate, sample width and full scale |
| `mode` | SURVEY or LOCK |
| `addresses` | How the addresses below are shown ([`i`](net-reading.md#addresses-i)), with the registry dates when names can appear |
| `reference` | What the offsets are worth ([the tags](net-trust.md#what-an-offset-is-worth)) |
| `session` | How long the stream has run |

Below the header, a `note` line says what the file is short of, or, when it
has no rows, which of [the silences](net-reading.md#the-three-silences) it
is.

## What a row says

A reading with an uncertainty is two columns, the value and its sigma. A
field the screen would show as a dash is **blank** in the file, never a
zero: a zero is a measurement, and a blank is the honest absence of one.

---

← [The NET section](net.md)
