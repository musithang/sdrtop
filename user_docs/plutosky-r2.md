# PlutoSky R2

← [Supported hardware](hardware.md)

---

OpenSourceSDRLab sent me this one. From the outside it looks like a PlutoSDR,
and its software even believes it is one, but it is their own board: a bigger
Zynq (the 7020), a transceiver that says it is an AD9361 and behaves like one,
and Gigabit Ethernet right on the board. Analog Devices' ADALM-PLUTO has a Zynq
7010 and an AD9363, so if that is the radio you own, this page is not about it.

Two receivers and two transmitters, 70 MHz to 6 GHz, 12 bits. A small Linux
computer on the board serves the radio to anyone who connects, over its USB
network or over Ethernet. sdrtop talks to that server itself, so there is
nothing to install.

The R2 runs a build of Analog Devices' own Pluto firmware. That is why it
introduces itself as `Analog Devices PlutoSDR Rev.C (Z7020-AD9361)`, and on
USB even as `PlutoSDR (ADALM-PLUTO)`. sdrtop checks the exact name, and only
this board opens natively.

Everything sdrtop shows about it was measured on the R2 on my desk, firmware
v0.38, mostly on a cable from a HackRF. The numbers and the charts have their
own page: **[PlutoSky R2, measured](plutosky-r2-measured.md)**.

## At a glance

| | |
|---|---|
| Range | 70 MHz to 6 GHz |
| Sample rates | 2.083, 3 and 4 Msps over USB, none of them losing a sample (3 and 4 ran for ten minutes) |
| Samples | 12 bits in int16, full scale 2048 |
| Gain | One control, shown in measured dB (1,860 settings measured at 25 frequencies) |
| AGC | The AD9361's own, slow or fast attack (`[A]`, the mode in Options) |
| Corrections | The AD9361's quadrature and DC tracking, always on |
| Filter | The analog filter follows the sample rate |
| Transmitter | Silenced when sdrtop opens the radio, unless another program is using it |
| Connection | sdrtop's own client for the radio's server, over its USB network (192.168.2.1) |

A plain `sdrtop` finds it on USB and opens it. `--device plutosky` asks for
this backend by name.

## The transmitter

Fresh from power-up, the R2's transmitter LO sits at 2.4 GHz, as on every
PlutoSDR. It sends nothing, but the LO leaks into the receiver, and at 2.4 GHz
a fresh R2 mostly hears itself. Great quality in a conversationalist, less so
in a receiver. So when sdrtop opens the radio, it turns both
transmit channels down all the way, powers the LO off, and reads both back. If
the radio doesn't confirm it, sdrtop won't use it. Nothing is put back when
sdrtop closes; the radio's own defaults return the next time it powers up.

If another program is transmitting, sdrtop leaves the transmitter alone and
receives beside it, and the top line of the screen says **TRANSMITTING TOO**.

## The gain

The R2's gain control is called `hardwaregain`, and sdrtop keeps the name. The
chip switches to a different gain table in each of three bands (up to 1.3 GHz,
up to 4 GHz, and above), and each band starts at a different bottom: -1, -3
and -10 dB. In sdrtop the setting counts from the band's bottom, so `↑` and
`↓` cover every band fully, and the gain you see is the one measured at the
nearest measured frequency. At 2.4 GHz I couldn't measure the top five
settings, so the gain stops at 66 dB there. I'd rather not show a number
nobody measured.

## Sharing the radio

The R2's server lets every program in, and any of them can retune it or change
its gain. While sdrtop receives, it checks the radio once a second. Whatever
another program changed, sdrtop goes along with it, never writes its own value
back, and tells you in the log:

```
Another program on this radio: tuned to 145.500000 MHz.
```

## Rates and the link

The radio samples at up to 61.44 Msps, but the R2 is a USB 2.0 device, and its
USB network carries about 5.7 Msps at best. So over USB sdrtop offers the rates
it ran without losing anything: 2.083, 3 and 4 Msps. Gigabit Ethernet is the
way to the higher rates, and that is next, as soon as I have the cable. And a
dongle, since my laptop is a 2015 MacBook Air, from back when Apple decided
nobody needed an Ethernet port. Progress.

## macOS

Written from the protocol, not yet tried on a Mac. macOS doesn't speak the
R2's default USB network (RNDIS), so set `usb_ethernet_mode = ncm` in the
`config.txt` on the radio's USB drive, eject it, and plug the radio in again.

## Other PlutoSDR boards

Every PlutoSDR shares the same USB id, so sdrtop asks the board what it is.
Only the R2 opens natively. Other boards are named and left to
[SoapySDR](soapysdr.md) (`--device soapy=driver=plutosdr`), because the R2 is
the only one I have measured.

---

← [Supported hardware](hardware.md)
