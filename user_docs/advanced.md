# Advanced Features

← [Back](README.md)

The less-obvious behaviour: what happens when you have two radios, when something
else already has the one you want, when the terminal is 40 columns wide, and what
sdrtop deliberately does not do.

This page is workflows and behaviour. For the reference material it used to
duplicate, go straight to the source:

| Looking for | Go to |
|-------------|-------|
| Every key, including focus modes | [Keyboard Shortcuts](keys.md) |
| What a panel draws | [What you see on screen](screens.md) |
| What a measurement means | [The Lab presets](lab.md) |
| Config fields, presets, panel names, positions | [Configuration](config.md) |
| Markers, gain recipes, workflows | [Tips and Tricks](tips-and-tricks.md) |

---

## Command-line options

```sh
sdrtop --frequency 92800000      # start tuned here (Hz)
sdrtop --gain 30                 # primary gain: HackRF LNA / RTL-SDR tuner
sdrtop --lna 24 --vga 30         # HackRF's two stages separately
sdrtop --device rtlsdr           # pin a backend: hackrf | rtlsdr | soapy
sdrtop --theme nord              # see themes.md
sdrtop --config ~/my-config.toml # use a different config file
```

Flags override the config file, and they're applied before the device is opened,
so `--frequency` and `--gain` take effect on the very first frame rather than
after a retune.

`--config` is the one worth remembering. It swaps the whole settings file, which
makes it the clean way to keep a throwaway experiment (a layout you're testing, a
band you're scripting) away from your real `~/.config/sdrtop/config.toml`. Since
`q` saves, driving sdrtop from a script without `--config` will rewrite your
settings.

---

## The session log

sdrtop takes over the terminal completely, which means there's nowhere on screen
for an error that arrives while the alternate screen is up. So for the whole
session it redirects its own error output to `~/.config/sdrtop/sdrtop.log`, and
restores the real stderr on the way out. Anything that would otherwise have
scribbled over your spectrum ends up there.

What's in it and when to read it:
[Troubleshooting](troubleshooting.md#start-here-the-log-file).

The in-app log is a different thing: that's sdrtop talking to *you* about retunes,
gain changes, snapshots and warnings, shown in the `log` panel and in the Command
Rail's `L` overlay.

---

## More than one radio

### The device picker

Plug in more than one radio and sdrtop shows a picker before the TUI starts,
listing every HackRF and RTL-SDR it can see by type and serial:

```
Select device:
  ▸ HackRF One (Serial: 000000000000953c64dc2a1d89c3)
    RTL-SDR R820T (Serial: 00000001)

[J]up [K]down [Enter]confirm [Q]uit
```

`j` / `k` or `↑` / `↓` to move, `Enter` to confirm.

`--device hackrf`, `--device rtlsdr` or `--device soapy` skips the picker when
your radios are
different types. Pinning a *specific* serial isn't possible yet, so with two of
the same kind you still get the list.

### One radio per instance

sdrtop drives one radio at a time. For several at once, run several instances,
which works fine because each one only needs a terminal:

```sh
# two panes, two radios
tmux new-session -d 'sdrtop --device hackrf --config ~/.config/sdrtop/hackrf.toml'
tmux split-window   'sdrtop --device rtlsdr --config ~/.config/sdrtop/rtl.toml'
```

Giving each instance its own `--config` is what stops them overwriting each
other's settings on quit.

Across machines, SSH works the same way:

```sh
ssh pi1@pi1.local 'sdrtop --frequency 433920000' &
ssh pi2@pi2.local 'sdrtop --frequency 156800000' &
```

---

## Observer mode: when another app owns the radio

### What it is

If you start sdrtop while another app (GNU Radio, SDR++, `hackrf_transfer`, a
`rtl_*` tool) already holds the device, sdrtop can't take control. Rather than
failing, it enters **observer mode** and reports what the operating system will
tell it.

In observer mode sdrtop still shows:

- device identity: serial number, board name and revision
- which process is holding the radio
- USB statistics: errors, data transferred
- its own CPU and memory use

It cannot tune, change gain, or stream samples, so there is no spectrum,
waterfall or measurement. The display marks itself accordingly, and quitting does
**not** save the config, since nothing about the session describes your real
settings.

### Getting the radio back

When the other app releases the device, usually by quitting, sdrtop picks it up
automatically. You don't need to restart it; leave it running and it'll switch to
normal mode when the device frees up.

### Finding out what has it

The observer panel names the process. If you want to confirm from outside:

```sh
# what is on the USB bus at all
lsusb

# which process has a USB device open
sudo lsof /dev/bus/usb/*/* 2>/dev/null
```

And if you know what it is:

```sh
pkill -f gnuradio
pkill -f sdrpp
```

---

## Micro mode on small screens

The **Micro** section of the menu holds the field views, on `1` to `4`. They exist
because sdrtop shouldn't need a full terminal to be useful: in a slim tmux split, an SSH
session on a phone, or a cyberdeck's screen, the full panels stop being readable.

Each view adapts to the width it's given across three layouts, so it stays
readable from an 80×24 SSH session down to a 40-column framebuffer. What's in each
view is in [What you see on screen](screens.md#micro-field-views).

The compact sweep view starts a scan on its own, so it's self-contained for field
use: open `Sweep 2` and it begins. It sits in the **Sweep** section rather than
with the other micro views, next to the full-size sweep it is a small version
of.

---

## Frequency tuning steps

In spectrum focus (`e`), `[` and `]` change how far `←` / `→` move the radio:

**1 kHz · 5 kHz · 10 kHz · 25 kHz · 100 kHz · 500 kHz · 1 MHz · 5 MHz · 10 MHz**

The current step shows at the top of the spectrum panel. At 1 MHz you can walk
across a band quickly; at 1 kHz you can settle onto an exact channel. The step is
also what "near a marker" means for the `b` channel-bandwidth key, which looks
within four steps of the cursor.

---

## The baseband filter (HackRF)

The HackRF's analog front end includes a tunable baseband filter, and its
bandwidth is chosen automatically from your sample rate: roughly 2 MHz wide at
2 Msps, 10 MHz at 10 Msps, 20 MHz at the 20 Msps USB 2.0 ceiling. The **BB filter**
field in the RF Chain panel shows what's actually in use.

The trade is the usual one: a narrower filter means less noise reaching the ADC
but less spectrum captured, a wider one the reverse. There's no direct control, so
if you want a narrower filter, lower the sample rate with `s`.

**RTL-SDR has no programmable baseband filter**, and the panel says N/A rather
than inventing a number. The same goes for the Friis noise-figure total, which
needs per-stage gains an RTL dongle doesn't expose.

---

## Color depth

sdrtop detects your terminal's color support and adapts:

- **True color** (24-bit RGB): the waterfall gradients as designed.
- **256-color**: gradients downsampled to the palette cube. Very close.
- **16-color**: basic terminal colors. Everything stays readable, the waterfall
  gets blocky.

No configuration needed. If your colors look wrong over SSH, it's usually `TERM`,
and `TERM=xterm-256color` fixes most of it.

---

## What sdrtop deliberately doesn't do

- **No audio.** Not on any panel, not planned. The demodulator exists to measure
  a transmission, not to play it. This is a decision, not a missing feature.
- **No recording.** sdrtop doesn't write IQ to disk. Use `hackrf_transfer` or
  `rtl_sdr` for that. (On the roadmap, but not here yet.)
- **No transmit.** Read-only, on purpose, on hardware that can transmit.
- **No calibrated power.** The dBm figures on the RF bench are modeled and
  relative, and the panel says so. A calibrated reading needs calibrated hardware.

---

## Known limitations

- **One radio per instance.** See above; run several instances.
- **No per-serial device pinning** for the native backends. `--device` picks a
  type, not a unit. Through SoapySDR you can be as specific as the driver lets
  you: `--device soapy=driver=airspy,serial=644064DC3639AF31`.
- **Sample rate ceilings are USB-bound.** USB 2.0 limits the HackRF to about
  20 Msps in practice, and an older hub or a long cable will lower that. If drops
  appear, the [timing bench](lab.md#timing-bench--lab-timing-lab-3) will tell you
  whether it's the bus or your CPU.
- **No in-app config editing.** The TOML is hand-edited. On the roadmap.

---

← [Back](README.md)
