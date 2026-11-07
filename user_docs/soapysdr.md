# SoapySDR: the honest version

← [Supported hardware](hardware.md)

---

Here is the thing. sdrtop supported two radios because I owned two radios, and I had
a rule: **hardware support lands only after physical testing.** It is a good
rule. It is also why every week somebody says they would try this if it spoke to
their Airspy, and every week the answer is "sorry, I do not have one".

So I broke my own rule, on purpose, in one specific place.

[SoapySDR](https://github.com/pothosware/SoapySDR) is one API with a lot of
radios behind it: the Airspy Mini, SDRplay's RSP line, PlutoSDR, LimeSDR, bladeRF, USRP,
and SoapyRemote for a radio on a different machine entirely. sdrtop now speaks
it. **The backend was written from the SoapySDR headers, not from owning the
devices**, which means it is exactly as good as the drivers are honest and as
careful as I could be reading a C header at midnight.

What I did instead of pretending otherwise:

- **Nothing about your radio is hardcoded.** Frequency range, sample rates, gain
  range, whether there is an AGC, whether there is a baseband filter, the sample
  format and how many bits actually mean something in it: sdrtop asks the driver
  for all of it. There is no table of devices in the source, because a table
  would be me guessing about hardware I have never held.
- **What cannot be asked is refused, not invented.** If sdrtop cannot work
  something out, that reading reads as unavailable. It does not fall back to a
  plausible number. A plausible number is the worst thing a measurement tool can
  produce.
- **When a driver disagrees with sdrtop, the log says so by name**, including the
  driver's own error text. That is in `~/.config/sdrtop/sdrtop.log`, and it is
  what makes a bug report actionable instead of a mystery.

A necessary evil, then, and it turned out to be less of an evil than I feared.
Before this, every row in the [hardware table](hardware.md) was green, and "tested on real
hardware" was a phrase that covered everything I shipped, which is another way of
saying it separated nothing. The grey rows are what give the green ones meaning.
A rule nobody has ever had a reason to test is not a rule, it is a habit, and
this one now has one marked edge on it. Marked being the whole point: an
exception you do not write down is not an exception, it is a rule you quietly
dropped.

## What is actually verified

| Thing | State |
|---|---|
| Loading libSoapySDR, ABI check, enumeration | Verified on a real install (0.8.1); 0.9 read from its headers and loaded in CI on macOS |
| Opening a device, reading its capabilities | Verified, on a HackRF through `SoapyHackRF` |
| Streaming, at full sample rate, no drops | Verified, same HackRF, 8 Msps and 10 Msps |
| An Airspy R2 through `SoapyAirspy` | Verified: streamed, and compared bench by bench with sdrtop's own [Airspy R2](airspy-r2.md) backend |
| The two radios above, unaffected | Verified. The native paths did not change |
| A HackRF through **SoapyRemote** | Verified by a user, not by me: Raspberry Pi Zero 2 W server, Wi-Fi link, 1 Msps. See [below](#a-radio-on-another-machine-soapyremote) |
| **Every other radio** | **Unverified.** The code is right as far as I can reason. That is not the same as right |

So if you have an Airspy Mini, an RSP, a Pluto or a Lime: you are the test. I would
genuinely love an issue either way, working or not. "It works, here is what the
header said" is as useful to me as a crash.

## Using it

Nothing to enable. If libSoapySDR is installed, sdrtop finds it at startup and
your devices appear in the picker next to any HackRF, RTL-SDR or Airspy R2. If it is not
installed, sdrtop behaves exactly as it did before, same binary, no complaints.

**libSoapySDR is not needed to build and not needed to run.** It is opened at
runtime if it happens to be there, which is why the same binary serves people who
have it and people who have never heard of it. On Debian and Ubuntu that is
`libsoapysdr0.8` plus a driver module such as `soapysdr-module-airspy`, or
`soapysdr-module-all` if you are feeling generous with disk space.

```sh
# Is your radio visible to SoapySDR at all? Start here, always.
SoapySDRUtil --find

# Everything SoapySDR can see, and nothing else
sdrtop --device soapy

# One driver in particular
sdrtop --device soapy=driver=airspy
```

`SoapySDRUtil --find` is the first thing to run and the first thing to paste into
an issue. If SoapySDR cannot see your radio, sdrtop has no chance, and the
problem is a missing driver module rather than anything in here.

## A radio on another machine: SoapyRemote

SoapyRemote puts the radio on one machine and sdrtop on another: a HackRF on a
Raspberry Pi in the attic, sdrtop on the laptop downstairs. To sdrtop it is still
a SoapySDR device, so everything on this page applies. The network is just one
more stage in the chain, and usually the slowest one.

On the machine with the radio, install the server (`soapyremote-server` on
Debian and Ubuntu) next to the radio's own driver module, and start it:

```sh
SoapySDRServer --bind
```

On the machine running sdrtop, install the client module
(`soapysdr-module-remote`), then:

```sh
# Can this machine see the server? Start here, as always.
SoapySDRUtil --find="driver=remote"

# Only radios reached over the network
sdrtop --device soapy=driver=remote

# One server in particular, written exactly as SoapySDRUtil prints it
sdrtop --device soapy=remote=tcp://192.168.1.20:55132
```

The remote radio shows up in the picker as `SoapySDR · remote · HackRF One …`,
and from there it is the same radio as if it were plugged in locally.

Two things to know before you rely on it:

- **sdrtop finds the server the way SoapyRemote finds it on its own**, by asking
  the local network. If `SoapySDRUtil --find="driver=remote"` lists your radio,
  sdrtop will too. If it only shows up when you hand SoapySDRUtil the address
  yourself (another subnet, a VPN, a network that drops multicast), sdrtop
  cannot reach it yet: there is no way to give sdrtop the address up front.
- **Every sample crosses the network.** Pick a sample rate the link can carry.
  The setup this was verified on ran 1 Msps over Wi-Fi that managed about
  1.8 MB/s. If the link falls behind, the drop counters say so, and here the
  culprit is the network rather than the USB cable: lower the rate first
  ([more on drops](troubleshooting.md#samples-are-dropping)).

This works thanks to a user who owns this exact setup, found why it did not, and
sent the fix. That is exactly how the grey rows are meant to turn green.

## The gotchas I already hit, so you do not have to

| Symptom | Cause / fix |
|---|---|
| Your **sound card** shows up as an SDR | It is real: `soapysdr-module-audio` presents audio inputs as SDR sources, which is genuinely useful with a soundcard receiver and confusing on a laptop. sdrtop skips the `audio` driver by default. Ask for it with `--device soapy=driver=audio` |
| A **SoapyRemote** radio is listed but **will not open** | A bug in 0.6.2 and earlier: the server address was dropped on the way from the picker to the open. Fixed; update sdrtop |
| Your HackRF, RTL-SDR, Airspy R2 or HF+ Discovery appears **once**, not twice, even with the Soapy module installed | On purpose. sdrtop's own driver for those four knows more about them than the generic path does, so the native one wins. Force the other with `--device soapy` |
| The **RF bench is missing** its noise figure, MDS and linearity card | Also on purpose. Those model a specific front end stage by stage. SoapySDR does not publish that, and inventing it would be worse than leaving it out |
| **No `[A]` boost** on your device | sdrtop asks the driver whether there is an automatic gain mode and only offers the key if there is. A HackRF reached through SoapySDR reports there is not, which surprised me too |
| `[` and `]` do nothing | Those two are the HackRF's VGA keys. Everywhere else, pick a stage with `,` / `.` and use `↑` / `↓`. See below |
| "its native sample format is CF32" in the log | sdrtop's pipeline is integer, so it handles `CS8`, `CU8` and `CS16` and refuses the rest by name rather than guessing at a conversion. Open an issue with the driver name |
| The frequency range looks **too optimistic** | It is the driver's number, not mine. `SoapyHackRF` claims 0 to 7.25 GHz where the datasheet says 1 MHz to 6 GHz. sdrtop reports what it is told; the radio will refuse the rest, and the log will say so |

## What a SoapySDR device gets less of

| | Native HackRF / RTL-SDR | Through SoapySDR |
|---|---|---|
| Gain | LNA and VGA on their own keys | Every element the driver names, each on its own range. `↑` / `↓` move the whole chain; `,` / `.` pick one stage, from any layout |
| Front-end boost | Always there | Only if the driver reports an automatic gain mode |
| Friis noise figure, MDS | Modelled per stage | Not shown. We do not know the chain |
| Linearity card (IIP3, IMD3, SFDR) | Shown | Not shown. Those are one front end's datasheet |
| ADC bench | 8-bit | Follows the converter the driver reports, so a 12 or 14-bit radio is described as one |
| Everything else | | The same |

## The gain, in more detail

This used to be one overall number, and one number was not enough. Handing a
driver a total and letting it decide the split is the standard thing to do, and
on a HackRF through `SoapyHackRF` what it decides is not what you want: asked for
60 dB it put most of it in the VGA and left the LNA low, which is the arrangement
with the *worst* noise figure of the ones available. Worse, it was not monotonic.
Turning the knob up could collapse the LNA from 32 dB to 19 on the way.

So sdrtop no longer asks the driver to split anything. It reads every gain
element the device names and every element's own range, and places gain itself:

- **`↑` / `↓` move the whole chain**, filling the front stage first, up to its
  ceiling, then the next. Front-first is the arrangement with the best noise
  figure, which is the arrangement you want unless you have a specific reason to
  want another one.
- **`,` / `.`**, from any layout, pick one element by name, and
  then `↑` / `↓` move that one alone, by its own step, leaving the rest exactly
  where they are. This is the only way to reach a third gain element, and some
  radios have one.
- **The config file names them too**, so a setup you worked out once survives a
  restart: `gain = "LNA=32,VGA=20"`. See [config.md](config.md#the-gain-line).

The element **names** are the driver's, not sdrtop's, and nothing is mapped onto
an LNA/VGA pair. A driver that calls its stages `IF1` and `IF2` gets rows labelled
`IF1` and `IF2`. Guessing which one is "really" the LNA is exactly the bug that
costs somebody an afternoon, and the names are right there to be read.

There is no table of devices behind any of this. If your radio's driver reports
its elements, it works; if it reports one element, you get one row and one knob,
which is the honest answer rather than a guess dressed as two.

---

← [Supported hardware](hardware.md)
