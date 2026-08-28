# Getting Started

← [Back](README.md)

---

## What you need

- A Linux machine
- A HackRF One **or** an RTL-SDR dongle connected via USB
- The `libhackrf` and `librtlsdr` libraries
- Rust stable 1.88 or newer. Most distributions ship something older, so
  install it with [rustup](https://rustup.rs) rather than from your package
  manager

```sh
# Arch Linux / Manjaro
sudo pacman -S hackrf rtl-sdr pkgconf

# Debian / Ubuntu / Linux Mint / Pop!_OS
sudo apt install libhackrf-dev librtlsdr-dev pkg-config

# Fedora
sudo dnf install hackrf-devel rtl-sdr-devel pkgconf-pkg-config

# openSUSE Tumbleweed / Leap
sudo zypper install libhackrf-devel rtl-sdr-devel pkg-config

# Void Linux
sudo xbps-install hackrf-devel rtl-sdr-devel pkg-config

# Gentoo
sudo emerge net-wireless/hackrf net-wireless/rtl-sdr

# NixOS: add to configuration.nix, or use a dev shell
nix-shell -p hackrf rtl-sdr pkg-config
```

> **Install both libraries even if you only own one radio.** sdrtop links both
> backends at build time, so a missing `librtlsdr` breaks the build for a HackRF
> owner and vice versa. At runtime it's perfectly happy with whichever radio you
> actually plug in.

Rust, if you don't have it:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Distribution Rust packages are often too old. If the build stops with a complaint
about `lock file version 4`, that's what happened, and rustup is the fix.

---

## Build and run

```sh
git clone https://github.com/mustang6139/sdrtop
cd sdrtop
cargo build --release
./target/release/sdrtop
```

That's it. sdrtop finds your radio automatically. If it doesn't, that's what the
[troubleshooting](troubleshooting.md) page is for, and we've all been there at
2 a.m.

---

## First run

If you have more than one radio connected, a **device selector** appears first,
listing every HackRF and RTL-SDR by type and serial. Use `↑` / `↓` (or `j` / `k`)
to pick one, then `Enter`. Skip it entirely with `--device hackrf` or
`--device rtlsdr`.

Then:

1. **`Space`** to start receiving. The spectrum and waterfall come to life.
2. **`↑` / `↓`** to adjust gain if the signal looks too weak or too strong. That's
   LNA on a HackRF, the tuner gain on an RTL-SDR. A flat trace usually means gain
   is far off in one direction or the other.
3. **`?`** at any time for the full key reference on screen.
4. **`q`** to quit. Your settings are saved automatically.

Once that works, the interesting parts are the number keys: `1` is the Command
Rail cockpit you started on, `5` through `9` are the
[measurement benches](lab.md), and `0` shrinks everything down for a small
screen.

---

## Common startup options

```sh
# Start tuned to a specific frequency (in Hz)
sdrtop --frequency 92800000

# Start with specific gain settings (HackRF LNA and VGA)
sdrtop --lna 24 --vga 30

# Device-agnostic primary gain: HackRF LNA, RTL-SDR tuner
sdrtop --gain 30

# Pin a backend when you have both a HackRF and an RTL-SDR plugged in
sdrtop --device rtlsdr

# Use a different color theme
sdrtop --theme nord

# Load a different config file
sdrtop --config ~/my-config.toml
```

`--config` is worth knowing about early: `q` saves your settings, so if you're
scripting sdrtop or experimenting with a layout, pointing it at a throwaway file
keeps your real config out of it.

---

## Where to go next

- **[Keyboard shortcuts](keys.md)**: every key, including the focus modes
- **[What you see on screen](screens.md)**: every panel, explained
- **[The Lab presets](lab.md)**: what the measurements mean
- **[Tips and Tricks](tips-and-tricks.md)**: setting gain, finding signals,
  surviving a long capture
