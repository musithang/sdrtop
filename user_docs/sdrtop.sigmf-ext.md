# The `sdrtop` SigMF Extension Namespace

← [Back](README.md)

This file defines the `sdrtop` extension namespace, version 1.0.0, as SigMF
1.2.6 section 1.9.2 asks: every field sdrtop writes outside `core` is described
here. The namespace is listed in `core:extensions` with `optional: true`, so a
tool that has never heard of sdrtop can ignore all of it and still read the
recording correctly.

The recordings themselves are ordinary SigMF: a `.sigmf-data` file holding the
samples exactly as the radio delivered them (`ci8` from a HackRF, `cu8` from an
RTL-SDR, `ci16_le` through SoapySDR) and a `.sigmf-meta` file beside it.

## 1 Global

| name | required | type | description |
|---|---|---|---|
| `sdrtop:full_scale` | true | double | The sample value that means full scale. Divide by it to get samples in the range -1 to 1, the way sdrtop reads them. For `cu8` the zero is 127.5 and the value is measured from there. |
| `sdrtop:bits` | true | uint | How many bits of the container the converter fills. A `ci16_le` recording from SoapySDR is often a 12-bit converter in a 16-bit container. A `ci32_le` recording carries a converter's words with fraction bits below them, left by the radio library's own processing: `sdrtop:full_scale` is then the converter's full scale times a power of two, and this field is the converter's depth. |
| `sdrtop:stack` | false | string | The software between sdrtop and a radio with no firmware of its own, with its version, for example the SoapySDR runtime. Absent on a radio that has firmware. |
| `sdrtop:gain` | true | string | The gain chain when the recording started, by stage name as the radio's driver names them (`LNA=16,VGA=20`), and the front end boost when the radio has one. A stage whose gain in dB was not measured at the frequency being recorded is written as its step instead (`LNA=step 14/14`). Changes during the recording are annotations labelled `gain`. |
| `sdrtop:reference` | true | string | The frequency reference sdrtop held when the recording started, or that it held none. **The samples are never corrected by it**: it is here so a reader who wants absolute frequencies has what they need. |
| `sdrtop:iq_correction` | true | string | The DC block and I/Q correction sdrtop was showing on screen. **The file never has them applied**; this says what the panels saw, so both stories can be told from one recording. `none` when neither was on. |
| `sdrtop:state` | true | string | `recording` while the file is still growing, `finished` once it has stopped. The metadata is rewritten once a second, so a file left behind by a crash says `recording` and is complete up to its last second. |
| `sdrtop:stopped` | false | string | Why the recording ended, as a sentence. Present once `sdrtop:state` is `finished`. |
| `sdrtop:pairs_written` | true | uint | Samples in the `.sigmf-data` file. |
| `sdrtop:lost_driver` | true | uint | Samples the radio's driver reported it dropped. |
| `sdrtop:lost_queue` | true | uint | Samples sdrtop could not hand to the disk in time, because the queue in front of it was full. |
| `sdrtop:lost_unexplained` | true | uint | Samples missing from the stream that nobody reported. Normally zero; counted, not assigned to a cause. |
| `sdrtop:wall_seconds` | true | double | System-clock seconds from the first block to the last. |
| `sdrtop:delivered_rate` | false | double | Samples per second the stream actually advanced by against the system clock, reported drops included. Present once the recording has spanned two seconds. |
| `sdrtop:radio_drops` | false | uint | Drops the radio counted inside itself while the recording ran: each a moment its own buffer was full and samples were thrown away before any reached the host. Present when the radio keeps such a count (a HackRF does), and 0 is a reading, not an absence. Each is placed in the file by a `radio dropped N` annotation. |
| `sdrtop:radio_drops_unreadable` | false | string | The radio keeps a count and it could not be read, with the reason. |
| `sdrtop:shortfall` | false | string | Present when `sdrtop:delivered_rate` is more than 1 % below `core:sample_rate`: the radio delivered fewer samples than its rate and reported no drop. The missing samples are somewhere in the file, at places nothing recorded, so a sample's position divided by the rate is **not** its time. A HackRF that USB cannot keep up with does this. |

## 2 Captures

No `sdrtop` fields. How sdrtop uses the `core` ones:

- A new capture segment starts at the beginning, after every stretch of
  missing samples, and at every retune. `core:global_index` is the sample's
  position in the stream the radio delivered, counted from the first sample
  offered to the recording, so it runs ahead of `core:sample_start` by
  exactly what is missing so far.
- `core:frequency` is the frequency sdrtop had set when the block arrived.
  The radio's own settling after a retune is not measured and not in the
  file.
- `core:datetime` is the system clock when the block holding the segment's
  first sample reached sdrtop: measured for every segment, never worked out
  from the sample count, because a radio can deliver fewer samples than its
  rate without saying so (`sdrtop:shortfall`). It is after the driver's own
  buffering, so late by up to a block, a few milliseconds.

## 3 Annotations

No `sdrtop` fields. `core:generator` is `sdrtop`. Every annotation marks a
moment, with `core:sample_count` 0, except `radio dropped N`, which marks the
stretch a drop inside the radio is known to lie in: such a drop leaves no mark
in the stream, so it is placed between two readings of the radio's count, and
the first reading of a recording may bracket a little from before it began.
The labels:

| `core:label` | at | `core:comment` says |
|---|---|---|
| `lost N` | the first sample after a hole | how many samples are missing, how long that is, and how many of them the driver dropped, the queue refused, or nobody explained |
| `tuned F MHz` | the first sample at a new frequency | that the radio's settling is not in the file |
| `gain` | the first block after a gain change | the new chain, set by the time the block arrived, not necessarily applied by the radio at that sample |
| `radio dropped N` | the stretch the drops lie in, with its `core:sample_count` | that the radio dropped samples N times somewhere in the stretch, the longest in ms, and that the samples either side are joined in the file |
| `stopped` | the end of the file | why the recording ended |
