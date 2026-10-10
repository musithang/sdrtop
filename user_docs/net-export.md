# Taking the data away (`o`)

← [The NET section](net.md)


`o` anywhere in NET writes six files, one for each record the section keeps:

| File | One row per |
|------|-------------|
| `net-band-*.csv` | megahertz of the band: duty, its uncertainty, coverage, power (from NET 2 only: elsewhere the band is not being measured, and the file says so instead) |
| `net-census-*.csv` | counted device, in the order the Census shows them |
| `net-ble-*.csv` | packet, in the list's order, as it was shown (held or filtered) |
| `net-fer-*.csv` | SNR bin of the frame error curve, for all traffic and each device |
| `net-bt-*.csv` | classic hit, oldest first, with its slot residual and header; `lap_kind` says piconet or inquiry code |
| `net-coded-*.csv` | LE Coded packet, in the same columns as `net-ble`; its `df1` and carrier columns are the LE Coded tests' (S=8 only), and its `note` says so |

They go to `~/.local/share/sdrtop/` (or `$XDG_DATA_HOME/sdrtop/`), named with
the second they were taken, and a second export in the same second is
refused rather than overwriting the first. If you are exporting twice a
second, it was probably the first one you wanted.

Every file opens with the same header of `#` lines: the sdrtop version, when
it was exported, the device and its serial, the tuning, SURVEY or LOCK, how
the addresses are shown (with the registry dates), what the offsets are
worth, and how long the session has run. A `note` line says what the file is
short of, or, when it has no rows, which of the silences it is.

A reading with an uncertainty is two columns, the value and its sigma. A field
the screen would show as a dash is **blank** in the file, never a zero.

---

← [The NET section](net.md)
