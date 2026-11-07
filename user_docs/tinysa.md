# tinySA, tinySA Ultra and Ultra+

← [Supported hardware](hardware.md)

---

sdrtop uses the tinySA USB console for calibrated power traces. The backend
supports the basic tinySA and the Ultra family. It was verified on a ZS405
running `tinySA4_v1.4-236-ge5aa115`.

The device supplies swept power readings without IQ samples. sdrtop offers the
spectrum, waterfall, full band sweep and micro sweep layouts. The Options pane
controls scan points, RBW, attenuation, spur removal and external gain. Basic
LOW and Ultra offer `Attenuation (dB)` with `auto` and 0–31 dB choices. Basic
HIGH offers `Coarse attenuation` with `off` and `on` choices. `on` asks the
firmware for frequency-dependent attenuation of roughly 25–40 dB. Ultra models
also expose the external front-end LNA. Scan points are a host-side setting.
The other controls use the matching tinySA console commands.

sdrtop disables output and aborts pending work during startup. It then forces
input mode and applies a safe automatic baseline before restoring `[tinysa]`
settings. Basic HIGH uses a truthful coarse attenuation baseline of `off`.
Recovery restores the active connector's attenuation setting. LOW numeric
attenuation and HIGH coarse attenuation are saved independently.

## Opening it

```sh
sdrtop --device tinysa
sdrtop --device tinysa=/dev/ttyACM2
sdrtop --device 'tinysa=/dev/ttyACM2?input=high'
```

Automatic discovery recognizes the official USB CDC identity: on Linux among
`/dev/ttyACM*`, on a Mac among `/dev/cu.usbmodem*` (there the path looks like
`tinysa=/dev/cu.usbmodem4001`). The explicit form selects a path when several
devices are present or discovery can't see the USB identity. A Basic tinySA uses the persisted
`[tinysa].basic_input` value. The default is LOW from 100 kHz to 350 MHz. HIGH
covers 240 MHz to 959 MHz. The supported firmware defines 959 MHz as the HIGH
limit. `?input=low` or `?input=high` overrides the config at startup. A bare
`--device tinysa` or `tinysa=PATH` uses the config value.

Connector selection happens only during startup. Restart sdrtop to use an edited
config value. One Basic session stays on one connector. Quitting after a Basic
session saves the connector that session used. This includes a CLI override.

Ultra firmware always uses automatic input selection. The Basic connector value
does not affect an Ultra open and remains saved for the next Basic session. An
explicit `?input=low` or `?input=high` is ignored on Ultra. The startup log says
that it was ignored.

## Installing

Nothing to install: the tinySA talks over its USB serial console, so it needs no
radio library at all. On a Mac it is looked for on its `/dev/cu.usbmodem*`
port; nobody has tried that with a real one yet.


---

← [Supported hardware](hardware.md)
