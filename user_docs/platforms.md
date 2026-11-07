# Host platforms

← [Supported hardware](hardware.md)

---

| Platform | Status |
|----------|--------|
| x86-64 Linux | Fully supported |
| Raspberry Pi (Pi 2 and newer, 64-bit Raspberry Pi OS Bookworm) | Supported, with lower sample rates on older Pis |
| ARM / Android (Termux) | Builds and runs; needs a root-capable USB stack to reach the device |
| macOS (Homebrew) | Unofficial, tested by the people who run it: [macOS, unofficially](#macos-unofficially) |

sdrtop loads libhackrf, librtlsdr, libairspy, libairspyhf and libsdrplay_api at runtime. None of
them is required to build sdrtop. A missing or incompatible library disables only
its backend.

Library loading results are cached for the process lifetime. Unavailable-backend
diagnostics appear on explicit selection or when automatic discovery finds no devices.
Successful discovery stays quiet about unused native backends.
Loaded library handles stay open until process exit.
Restart sdrtop after installing or updating a library.

What each radio's library has to be, by version and file name, is on its own
page: [HackRF](hackrf.md#installing-the-library),
[RTL-SDR](rtl-sdr.md#installing-the-library),
[Airspy R2](airspy-r2.md#installing-the-library),
[Airspy HF+ Discovery](airspy-hf.md#installing-the-library),
[SDRplay RSP1B](sdrplay.md#installing-the-api) and
[SoapySDR](soapysdr.md#using-it).

## macOS, unofficially

Linux is home, and that's not an oversight: it's the only system I have run
sdrtop on with a radio plugged in. The Mac build is built and tested on every
change, on GitHub's Apple Silicon machines, which have no radio. A user proved
the rest before I did: an M4 MacBook Air with an RTL-SDR Blog V4, through
Homebrew. [Observer mode](advanced.md#observer-mode-when-another-app-owns-the-radio)
works there too, reading what macOS's own device registry says about the radio
and the app holding it. The same user watched it switch over while `rtl_test`
held the V4, and it named `rtl_test` as the holder. Nobody has tried a HackRF
or a [tinySA](tinysa.md) there yet (the tinySA is looked for on its
`/dev/cu.usbmodem*` port), so if you do, tell me what it said. sdrtop looks for the
libraries where Homebrew and MacPorts put them (`/opt/homebrew/lib`,
`/usr/local/lib`, `/opt/local/lib`), so this is the whole setup:

```sh
brew install rtl-sdr        # an RTL-SDR
brew install hackrf         # a HackRF
brew install airspy         # an Airspy R2
brew install airspyhf       # an Airspy HF+ Discovery
brew install soapysdr       # SoapySDR
brew install soapyrtlsdr    # and the driver module for your radio (soapyhackrf for a HackRF)
cargo install sdrtop --locked
```

The [one-line installer](getting-started.md) works on a Mac too: it takes the
macOS tarball for your Mac when the release carries one and compiles with cargo
when it doesn't (that needs Apple's Command Line Tools, `xcode-select
--install`), and its `--hackrf`, `--rtlsdr`, `--airspy`, `--airspyhf` and `--soapy` go through Homebrew.
I don't own a Mac, so this stays unofficial: tested by the people who run it. If something behaves
differently there, an issue is how I find out.

---

← [Supported hardware](hardware.md)
