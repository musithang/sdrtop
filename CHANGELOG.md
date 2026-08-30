# Changelog

All notable changes to sdrtop are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

**While the major version is 0, a minor bump may break the config file.** That
file is the user-visible compatibility contract: `~/.config/sdrtop/config.toml`
is rewritten on quit, and a version that changes its shape says so here under
*Changed*. Loading stays forgiving in both directions, so a missing or unknown
field falls back to a default rather than refusing to start.

For the same story told as narrative rather than as a list, see
[`user_docs/whats-new.md`](user_docs/whats-new.md), which is organised by
checkpoint instead of by version.

## [Unreleased]

## [0.4.2] - 2026-08-29

> 🎧 Written on two days without sleep, most of which were spent losing an
> argument to a YAML file. It is a good release. I would not do it again.
>
> Soundtrack, if you want the authentic experience:
> [What's Happening 2 BATTLE](https://www.youtube.com/watch?v=eYWDZrn3ptQ)

**No radio-facing code changed.** Not one line. The spectrum is the spectrum,
RDS still decodes, the FM discriminator is still hand written and still correct.
What changed is everything about how sdrtop reaches you.

This is the first release built end to end by the current pipeline: reproducible
bytes, a signed provenance attestation, and a release page written by a human
instead of assembled from commit subjects.

Things that were true a week ago and are no longer true:

- the checksum check in the installer was a **decoration**. It lived inside an
  `if` whose every branch continued, so a network hiccup silently skipped it
- the build container **never rebuilt** after its recipe changed
- "what version is this" had **two different answers**, one of which was a
  regular expression applied to JSON
- there was, briefly, a plan to ship a `.dmg`. On Linux.

Things that are true and will remain true forever:

- Debian says `librtlsdr0`, Ubuntu says `librtlsdr2`, and they are the same
  source code
- docs.rs says "sdrtop is not a library", and docs.rs is right
- it is `release.yaml`. With an `a`.

### Added

- **`sdrtop --version` now names the commit**, like `0.4.2 (a1b2c3d)`, with a
  `-dirty` marker when it was built from an edited tree. A bug report saying
  "0.4.2" now identifies exactly one tree, which matters because `install.sh`
  can hand you the `main` branch on request.
- **Signed build provenance.** Release artefacts carry a Sigstore attestation
  binding them to this repository and this workflow. Verify with
  `gh attestation verify <file> --repo mustang6139/sdrtop`. The checksum says a
  download arrived intact; this says where it came from.
- **`install.sh --git`** builds the `main` branch, and **`install.sh
  --no-verify`** skips the checksum for whoever has a reason.
- **`RELEASING.md`**, the release procedure written down instead of remembered.

### Changed

- **`install.sh` no longer builds sdrtop itself.** It used to carry its own
  download-and-compile pipeline, a second unmaintained copy of the build recipe.
  It now does the two jobs cargo cannot (your distribution's libraries, and a
  Rust new enough to matter) and hands the rest to
  `cargo install sdrtop --locked`.
- **The installer reports on device permissions instead of arranging them.** It
  no longer writes its own udev rules or edits your groups. The `libhackrf` and
  `rtl-sdr` packages ship rules already, and a second set that agrees only by
  coincidence is worse than one.
- **The release tarball is named after the full Rust target triple**,
  `sdrtop-0.4.2-x86_64-unknown-linux-gnu.tar.gz`. The old short form did not say
  which libc, which is the exact axis this project has trouble on. `install.sh`
  still understands the old name for 0.4.1.
- **Release notes come from this file**, not from a list of commit subjects.
- Publishing to crates.io happens automatically on a tag, through Trusted
  Publishing, before the GitHub release is drafted.
- The README documents three install paths in order, and states plainly which
  machines the prebuilt binary actually serves. It does not serve Ubuntu or Mint.

### Fixed

- **Checksum verification could silently not happen.** The check lived inside an
  `if` whose every branch continued, so a failed `SHA256SUMS` download, or a
  machine without `sha256sum`, installed an unverified binary without a word. It
  now stops.
- **The build container was never rebuilt after its recipe changed.** Editing
  `Containerfile` on a machine that already had an image produced a tarball
  built by the old recipe, with nothing on screen to say so.
- Two different rules decided what "the version" was, one of them a regular
  expression applied to `cargo metadata` JSON. There is now one.
- `README.md` claimed the installer adds you to `plugdev`, which it no longer
  does, and its one example of `--version` pointed at a release that does not
  exist.

### Internal

- **The release tarball is reproducible.** The same commit built twice produces
  the same bytes, archive and binary alike: base image pinned by digest,
  compiler pinned, `SOURCE_DATE_EPOCH` from the commit date, deterministic `tar`
  and `gzip -n`.
- The release workflow asserts the glibc floor, the exact set of shared
  libraries the binary needs, and that the binary reports the version the
  archive is named after.
- CI gained an MSRV job on 1.88, `--locked` everywhere, `cargo package` on every
  push, and shellcheck.

### Removed

- The `.deb` matrix, the architecture matrix, and the QEMU-emulated `armhf`
  runner that built a package for a machine nobody owned and then tested it on a
  machine that did not exist. It is in the git history if anyone gets nostalgic.
- The plan to ship a `.dmg`. For a terminal application. On Linux.
- 48 consecutive hours of the maintainer's sleep. Unlike a crates.io version,
  this cannot be yanked either.

<!--
     .  *  .   .
  .    \ | /    .           you found it
.   --== 📡 ==--   .
  .    / | \    .        0.4.2 was assembled across 48 hours
     .  *  .   .         without meaningful sleep, in a fight
                         that was ultimately against a YAML file.

                         the YAML file lost. eventually. at 04:00.

                         if you are reading the raw markdown of a
                         changelog looking for jokes, you are exactly
                         the kind of person this program was written
                         for. plug in a radio. press space.

                                              73 de sdrtop 📻
-->

## [0.4.1] - 2026-08-29

The first release published to [crates.io](https://crates.io/crates/sdrtop), so
`cargo install sdrtop --locked` works from here on. Packaging only: the program
behaves exactly as 0.4.0 did, and no radio-facing code was touched.

### Changed

- The crate is publishable. `user_docs/pics`, `packaging/`, `.github/` and
  `dev_docs/` are excluded from the source package, which takes it from 278
  files and 18 MB to 256 files and 1.8 MB, 516 KB compressed. The 15.8 MB demo
  video alone put it over the crates.io ceiling.
- `README.md` addresses its screenshots and demo video by absolute URL. They are
  no longer inside the source package, so a relative path would render broken on
  the crates.io page.
- Container runtime detection (podman, then docker) lives in
  `packaging/build-tarball.sh` rather than in a shared helper, now that one build
  script is left to use it.

### Fixed

- `build.rs` no longer fails on docs.rs. The `libhackrf` pkg-config probe is
  skipped when `DOCS_RS` is set, since docs.rs cannot install system packages
  and `cargo doc` never links the binary.

## [0.4.0] - 2026-08-29

### Lab Signal

Every instrument up to here described a signal from the outside: how strong, how
wide, how clean. Lab Signal (`8`) opens the channel and reads what is inside it.
There is still no audio in sdrtop and there is not going to be. This is a bench.

Two focus modes answer two questions: what is this signal, and what is it
carrying?

### Added

- **FM MPX and Demod** (focus `m`).
  - Deviation, peak and RMS, measured about the carrier, so a mistuned radio
    reports its error as offset instead of inflating the modulation figure.
    Drawn against 75 kHz broadcast or 5 kHz narrowband.
  - MPX baseband from 0 to 60 kHz as a live profile, with the 19 k pilot, the
    38 k stereo difference and the 57 k RDS subcarrier visible where they live.
  - Pilot and stereo: STEREO / MARGINAL / MONO, plus injection percentage
    against the 8 to 10 % norm. That reads the transmitter's health, not your
    reception.
  - RDS: station name, PI code, programme type, traffic flags and RadioText.
    Nothing is shown until it has been confirmed twice, so no character appears
    and then gets corrected in front of you.
  - CTCSS against the standard 40 tone table, with the margin it won by and an
    honest "searching" while its window fills.
  - AM depth, positive and negative peaks separately, because a negative peak
    near 100 % pinches the carrier off and splatters.
  - Tune the demodulated channel inside the capture with `←`/`→`. `P` snaps to
    the strongest carrier and ignores the local LO leakage, `T` overrides the
    classifier.
- **Signal Characterization** (focus `x`).
  - Occupied bandwidth by the 99 % method, bounded to the carrier.
  - Channel power integrated over the channel rather than the capture.
  - ACPR on the adjacent bands, each sized from the measured occupancy. An
    undefined ratio reads undefined, never a guess.
  - A modulation classifier that is honest about being a bandwidth heuristic,
    and a verdict card that says what it thinks and why.
- Themes and layouts are files. A TOML dropped in `~/.config/sdrtop/themes/` or
  `~/.config/sdrtop/presets/` exists on the next start. Nothing to register,
  nothing to rebuild.
- `--version`, and a generated man page.
- One-line install for any distribution via `packaging/install.sh`, plus an
  x86_64 tarball and `SHA256SUMS` on the release page.

All of the signal processing is hand written: the FIRs, the FM discriminator,
the AM envelope detector, the Goertzel tone search, and the whole RDS chain down
to the CRC. The only maths dependencies in the program are an FFT for drawing
the MPX spectrum and a complex number type.

### Changed

- **One saturation scale.** SAT used to read green on the Command Rail and red
  in the micro views at the same instant, because two functions with the same
  name escalated at 1 % / 5 % and at 10 % / 50 %. The number means one thing
  everywhere now, and the reassuring half moved to clip headroom, where it
  belongs.
- Occupied bandwidth is bounded to the carrier, not to the capture. A broadcast
  station at 10 Msps used to report 7.49 MHz, which was the sample rate read
  back at you.
- Channel power no longer grows when the span widens.
- The waterfall buffer is deeper, so `J` and `K` have more history to scroll
  back through.
- The declared MSRV is 1.88, which is what the source has actually required
  since `slice::as_chunks` entered the hot path. The README claimed 1.78, which
  was the lockfile floor; a toolchain between the two compiled the lockfile and
  then failed on the source.

### Fixed

- **RadioText never arrived.** Every gap in reception threw away half confirmed
  characters, and RadioText needs eleven unbroken seconds where a station name
  needs two.
- `+` and `-` were dead keys on the spectrum-only view (`2`), which also
  discarded any zoom set on the rail.
- The waterfall left a blank strip on tall terminals.
- On RTL-SDR the field gain rows started in three different columns, because the
  labels were sized for three-letter names and `Tuner` is five. HackRF was never
  affected, which is why it went unnoticed.
- Per-field theme overrides were deleted on quit. `save_config` rewrites the
  whole file, so anything the running app did not hold in memory was lost.
- Keys `[1]` to `[4]` reported switching presets when they had not.

### Internal

- No function is over 200 lines, down from fourteen that were.
- Tests went from roughly 550 to 831.
- Built-in presets and themes are data (`config/presets/*.toml`,
  `theme/palettes/*.toml`) rather than code.

## [0.3.5] - 2026-06-28

### Added

- **Lab Timing** (`7`), the real-time bench: `TimingVitalsPanel` with link
  utility and buffer telemetry, `TimingDiagnosticsPanel` for pipeline
  profiling, and `TimingStripchartPanel`, a zero-centred bipolar braille drift
  trace that tags the direction of over-range spikes.
- Rolling callback gap tracking and real-time deadline budget metrics in the RX
  path.
- A reusable zero-centred `bipolar_braille_strip` chart widget.
- Gain clamping, so a config saved on one device family is snapped into the
  other's legal range at startup rather than rejected.

### Changed

- Timing views use the lab-standard coloured bars and airy layouts.
- `fit_spacers` replaces `collapse_spacers` for panel density, and `pad_to_fill`
  handles the leftover rows.
- Lab side-panel subheadings share one `section` helper.

### Fixed

- Carrier detection folds the noise floor into its SNR calculation.
- Frequency bounds in spectrum and waterfall focus handling.
- The AGC flag stayed out of sync with manual gain on single-tuner devices.
- Body panel specs were rendered without a visibility check.

## [0.3.0] - 2026-06-27

### Added

- **Command Rail** (`1`, and now the default view): a left instrument rail with
  a segmented frequency hero, an analog S-meter, the HUNT / MONITOR / BENCH mode
  tabs whose lead card follows what you are doing, recall slots with live
  activity pips, and a SIGNAL zone where SNR, PWR, NF and SAT each ride their
  own braille trace beside the value.
- **Lab RF rebuilt as a front-end bench** (`6`): RF Diagnostics (gain lineup,
  staging, Friis noise figure, sensitivity, verdict), a Gain-Staging Level
  Diagram tracing signal and noise through ANT, LNA, MIX, VGA and ADC, and an
  ADC Loading panel with a signed-sample histogram and a modeled linearity card.
  Focus with `D`, `A` to auto-stage the gain, space to freeze the bench.
- **Lab IQ redrawn**: analog null meters where centre is ideal and the needle
  shows deviation, paired with a persistence constellation whose fitted ellipse
  reports amplitude imbalance as stretch and phase imbalance as tilt.
- Cascaded RF chain modelling, Friis noise figure and linearity metrics in
  `signal/`.
- A live Image-Rejection Scope with auto-tracking carrier and image markers, and
  manual pinning with `M`.
- One-shot auto-gain and AGC-lite tracking behind the RF bench focus mode.

### Changed

- Panels self-adjust their spacers, so a stack breathes or compresses to the
  terminal height instead of clipping.
- Noise-figure colour thresholds are bound to the live NF measurement.
- The noise figure label reads FLR for consistency.
- RF diagnostics and noise bars share the `gain_bar_colored` widget.

### Fixed

- Waterfall frame averaging and row reads survive a change in bin count.
- Frequency mapping drift and signed image suppression in the IQ path.
- The clip row shifted the layout vertically; it now sits in the SAT padding
  slot.
- IQ diagnostics sparkline overflow, with a responsive chip fallback.
- The Image-Rejection Scope left an empty gap instead of taking the height.
- Gain calculation uses ADC peak levels.

## [0.2.0] - 2026-06-23

First tagged release. The point where the TUI became feature-complete and
sdrtop stopped being a one-radio program.

### Added

- **RTL-SDR support** (R820T, R828D, E4000) alongside the HackRF One, behind the
  `SdrDevice` abstraction. Both radios share the same RX, FFT and UI pipeline,
  in normal RX and in observer mode. A device picker appears when more than one
  is attached, and `--device hackrf|rtlsdr` pins one.
- **Observer mode**: when another process holds the radio, sdrtop falls back to
  sysfs-derived information rather than refusing to start.
- The preset and layout engine, preset hotkeys `[1]` to `[0]`, the lab presets,
  the micro field views, and structural panel-focus routing.
- Native wideband sweep (`lab_sweep`, `micro_sweep`) with runtime boundary
  adjustment.
- Spectrum and waterfall, the IQ constellation with braille phosphor density
  layers, reference grids and covariance ellipses, and the IQ diagnostics panel
  with analog null meters.
- Real-time streaming timing diagnostics, session jitter-peak tracking, and
  CPU/RSS metrics.
- Radio math: minimum detectable signal, PAPR, DC spike tracking in dBFS,
  image rejection ratio, wavelength and antenna metrics.
- Config file with atomic save on quit, and the CLI flags that override it.

[Unreleased]: https://github.com/mustang6139/sdrtop/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/mustang6139/sdrtop/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/mustang6139/sdrtop/compare/v0.3.5...v0.4.0
[0.3.5]: https://github.com/mustang6139/sdrtop/compare/v0.3.0...v0.3.5
[0.3.0]: https://github.com/mustang6139/sdrtop/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/mustang6139/sdrtop/releases/tag/v0.2.0
