# Recording the IQ stream

← [Back](README.md)

`Ctrl+R`, from any layout, records the raw samples the radio is delivering. `Ctrl+R`
again stops it. That's the whole interface, and you can use it with every
other key, because a recording is a state of the whole app, like streaming,
not a panel.

What you get is a pair of files any SDR tool can open, and enough written in
them that someone else can trust the recording, or know exactly why not:

```
~/.local/share/sdrtop/iq-20260927-101500.sigmf-data
~/.local/share/sdrtop/iq-20260927-101500.sigmf-meta
```

(`$XDG_DATA_HOME/sdrtop/` if you have set that.) The format is
[SigMF](https://sigmf.org), which GNU Radio, inspectrum, SigDigger, URH and the
`sigmf` Python package all read.

---

## What is in the files

**`.sigmf-data` is the samples, exactly as the radio delivered them**: 8-bit
signed from a HackRF (`ci8`), 8-bit unsigned from an RTL-SDR (`cu8`), 16-bit
through SoapySDR (`ci16_le`). No header and no conversion. If `D` (DC block)
or `C` (I/Q correction) was on in the Lab, the panels showed you corrected
samples and the file still holds the raw ones, because a correction can be
applied to a recording afterwards and never taken out of one. The metadata
says which corrections were on.

**`.sigmf-meta` is what the recording knows about itself**: the rate, the
radio and its serial, the gain chain by stage name, the frequency reference
if you had one, and where each piece of the file sits in time. sdrtop's own
fields are described in [the `sdrtop` extension](sdrtop.sigmf-ext.md), so a
reader doesn't have to guess what `sdrtop:lost_queue` means.

## When something goes missing

A recording that lost samples and doesn't say where is worse than no recording:
you'd measure a gap as a signal. So every missing stretch is written down, to
the sample:

- a new **capture segment** starts after it, with `core:global_index` saying
  where that sample really was in the stream the radio delivered, so the
  numbers run ahead of the file by exactly what is missing;
- an **annotation** labelled `lost N` says how many samples, how long that
  was, and who lost them: the radio's driver, sdrtop's queue in front of the
  disk, or (normally never) nobody who admitted it.

A retune doesn't stop the recording. It starts a new segment at the new
frequency, with a `tuned` annotation, so you can record the NET survey walking
the band and know where each dwell begins. A gain change gets a `gain`
annotation. A change of sample rate does stop it, because a SigMF recording has
one rate.

There is one kind of loss nobody reports, and it's the sneaky one: **a radio
that simply delivers fewer samples than its rate**. The positions stay unbroken,
the driver says nothing, and the file quietly holds 20 seconds of samples for 30
seconds of sky. Only a clock can see it, so every block's arrival is timed on
the system clock, every capture segment's `core:datetime` is that measured
arrival (never worked out from the sample count), and once the recording has
run two seconds it compares the two. More than 1 % short, and the file gets an
`sdrtop:shortfall` sentence saying so, the log repeats it, and the header shows
it. From then on, treat a sample's position as a position, not a time.

A HackRF goes one better: it counts its own drops, the moments its buffer was
full and samples went nowhere. sdrtop reads that count while it streams, and a
recording writes each one down as a `radio dropped N` annotation over the
stretch of samples it lies in, with the total in `sdrtop:radio_drops`. A stretch
rather than a sample, because a drop inside the radio leaves no mark in the
stream: the samples either side of it arrive joined, and the count is read five
times a second. The same count is on the Timing bench (`Receiver 3`), recording or
not.

While it runs, the header shows `● REC 12.4 s · 496 MB`. That's the length of
the file, samples over rate, not the time since you pressed the key. The first
missing sample adds a second chip, `lost 3.2 ms`, the radio's own drops add
`radio dropped 7`, and a radio running short adds `short 31 %`. They are
different numbers on purpose.

## What stops it

- `Ctrl+R` again. Whatever was already queued is still written: it was
  captured before you pressed the key.
- The limit: **60 seconds or 4 GB, whichever comes first**, set in
  [`[record]`](config.md#the-whole-file). 20 Msps from a HackRF is 40 MB a
  second, so the minute runs out first; an RTL-SDR at 2.4 Msps writes
  4.8 MB/s, 290 MB a minute.
- Stopping the stream with `Space`, the radio stopping on its own, a new
  sample rate, or the disk refusing a write.
- Quitting, with `q` or `Ctrl+C`: sdrtop waits for the last write so the file
  says `finished`.

The reason goes into the file as its last annotation and into the log with the
path. The metadata is also rewritten once a second while the recording runs,
so a crash or a pulled cable still leaves a pair that opens, complete up to its
last second, and saying `recording` rather than `finished`.

## When it won't start

It says why in the log and writes nothing when:

- the radio isn't streaming (press `Space` first);
- the device is a tinySA, which delivers calibrated power traces rather than
  samples, so there is nothing of this kind to record;
- sdrtop is only [observing](advanced.md) a radio another program holds;
- the disk doesn't have room for the limit, plus a little. It checks before
  the first byte, so a full disk isn't discovered at the last one.

## How fast a machine it needs

The disk was not the problem on the old i3 this is developed on: its laptop SSD
took a full minute of 20 Msps from the recorder without refusing a single
block, and handing a block over cost the USB callback about a microsecond and a
half.

sdrtop was. The first real recording, a HackRF at 20 Msps, came back with 21
seconds of samples for 31 seconds of wall time, and nothing had reported a
single drop: sdrtop's own work was holding up the USB thread long enough for the
radio's buffer to overflow. That is fixed: the work moved off the USB thread,
and the same machine now streams 20 Msps whole.

Recording at 20 Msps to that laptop's encrypted disk still costs a little: the
kernel's encryption competes for the same two cores, and in fifteen seconds the
radio dropped samples about seven times. Every one of them is in the file, in
the header and on the Timing bench. If a recording has to be whole, record to a
disk that is not encrypted, or lower the rate, and watch the `radio dropped`
chip stay away.

A Raspberry Pi writing to an SD card will be slower still, and will say `lost`
or `short`, loudly. Nobody has measured one yet, so that's a warning, not a
number.

## Opening a recording

In Python, with the `sigmf` package:

```python
import sigmf
rec = sigmf.sigmffile.fromfile("iq-20260927-101500")
samples = rec.read_samples()          # complex, scaled to about ±1
for seg in rec.get_captures():
    print(seg["core:sample_start"], seg.get("core:global_index"), seg["core:frequency"])
```

Or plain numpy, for a HackRF recording:

```python
import numpy as np
raw = np.fromfile("iq-20260927-101500.sigmf-data", dtype=np.int8)
samples = (raw[0::2] + 1j * raw[1::2]) / 128.0   # sdrtop:full_scale
```

For an RTL-SDR (`cu8`) the zero is 127.5: `(raw.astype(float) - 127.5) / 127.5`.

inspectrum and IQEngine open the pair directly. sdrtop's annotations mark
moments (they have no length and no frequency edges), so a viewer that draws
boxes may show them as nothing; the `annotations` list in the `.sigmf-meta` is
the account to read.

## What it doesn't do

Play a recording back into sdrtop, trim it, decimate it, or convert it to
floats. The file is what the radio said. And, as everywhere in sdrtop, nothing
here transmits.
