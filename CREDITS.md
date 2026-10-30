# Credits

This project was developed with humans and AI working together.

## Contributors

- **[@gorkemgun](https://github.com/gorkemgun)**: the RTL-SDR backend (R820T /
  R828D / E4000), written against the `SdrDevice` abstraction and confirmed on
  real hardware in both normal RX and observer mode. It is the single change
  that took sdrtop from a one-device app to a two-device one.
- **[@AlCalzone](https://github.com/AlCalzone)**: the tinySA / tinySA Ultra
  backend, staged as a sequence of reviewable PRs - calibrated power-trace
  acquisition, native band sweeps, and the device-options framework with
  numeric entry that both the spectrum analyzer and tinySA controls now share.
  Verified on a tinySA Ultra ZS405. Thank you for the care put into structuring
  it this way.

Found a bug, tested a clone nobody else owns, or sent a patch? Open an issue or
a pull request and your name belongs here too.

## Packaging

- **Ateles ([@4goettma](https://github.com/4goettma))**: the Arch Linux
  packages on the AUR, `sdrtop`, `sdrtop-git` and `sdrtop-bin`, kept current:
  the optional radio libraries and a build that matches the release profile.

## Hardware

- **[Airspy](https://airspy.com/)**: an Airspy R2 and an Airspy HF+ Discovery,
  engineering samples, sent to the project in October 2026.
- **[SDRplay](https://www.sdrplay.com/)**: an RSP1B, sent to the project in
  October 2026.

## Data

- **The Bluetooth SIG's assigned numbers**
  ([bitbucket.org/bluetooth-SIG/public](https://bitbucket.org/bluetooth-SIG/public)):
  company identifiers, 16-bit service UUIDs and Core versions, read from
  `company_identifiers.yaml`, `service_uuids.yaml` and `core_version.yaml`
  into the dated snapshot `src/signal/sig_numbers.tsv` by
  `packaging/update-sig.sh`. They name a BLE advertiser's company and a
  classic link manager's company and Core version, rather than leaving two
  numbers. The names remain the SIG's.
- **[Natural Earth](https://www.naturalearthdata.com/)** (public domain):
  the land polygons at 1:110m, `packaging/data/ne_110m_land.geojson` from the
  `natural-earth-vector` repository (5.2.0-pre), packed into
  `src/geo/land.bin` by the converter in `src/geo/land.rs`, whose test
  rebuilds it on every run. They are the coastlines on the FT8 map.
- **The FT4 and FT8 protocols' reference files**: S. Franke, B. Somerville
  and J. Taylor, "The FT4 and FT8 Communication Protocols", QEX July/August
  2020, and its public-domain reference programs and tables (the ARRL QEX
  files page). The LDPC code's generator and parity-check tables are
  `src/signal/ft8/ldpc_generator.dat` and `ldpc_parity.dat`, unaltered.
- **NOAA's General Solar Position Calculations** (Global Monitoring
  Division): the declination and equation-of-time series behind the map's
  day, twilight and night, in `src/geo/sun.rs`.

## Development approach
- **Core code**: Written and reviewed by humans
- **AI assistance**: Used Claude Code for implementation, architecture discussion, and problem-solving
- **Quality gate**: All changes reviewed and tested by humans before merge

## Tools
- [Claude Code](https://claude.com): development environment
- Rust ecosystem (Cargo, rustc, etc.)

## The reality
We use AI tools as part of our workflow because they make us faster and smarter. That's normal in 2026. What matters is that humans stay in control and code gets reviewed properly. This project does both.
