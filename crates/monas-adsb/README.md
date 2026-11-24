# monas-adsb

The 1090 MHz Extended Squitter decoder for the
[Monas workspace](https://github.com/musithang/sdrtop). The current crate
validates DF17 frames and decodes identification, airborne altitude and
airborne velocity. It resolves global airborne CPR positions from a
compatible even and odd pair and returns the sample provenance of both
receptions. The frame retains the original bytes for inspection.

The preamble detector proposes ranked fractional timings in an IQ power
envelope. The PPM slicer uses a timing to recover the payload by fractional
window overlap and can return the two measured half-cell energies per bit.
The streaming receiver scans a sequence of envelope blocks with absolute
sample positions. It retains the working window across block boundaries
and reports CRC-checked frames and other candidate outcomes.

To replay a Monas 1090 MHz `ci16_le` recording, pass its SigMF metadata
path to `cargo run -p monas-adsb --example replay_sigmf -- <capture.sigmf-meta>`.
The `--mean-dc` flag subtracts the capture's complex mean for offline
comparison. Accepted frames go to stdout as TSV, and receiver counters
go to stderr.

Airborne altitude follows the DO-260C Q-bit encoding. It distinguishes
barometric and geometric sources and exposes 100-foot Gillham codes as
raw values. Airborne velocity distinguishes ground components, IAS or
TAS, heading and vertical rate. Speed magnitudes distinguish missing,
zero and saturated reports.

The crate follows the workspace version and has no stability promise of
its own.
