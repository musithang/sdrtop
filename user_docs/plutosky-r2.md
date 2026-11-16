# PlutoSky R2

← [Supported hardware](hardware.md)

---

OpenSourceSDRLab sent this one. It is an AD9361 board in the PlutoSDR family:
two receivers and two transmitters from 70 MHz to 6 GHz, a 12-bit converter,
and a small Linux computer on board that serves the radio over its USB
network (and over Ethernet). sdrtop talks to that server itself, so there is
no library to install.

Everything sdrtop shows about it was measured on the R2 on my desk, firmware
v0.38, mostly on a cable from a HackRF. The numbers and the charts are on their
own page: **[PlutoSky R2, measured](plutosky-r2-measured.md)**.

## At a glance

| | |
|---|---|
| Range | 70 MHz to 6 GHz |
| Sample rates | 2.083, 3 and 4 Msps over USB, each run ten minutes without a lost sample |
| Samples | 12 bits in int16, full scale 2048 |
| Gain | One control, shown in **measured** dB: 1,860 settings measured at 25 frequencies |
| AGC | The AD9361's own, slow or fast attack (`[A]`; the mode in Options) |
| Corrections | The AD9361's quadrature and DC tracking, always on |
| Filter | The analog filter follows the sample rate |
| Transmitter | Silenced when sdrtop opens the radio, unless another program is using it |
| Connection | sdrtop's own client for the radio's server, over its USB network (192.168.2.1) |

A plain `sdrtop` finds it on USB and opens it natively. `--device plutosky`
asks for this backend by name.

## The transmitter

After power-up the R2's transmitter LO runs at 2.4 GHz, as on every PlutoSDR.
Nothing is transmitted, but the LO leaks into the receiver, strongly enough that
at 2.4 GHz a fresh R2 hears mostly itself. So when sdrtop opens the radio it
turns both transmit channels to full attenuation, powers the LO down, and reads
both back. If the radio does not confirm, sdrtop refuses to use it. Nothing is
restored when sdrtop closes; the radio's own defaults come back at its next
power-up.

If another program is transmitting, sdrtop leaves the transmitter alone,
receives beside it, and the top line of the screen says **TRANSMITTING TOO**.

## The gain

The R2's gain control is called `hardwaregain`, and sdrtop keeps its name.
The chip uses a different gain table in each of three bands (to 1.3 GHz, to
4 GHz, above), and each band starts somewhere else: -1, -3 and -10 dB. sdrtop's
gain setting is the position from the band's bottom, so `↑` and `↓` cover every
band fully, and the gain on screen is the measured gain at the nearest measured
frequency. At 2.4 GHz the top five settings could not be measured, so the gain
stops at 66 dB there rather than show a figure nobody measured.

## Sharing the radio

The R2's server accepts every program that connects, and any of them can
retune it or change its gain. While sdrtop receives, it checks the radio once a
second. Whatever another program changed is taken as it is (sdrtop never writes
its own value back over it) and the log says so:

```
Another program on this radio: tuned to 145.500000 MHz.
```

## Rates and the link

Over USB the R2's network carries about 5.7 Msps at best, so sdrtop offers what
it measured without loss: 2.083, 3 and 4 Msps. The radio itself samples up to
61.44 Msps; Ethernet is the road to those rates, and support for it is coming.

## macOS

Written from the protocol, not yet run on a Mac. macOS does not speak the R2's
default USB network (RNDIS): set `usb_ethernet_mode = ncm` in the
`config.txt` on the radio's USB drive, then eject it and replug the radio.

## Other PlutoSDR boards

The USB id is the one every PlutoSDR shares, so sdrtop asks the board what it
is. Only the R2 (`Analog Devices PlutoSDR Rev.C (Z7020-AD9361)`) opens
natively; other boards are named and left to [SoapySDR](soapysdr.md)
(`--device soapy=driver=plutosdr`), because only the R2 was measured here.

---

← [Supported hardware](hardware.md)
