# Getting Started

← [Back](README.md)

---

## The short way

If you already have Rust, this is the whole thing:

```sh
cargo install sdrtop --locked
```

sdrtop is on [crates.io](https://crates.io/crates/sdrtop), so cargo compiles it
on your machine and links whatever your machine actually has. Works on every
architecture and every distribution. You still need the two libraries first,
which is the "What you need" section below.

## The shorter way, if you don't want to think about it

```sh
curl -fsSL https://raw.githubusercontent.com/musithang/sdrtop/main/packaging/install.sh | sh

# ...and with SoapySDR, if you have an Airspy, an RSP, a Pluto or a Lime
curl -fsSL https://raw.githubusercontent.com/musithang/sdrtop/main/packaging/install.sh | sh -s -- --soapy
```

The installer covers everything the rest of this page describes: it finds your
package manager, installs the two libraries under whatever names your
distribution gives them, and puts sdrtop into `/usr/local/bin` (or `~/.local`
with `--prefix` if you would rather not use root). Then it runs the result to
prove it works.

It uses the prebuilt binary only when that binary can run on your machine, and
it decides that by **running it**, not by guessing from your distribution's
name. When it cannot, on a Raspberry Pi or on Ubuntu and Mint, which package the
exact same `librtlsdr` as Debian under a different soname because of course they
do, it makes sure Rust is present and hands over to `cargo install sdrtop
--locked` instead. That takes a few minutes and needs no
decisions from you.

**SoapySDR is not installed unless you ask.** sdrtop opens it at runtime and
works perfectly well without it, so a plain run leaves it alone and just tells
you at the end whether you have it and what it would buy you. `--soapy` adds the
library and the driver modules your distribution ships. If your distribution's
packages are named something this script has not heard of, it says so rather
than failing, and the fix is one line in
[`packaging/install.sh`](https://github.com/musithang/sdrtop/blob/main/packaging/install.sh)
plus an issue so I can correct it for everyone else.

What it does **not** do is set up device permissions. It reports on them, and
that is deliberate: the `libhackrf` and `rtl-sdr` packages ship their own udev
rules, so installing the libraries is what grants access. If your radio still
needs root afterwards, [troubleshooting](troubleshooting.md#permission-denied)
has the fix.

Read it before you pipe it into a shell if you like:
[`packaging/install.sh`](https://github.com/musithang/sdrtop/blob/main/packaging/install.sh).

### Every flag it takes

```sh
sh install.sh --prefix ~/.local     # install under a directory, no root anywhere
sh install.sh --version v0.4.1      # a specific release instead of the latest
sh install.sh --from-source         # skip the prebuilt binary, always compile
sh install.sh --git                 # compile the main branch, live dangerously
sh install.sh --no-verify           # skip the checksum check (say why first)
sh install.sh --soapy               # add SoapySDR and its driver modules
sh install.sh --deps-only           # the libraries and nothing else
sh install.sh --uninstall           # remove what a previous run installed
sh install.sh --help                # this list, from the script itself
```

Piped straight into a shell they go after `sh -s --`, the way `--soapy` does
higher up. `--help` is the authority here: the script prints its own flags, and
that list cannot drift out of date the way this page can.

`--no-verify` earns a warning of its own. It turns off the checksum check on a
download, which is the one thing standing between you and a tarball that isn't
the one I published. It exists for people who have a reason and know they have
one. If you are reaching for it because the check failed,
[troubleshooting](troubleshooting.md#checksum-mismatch-or-could-not-download-sha256sums)
is the better door.

Everything below is the same job done by hand.

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

## Install and run

With the libraries in place, either of these gets you a working `sdrtop`:

```sh
# From crates.io, straight onto your PATH
cargo install sdrtop --locked

# Or from a clone, if you want to poke at the source
git clone https://github.com/musithang/sdrtop
cd sdrtop
cargo build --release
./target/release/sdrtop
```

`--locked` is worth the four extra characters: it builds with the exact
dependency versions the release was tested with, rather than whatever resolved
this morning.

That's it. sdrtop finds your radio automatically. If it doesn't, that's what the
[troubleshooting](troubleshooting.md) page is for, and we've all been there at
2 a.m.

---

## The prebuilt tarball, by hand

The [releases page](../../../releases) carries one tarball per release, and the
installer fetches it for you. If you would rather do it yourself, or you are
putting sdrtop somewhere a script has no business going:

```sh
tar -xzf sdrtop-<version>-x86_64-unknown-linux-gnu.tar.gz
cd sdrtop-<version>-x86_64-unknown-linux-gnu
sha256sum -c <(grep sdrtop- ../SHA256SUMS)   # check it before you trust it
sudo install -Dm755 sdrtop /usr/local/bin/sdrtop
```

It is x86_64 only, and it wants **glibc 2.36 or newer** plus `librtlsdr.so.0`.
In practice that means Debian 12 and 13, Kali, and Raspberry Pi OS Bookworm.
Ubuntu and Mint package the identical upstream library as `.so.2`, so it will
not start there, and a Raspberry Pi is not x86_64 in the first place. On those,
compile. It is one command, it takes a few minutes, and it always works.

From 0.4.2 onward every release also carries a signed build provenance
attestation. The checksum only tells you the download arrived intact. This tells
you the file came out of this repository's release workflow and nowhere else:

```sh
gh attestation verify sdrtop-<version>-x86_64-unknown-linux-gnu.tar.gz --repo musithang/sdrtop
```

Worth thirty seconds. You are about to give this binary your USB bus.

---

## First run

If you have more than one radio connected, a **device selector** appears first,
listing every HackRF and RTL-SDR by type and serial. Use `↑` / `↓` (or `j` / `k`)
to pick one, then `Enter`. Skip it entirely with `--device hackrf` or
`--device rtlsdr`.

Then the **menu** opens, which is sdrtop's front door. On the left are the four
families of layout, on the right the layouts in whichever one is selected.

1. **`Enter`** takes the highlighted layout, which on a first run is the Command
   Rail cockpit. From then on the menu opens on whatever you were using last, so
   `Enter` is a resume.
2. **`Space`** to start receiving. The spectrum and waterfall come to life.
3. **`↑` / `↓`** to adjust gain if the signal looks too weak or too strong. That's
   the LNA on a HackRF, the tuner gain on an RTL-SDR, and the whole gain chain on
   anything reached through SoapySDR. A flat trace usually means gain is far off
   in one direction or the other.
4. **`Esc`** brings the menu back at any time. Tab across to **Keys** for the full
   key reference without leaving the app.
5. **`q`** to quit. Your settings are saved automatically.

Once that works, the interesting part is the sections. `Esc`, then `Tab` to
**Lab**, and `1` to `4` are the [measurement benches](lab.md). **Micro** shrinks
everything down for a small screen, and **Sweep** scans a whole band.

Numbers start again at `1` in every section, which is why there are four families
rather than one long row of keys. This guide writes them as section then number,
so `Lab 2` means "press `2` while Lab is the section you are in".

---

## Common startup options

```sh
# Start tuned to a specific frequency (in Hz)
sdrtop --frequency 92800000

# Start with specific gain settings, naming the stages your radio has
sdrtop --gain "LNA=24,VGA=30"

# Or give a total and let sdrtop place it, front stage first
sdrtop --gain 54

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
