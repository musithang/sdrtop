# Getting Started

← [Back](README.md)

---

## The short way

If you already have Rust, this is the whole thing:

```sh
cargo install sdrtop --locked
```

sdrtop is on [crates.io](https://crates.io/crates/sdrtop). Building needs Rust
1.88+ and a C compiler/linker. Native SDR libraries are optional at runtime.

## The shorter way, if you don't want to think about it

```sh
curl -fsSL https://raw.githubusercontent.com/musithang/sdrtop/main/packaging/install.sh | sh

# Install the runtime for a HackRF
curl -fsSL https://raw.githubusercontent.com/musithang/sdrtop/main/packaging/install.sh | sh -s -- --hackrf

# ...and with SoapySDR, if you have an Airspy, an RSP, a Pluto or a Lime
curl -fsSL https://raw.githubusercontent.com/musithang/sdrtop/main/packaging/install.sh | sh -s -- --soapy
```

The installer puts sdrtop into `/usr/local/bin`. Use `--prefix ~/.local` for
a user-local install. It runs the binary to check compatibility with your machine.
An incompatible architecture or libc triggers a source build through
`cargo install sdrtop --locked`.

Runtime installation is opt-in. `--hackrf` adds libhackrf. `--rtlsdr` adds
librtlsdr. `--soapy` adds SoapySDR and its driver modules. These flags can be
combined. A plain install of a runtime-loading release adds none of these libraries.
Missing or incompatible libraries disable only their respective backends.

Older releases can require native SDR development packages.
After a recognized native-link build failure, the installer adds those packages
and retries the same release once. It never switches to `main`.
Use `sh install.sh --git` to select `main` explicitly. See
[older-release build failures](troubleshooting.md#the-build-fails-looking-for-libhackrf).

Runtime package installation is best-effort. Check warnings for packages that
could not be installed. The runtime package lists fall back to development
packages on some distributions. Those packages include the runtime library.

The installer uses distribution packages for native SDR udev rules. It reports
device-access advice when `--hackrf` or `--rtlsdr` is selected. See
[troubleshooting](troubleshooting.md#permission-denied) for access problems.

Read it before you pipe it into a shell if you like:
[`packaging/install.sh`](https://github.com/musithang/sdrtop/blob/main/packaging/install.sh).

### Every flag it takes

```sh
sh install.sh --prefix ~/.local     # install under a directory, no root anywhere
sh install.sh --version v0.4.1      # a specific release instead of the latest
sh install.sh --from-source         # skip the prebuilt binary, always compile
sh install.sh --git                 # compile the main branch, live dangerously
sh install.sh --no-verify           # skip the checksum check (say why first)
sh install.sh --hackrf              # Install the HackRF runtime
sh install.sh --rtlsdr              # Install the RTL-SDR runtime
sh install.sh --soapy               # add SoapySDR and its driver modules
sh install.sh --deps-only --rtlsdr   # Install only the selected runtime
sh install.sh --uninstall           # remove what a previous run installed
sh install.sh --help                # this list, from the script itself
```

Piped straight into a shell they go after `sh -s --`, the way `--soapy` does
higher up. `--help` is the authority here: the script prints its own flags, and
that list cannot drift out of date the way this page can.

`--deps-only` installs only the runtimes selected by `--hackrf`, `--rtlsdr`
and `--soapy`. It never installs sdrtop or build tools. Without a runtime flag
it exits with a usage error.

`--no-verify` earns a warning of its own. It turns off the checksum check on a
download, which is the one thing standing between you and a tarball that isn't
the one I published. It exists for people who have a reason and know they have
one. If you are reaching for it because the check failed,
[troubleshooting](troubleshooting.md#checksum-mismatch-or-could-not-download-sha256sums)
is the better door.

Everything below is the same job done by hand.

## What you need

- **Host:** A Linux machine.
- **Radio:** A HackRF One, RTL-SDR or a supported SoapySDR device.
- **Source builds:** Rust 1.88+ and a C compiler/linker. Install Rust with
  [rustup](https://rustup.rs). Runtime-loading releases need no SDR development
  headers or pkg-config. Older releases can require both native development libraries.
- **Runtime:** Install only the library for the backend you use.

Build tools by distribution:

```sh
# Arch Linux / Manjaro
sudo pacman -S base-devel

# Debian / Ubuntu / Linux Mint / Pop!_OS
sudo apt install build-essential

# Fedora
sudo dnf install gcc

# openSUSE Tumbleweed / Leap
sudo zypper install gcc

# Void Linux
sudo xbps-install base-devel

# Gentoo
sudo emerge sys-devel/gcc

# NixOS: add to configuration.nix, or use a dev shell
nix-shell -p gcc
```

Install runtime packages separately for your radio:

| Distribution | HackRF | RTL-SDR |
|---|---|---|
| Debian / Kali / Raspberry Pi OS | `libhackrf0` | `librtlsdr0` |
| Ubuntu / Mint / Pop!_OS | `libhackrf0` | `librtlsdr2` |
| Arch / Manjaro / Fedora / Void | `hackrf` | `rtl-sdr` |
| openSUSE | `libhackrf0` | `librtlsdr0` |
| Alpine | `hackrf-libs`, `hackrf-udev` | `librtlsdr`, `librtlsdr-udev` |
| Gentoo | `net-wireless/hackrf` | `net-wireless/rtl-sdr` |
| Nixpkgs | `hackrf` | `rtl-sdr` |

HackRF requires libhackrf 2023.01.1+ with all required symbols. RTL-SDR supports
`librtlsdr.so.0`, `librtlsdr.so.2` and `librtlsdr.so`. HackRF supports
`libhackrf.so.0` and `libhackrf.so`. See [hardware](hardware.md#host-platforms)
for the runtime contract. Custom library locations must be on the dynamic
loader's search path, for example through `LD_LIBRARY_PATH`.

Rust, if you don't have it:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Distribution Rust packages are often too old. If the build stops with a complaint
about `lock file version 4`, that's what happened, and rustup is the fix.

---

## Install and run

Build and install sdrtop:

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

The tarball needs x86_64 Linux with glibc 2.36+. Debian 12+ and Ubuntu 24.04+
meet that floor. Both Debian's `librtlsdr.so.0` and Ubuntu's `librtlsdr.so.2`
are supported at runtime. Neither library is needed to start sdrtop.
Raspberry Pi, musl-based systems and older glibc systems need a source build.

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
