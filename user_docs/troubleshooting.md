# Troubleshooting

← [Back](README.md)

---

## Start here: the log file

sdrtop owns the whole terminal while it runs, so there's nowhere on screen for an
error that arrives mid-session. Everything of that kind goes to:

```
~/.config/sdrtop/sdrtop.log
```

Config parse warnings, driver messages, and whatever the backend libraries feel
like saying (librtlsdr in particular is talkative: "Allocating zero-copy buffers",
"Found … tuner", "[R82XX] PLL not locked!"). If sdrtop did something strange and
told you nothing, read that file first. It's also the single most useful thing to
attach to a bug report.

The in-app log is a different thing: that's sdrtop talking to you about retunes,
gain changes and warnings, shown in the `log` panel and in the Command Rail's `L`
overlay.

---

## The radio isn't found

### "No SDR device found"

sdrtop prints this and exits before the TUI starts.

1. **Check the cable and the port.** A surprising share of "dead" radios are dead
   cables. Try a different, short one, straight into the machine rather than
   through a hub.
2. **Check the kernel sees it at all:**
   ```sh
   lsusb
   ```
   You want a line naming your device: `Great Scott Gadgets HackRF One`, or for a
   dongle something like `Realtek Semiconductor Corp. RTL2838 DVB-T`. If nothing
   appears, the problem is below sdrtop, and no amount of software configuration
   will help.
3. **If it appears in `lsusb` but sdrtop can't open it**, that's permissions. See
   below.
4. **If something else already has it**, sdrtop enters observer mode rather than
   failing. See [Advanced Features](advanced.md#observer-mode-when-another-app-owns-the-radio).

### Permission denied

Almost always missing udev rules. The easy fix is to install your distribution's
package for the device, which ships the rules:

```sh
sudo pacman -S hackrf rtl-sdr        # Arch
sudo apt install hackrf rtl-sdr      # Debian / Ubuntu
sudo dnf install hackrf rtl-sdr      # Fedora
```

If you need to write a rule by hand, take the vendor and product IDs from your own
`lsusb` output rather than from a web page, since they vary by revision and clone.
For a line reading `Bus 001 Device 005: ID 0bda:2838 …`, the rule is:

```sh
# /etc/udev/rules.d/99-sdr.rules
SUBSYSTEM=="usb", ATTRS{idVendor}=="0bda", ATTRS{idProduct}=="2838", MODE="0666", GROUP="plugdev"
```

Then reload and replug:

```sh
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Check you're in the group the rule names:

```sh
groups $USER | grep -q plugdev || sudo usermod -aG plugdev $USER
```

A group change needs a full logout to take effect, not just a new shell.

---

## RTL-SDR specifics

### The kernel stole your dongle

This is the most common RTL-SDR problem on Linux, and it looks exactly like a
broken device. An RTL2832-based dongle is, as far as Linux is concerned, a DVB-T
television tuner, so the kernel loads its TV driver and claims the hardware before
any SDR software gets a chance.

Symptom: the dongle shows up in `lsusb`, and `rtl_test` says "usb_claim_interface
error -6" or "Failed to open rtlsdr device".

Fix: tell the kernel not to load that driver.

```sh
# /etc/modprobe.d/blacklist-rtl.conf
blacklist dvb_usb_rtl28xxu
blacklist rtl2832
blacklist rtl2830
blacklist rtl8xxxu
```

Then unload it for this session and replug the dongle:

```sh
sudo modprobe -r dvb_usb_rtl28xxu
```

If `modprobe -r` complains the module is in use, unplug the dongle first.

### Confirm the dongle works at all

Before blaming sdrtop, check the dongle against the reference tool:

```sh
rtl_test -t
```

That prints the tuner type (R820T, R828D, E4000) and the supported gain values. If
`rtl_test` can't open it either, the problem isn't sdrtop.

### Clones behave differently

The RTL clone market is a zoo, and no single person owns all of it. Different
tuners have different gain tables, different frequency ranges and different
quirks. If yours behaves oddly in a way `rtl_test` doesn't explain,
[open an issue](../../../issues) with the tuner type and what you saw.

---

## The display isn't doing anything

### Spectrum and waterfall are frozen

1. **Press `Space`.** If RX isn't streaming, nothing moves. Every measurement
   panel also marks itself `[STALE]` in this state, which is the tell.
2. **Check gain.** A completely flat trace means gain is far too low (all noise
   floor) or far too high (everything slammed at the top). `↑` / `↓` to adjust.
   On a HackRF, LNA 24 / VGA 30 is a reasonable place to start over from.
3. **Check the health panels.** Lab Timing (`7`) shows drops, saturation and CPU.
   Non-zero drops mean USB or CPU can't keep up; high saturation means turn the
   gain down.

### A key does nothing

1. **Focus keys only work when their panel is on screen.** `x` needs Lab Signal,
   `d` needs Lab RF, and so on. If the panel isn't in the current preset, the key
   is inert.
2. **VGA keys do nothing on an RTL-SDR.** There's no VGA stage. `[` and `]` are
   HackRF-only; use `a` for the tuner AGC instead.
3. **You might be in a text-entry mode.** After `f`, `s` or `m`, keystrokes are
   letters rather than commands until you press `Enter` or `Esc`.
4. **Known bug: `v` and `t` are unreliable in Lab Timing.** Two panels each claim
   these focus keys, and which one wins is decided randomly at startup. On some
   launches pressing `v` or `t` in preset `7` does nothing at all, even though the
   footer offers them. Restarting sdrtop reshuffles it. This is a bug and will be
   fixed; it isn't something you're doing wrong.

---

## Drops, USB errors and stuttering

### Samples are dropping

1. **Lower the sample rate.** `s`, then a smaller number. If you're at 20, try 5.
   This is the single most effective fix and usually the correct one.
2. **Check which end is at fault.** Lab Timing (`7`) answers this directly. If the
   **Callback Interval Strip Chart** shows late callbacks clustered around
   something, your host is stalling. If the callbacks are punctual but the ring
   buffer still fills, the bus is the limit.
3. **Change the cable, port or hub.** Long cables and cheap hubs cause this
   constantly. Straight into the machine, short cable.
4. **Check CPU.** If sdrtop's own CPU is high on a modern multi-core machine,
   something else is competing. A lower sample rate also cuts the FFT cost.

### USB errors (zero-length transfers)

Nearly always physical: cable, port or hub, in that order of likelihood. The count
is coloured by *recent* rate rather than session total, so a single old glitch
doesn't stay red forever. A slowly climbing count during a capture is worth acting
on; one event on plug-in isn't.

### Stuttering at high sample rates

sdrtop redraws at about 30 fps. On a slow host at 20 Msps that's real work, mostly
FFT.

- Drop the sample rate. It cuts the FFT size and the CPU load together.
- Use a smaller preset. The micro views (`0`) draw far less than a full lab bench.
- Close the other things. Browsers are the usual culprit.

---

## Measurements that look wrong

### The demod says blocks were dropped

Believe it. The demodulator is fed through a small queue that discards blocks when
the machine can't keep up, and RDS and CTCSS both need an unbroken run of samples
to decode anything. Without that warning, a busy computer and a station with no
RDS look identical.

The fix is the same as for drops generally: lower the sample rate, or close
whatever else is using the CPU. The warning only appears while blocks are actually
being lost, so if it's gone, the problem is gone.

### RDS shows nothing

- **RDS needs a decent signal.** It rides about 20 dB below the programme audio,
  so a station that sounds perfect can still decode nothing.
- **Check the mode.** If the badge says `NFM` on a broadcast station, the
  classifier has been fooled by a wide span. Press `T` in demod focus to force
  WFM.
- **Check the channel.** If the panel warns "on DC spike", the demodulated channel
  is sitting on the radio's own LO leakage. Press `D` in Lab IQ for the DC block,
  or walk the channel off centre with `←` / `→`.
- **Give it a moment.** `◌ DECODING` means bits are arriving and the name is a
  second or two away. `○ NO RDS` is the real "this station carries none".
- **Check `Groups`.** Two numbers means the total decoded here and the current
  unbroken run. A big total with a run of 1 means reception keeps breaking, which
  points at signal or CPU rather than at the station.

### Occupied bandwidth changed when I changed the sample rate

It does, and that's a known limitation rather than a glitch. A wider span means
coarser FFT bins and a different view of the carrier's skirts, so the same station
can measure 101.6 kHz at 2 Msps and 65.9 kHz at 5 Msps. Two readings are only
comparable at the same rate.

It also drags the modulation badge along, which is why a broadcast station on a
very wide span can classify as `NFM`. Force the demodulator with `T` rather than
trusting the badge. Full explanation in
[The Lab presets](lab.md#about-occupied-bandwidth).

### The noise floor jumped when I changed the sample rate

Look at the **density** figure in dBFS/Hz beside it instead. The per-bin noise
floor rises with the bin width, so it genuinely changes with sample rate and
describes the analyser as much as the radio. The density divides that out and
reports the same receiver as the same receiver.

### DC offset or DC spike is high

Every radio has some DC offset from component tolerances, and a DC spike below
−40 dBFS is normal. If it's in the way of a measurement, press `D` in Lab IQ
focus and it's subtracted from the stream. Some sample rates also show less of it
than others, so `s` and a bit of experimenting is worth a try.

### IRR is low

Below 20 dB means quadrature imbalance is significant and mirror images will be
visible. Two things to try:

- **Press `C` in Lab IQ focus** with a strong clean carrier on screen. That
  captures a quadrature correction and applies it, which is exactly what this is
  for. Watch the image drop on the Image-Rejection Scope.
- **Try another sample rate.** Some are better than others on the same hardware.

Beyond that, IRR is largely a property of the hardware. If you're just watching a
strong signal it doesn't matter much; for clean spectrum work, aim for 30 dB or
better.

---

## Settings and the config file

### My settings vanished after I edited the config

If the file doesn't parse, sdrtop falls back to **all defaults** for the whole
file rather than guessing which line you meant, so one stray character looks like
total amnesia. The warning naming the problem is in `~/.config/sdrtop/sdrtop.log`.

The most common cause is a capitalised position name. `position = "top"` is
correct; `"Top"` fails to parse and takes the whole file with it.

### My theme color overrides disappeared

Known bug. Only `theme.base` is written back on save, so per-field overrides like
`border_accent` are removed from the file the first time you quit with `q`. They
work correctly while sdrtop is running and load correctly every launch; they just
aren't preserved. Keep them in a config you don't quit-and-save over
(`--config`), or re-add them after a save. See
[Configuration](config.md#what-survives-a-save).

### Settings aren't saved at all

1. **Quit with `q`, not `Ctrl+C`.** Only a clean quit saves.
2. **Check the directory is writable:**
   ```sh
   ls -ld ~/.config/sdrtop/
   ```
3. **Check you have disk space.** A full home partition makes the save fail.
   ```sh
   df -h ~/
   ```
4. **Observer mode never saves.** If another app held the radio, there was nothing
   real to save, so sdrtop deliberately leaves the file alone.

### Markers aren't persisting

Same rules: quit with `q`, and check the file:

```sh
grep -A2 spectrum_markers ~/.config/sdrtop/config.toml
```

You can also write them by hand; see
[Configuration](config.md#spectrum-markers).

---

## Building

### The build fails with "lock file version 4"

Your `cargo` is too old. The lockfile needs cargo 1.78 or newer, and distribution
Rust packages are frequently behind. Install rustup:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### The build fails looking for libhackrf

sdrtop links **both** backends at build time, so you need `libhackrf` and
`librtlsdr` present even if you only own one radio. At runtime it's happy with
whichever you plug in. Package names per distribution are in
[Getting Started](getting-started.md).

libhackrf must be **2023.01.1 or newer**. That's what ships in Raspberry Pi OS
Bookworm and Ubuntu 24.04; older distributions need it built from source.

---

## Getting help

If none of this covered it:

1. **Check [What's new](whats-new.md)** in case the behaviour changed recently.
2. **Collect the useful things:** `~/.config/sdrtop/sdrtop.log`, your device and
   tuner type, sample rate, host OS and CPU, and what you did to trigger it.
3. **[Open an issue](../../../issues)** with all of that. A log file and a
   reproduction turn a guess into a fix.

← [Back](README.md)
