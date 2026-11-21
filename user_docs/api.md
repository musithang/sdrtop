# The monasd API

<!-- Generated from the source by crates/monasd/src/api_doc.rs. Do not edit:
     MONAS_WRITE_API=1 cargo test -p monasd api_reference writes it anew. -->

← [monasd](monasd.md)

---

This page is written by a test, from the code that serves the API: the
routes from the router, every field from the type it is serialised from,
each description from the comment above it in the source. If the two ever
disagree, the test fails before a release does. So everything here is true
of the version it came with, and a little dry, because I didn't write it.
[monasd](monasd.md) is the page with the context, the token and examples.

Everything is under `http://ADDR/v1`, JSON in and out. Off loopback every
request carries `Authorization: Bearer TOKEN`.

**Contents:** [Conventions](#conventions) · [Routes](#routes) ·
[Commands](#commands) · [Events](#events) · [Types](#types)

## Conventions

**A reading** is `{"value": ...}` or `{"absent": "why"}`, never a bare
number: a figure the screen would dash arrives as the reason it is missing,
so no client can read it as zero. Written below as *X or absent*.

**A group** is a set of readings with its testimony beside them: the
object has a `testimony` key, and the group's own fields at the same level.
Written below as *group of X*.

**`?since=N`** on a list leaves out the entries numbered `N` and below, so a
client that polls is sent each entry once.

**`null`** is a field that has nothing to say for this entry, for the reason
its description gives: a message with no locator, a stage with no measured
dB here, a header that could not be read.

**The testimony** of a group:

| Field | Type | |
|---|---|---|
| `at_unix_ms` | integer or null | When it was measured, in milliseconds since the Unix epoch: for a reader on another machine. Reported as stamped; never used to decide staleness, which runs on the monotonic clock. |
| `stale` | boolean | Whether it has stopped being live, by its `Staleness` rule. |
| `feed_loss` | boolean | Whether the feed lost samples inside the span it covers, by its `FeedSpan`: its counts are then lower bounds. |
| `absent` | string or null | Why there is no value, where there is none. |

## Routes

| Route | What it serves | Returns |
|---|---|---|
| `GET /v1/health` | Whether the radio is delivering, and which radio, where, doing what; the samples lost this session, split as the timing bench splits them; each sample feed's own account; and what the process costs the machine. | [`Health`](#health) |
| `GET /v1/radios` | The radios found at start, verified or reasoned, and which one runs. | list of [`server::Radio`](#serverradio) |
| `GET /v1/config` | The engine's blocks of the config as they would be saved now. Only the engine's: the server's own block, with its token, is not among them, so there is nothing to withhold. | the engine's blocks of the config, as JSON |
| `GET /v1/snapshot` | Everything the engine reads: each reading a value or the reason it has none, each group with its testimony. Built from a copy of the state, so the engine's lock is held only for the copy. | [`Snapshot`](#snapshot) |
| `GET /v1/provenance` | Where the measurements come from: the same fields an export's header carries, by the same names. | an object: `monas` (the version) and each header field by name, as text |
| `GET /v1/spectrum` | The latest spectrum frame with its testimony, or why there is none. Its own route, so a client that wants the figures is not sent thousands of bins with every snapshot. | group of [`spectrum::Frame`](#spectrumframe) or absent |
| `GET /v1/pager` | The pages the pager shows, as it shows them: masked, only the kind and length; answering one address, only that address's. | group of [`Pages`](#pages) |
| `GET /v1/ft8` | The FT8 decodes held, each with what it measured beside what it said: the CSV export's account, as JSON. | group of [`Decodes`](#decodes) |
| `GET /v1/net/band` | The 2.4 GHz band's occupancy, one megahertz to a cell: the occupancy export's account, unobserved cells unobserved rather than zero. | group of [`Band`](#band) or absent |
| `GET /v1/net/census` | The census of transmitters, in the panel's order, addresses shown as the panel shows them: the census export's account. | group of [`Census`](#census) |
| `GET /v1/net/ble` | The LE 1M and 2M packet list as it is showing, newest first, each with what was measured of it: the BLE export's account. `?since=N` sends only packets numbered after `N`. | group of [`Packets`](#packets) |
| `GET /v1/net/coded` | The LE Coded packet list, the same way. | group of [`Packets`](#packets) |
| `GET /v1/net/bt` | The classic Bluetooth hits kept, oldest first: the BR/EDR export's account. | group of [`Hits`](#hits) |
| `GET /v1/net/fer` | The frame error rate against SNR, all traffic and each device: the FER export's account. | group of [`Fer`](#fer) |
| `GET /v1/net/wifi/survey` | The Wi-Fi survey: each channel's busy share over its visits, its airtime by family, and which channel is least busy or why none can be named yet. | group of [`Survey`](#survey) |
| `GET /v1/net/wifi/frames` | The Wi-Fi frames heard on the radio's centre, newest first, worded as the Frames list words them. `?since=N` sends only frames after `N`. | group of [`Frames`](#frames) |
| `GET /v1/net/wifi/airtime` | The locked channel's airtime: its share, who spent it, and the rhythm of the gaps. | group of [`Airtime`](#airtime) |
| `POST /v1/command` | One command, answered: 200 when applied, 422 with the reason when not. | [a reply](#commands) |
| `GET /v1/events` | The events, as they happen, one JSON message each. | [a WebSocket of events](#events) |

## Commands

`POST /v1/command` takes one command as JSON, named by its `command` field. It is answered `200` with `{"reply": "applied", ...}` when it was done, or `422` with `{"reply": "refused", "reason": ...}` when it was not, and then nothing was changed.

| Name | Fields | |
|---|---|---|
| `"command": "tune"` | `hz`: integer | Tune to this frequency. |
| `"command": "sample_rate"` | `hz`: number | Run at this sample rate (on a swept analyser, this span). |
| `"command": "gain"` | `stage`: string<br>`db`: number | Set one gain stage, by the name the radio gives it. |
| `"command": "gains"` | `values`: list of number | Set every gain stage at once, front to back. |
| `"command": "agc"` | `on`: boolean | Switch the tuner's own AGC. |
| `"command": "boost"` | `on`: boolean | Switch the front-end boost: an amplifier, or the tuner's automatic gain, whichever the radio's boost is. |
| `"command": "reset"` |  | Back to the radio's own defaults: rate, gains, frequency, boost off. |
| `"command": "set_mode"` | `section`: string<br>`view`: string | Compute what this view in this section asks for, by the terminal's own names for both. |
| `"command": "stream"` | `on`: boolean | Stream from the radio, or stop. |
| `"command": "dc_block"` | `on`: boolean | Take the DC offset out of the samples the spectrum, the demodulator and the constellation see; the statistics stay on the raw samples. |
| `"command": "device_option"` | `id`: string<br>`choice`: string | Set one of the radio's own options, by its id, to one of its choices. |
| `"command": "addresses"` | `show`: string | Show addresses `full`, by registrant (`oui`) or `masked`, on every screen and in every export and API answer alike. |
| `"command": "pager"` | `masked`: boolean<br>`mine`: integer or null | The pager's view: text `masked` to its kind and length, and the one address it answers to (`mine`, none for every address). |

The reply:

| Name | Fields | |
|---|---|---|
| `"reply": "applied"` | `detail`: string | Done; what the radio is now set to. |
| `"reply": "refused"` | `reason`: string | Not done, and why. Nothing was changed. |

## Events

`GET /v1/events` opens a WebSocket that sends one JSON message per event, each as it is delivered to this client:

| Field | Type | |
|---|---|---|
| `event` | [`Event`](#events) |  |
| `lost_before` | integer | Events this client missed, its queue full, just before this one. |

The event, named by its `event` field:

| Name | Fields | |
|---|---|---|
| `"event": "applied"` | `command`: [`Command`](#commands)<br>`reply`: [`Reply`](#commands) | A command was decided and answered, by whoever sent it. |
| `"event": "log"` | `at_epoch_secs`: integer<br>`level`: [`LogLevel`](#loglevel)<br>`text`: string | A line was added to the log. |
| `"event": "gap"` | `at_unix_ms`: integer<br>`samples`: integer<br>`driver`: integer<br>`sdrtop`: integer | Samples went missing from the stream, split by whose loss it was. |
| `"event": "stale"` | `stale`: boolean | The radio stopped delivering (`stale: true`) or started again. |

## Types

### Health

How the radio and the process are doing.

| Field | Type | |
|---|---|---|
| `radio` | string | The radio's name. |
| `streaming` | boolean |  |
| `stale` | boolean | Whether its readings have stopped being live. |
| `frequency_hz` | integer |  |
| `sample_rate_hz` | number |  |
| `view` | string | The view whose work the engine does. |
| `lost_samples` | [`LostSamples`](#lostsamples) | Samples lost this session, and whose loss each was. |
| `feeds` | [`Feeds`](#feeds) | Each sample feed's refusals: blocks it had no room for. |
| `process` | [`Process`](#process) |  |

### server::Radio

One radio found, as the API lists it.

| Field | Type | |
|---|---|---|
| `label` | string |  |
| `backend` | string | The backend, spelled as `--device` takes it. |
| `serial` | string or null |  |
| `verified` | boolean | Written with this radio on the bench, rather than reasoned from a library's headers (POLICY rule 1). |
| `open` | boolean | The one this engine runs. |

### Snapshot

Everything the engine reads, at one moment.

| Field | Type | |
|---|---|---|
| `at_unix_ms` | integer | When the snapshot was taken, milliseconds since the Unix epoch. |
| `monas` | string | The engine's version. |
| `mode` | string | What the engine computes: the name of the view whose work it does. |
| `radio` | group of [`view::Radio`](#viewradio) |  |
| `signal` | group of [`Signal`](#signal) |  |
| `iq` | group of [`Iq`](#iq) |  |
| `vitals` | group of [`Vitals`](#vitals) |  |
| `record` | group of [`Recording`](#recording) or absent |  |
| `pager` | group of [`Pager`](#pager) |  |
| `ft8` | group of [`Ft8`](#ft8) |  |
| `demod` | group of [`Demod`](#demod) |  |
| `net` | group of [`Net`](#net) |  |

### spectrum::Frame

One spectrum frame.

| Field | Type | |
|---|---|---|
| `center_hz` | integer |  |
| `start_hz` | number | The frequency of the axis's start, Hz. |
| `sample_rate_hz` | number |  |
| `enbw_hz` | number | The noise-equivalent bandwidth of one bin: the resolution bandwidth every bin's power was measured in. |
| `axis` | string | `fft_bins` (bin `i` covers `[start + i*w, start + (i+1)*w)`) or `measured_points` (point `i` is the power at `start + i*w`). |
| `noise_floor_dbfs` | number |  |
| `bins_dbfs` | list of number | Power per bin, dBFS, from the axis's start. |

### Pages

The pages, for their own route.

| Field | Type | |
|---|---|---|
| `masked` | boolean |  |
| `mine` | integer or null |  |
| `pages` | list of [`PageView`](#pageview) | Oldest first, each with its number; `since` leaves out the ones numbered up to it. |

### Decodes

The decodes, for their own route.

| Field | Type | |
|---|---|---|
| `decodes` | list of [`Decode`](#decode) | Oldest first; `since` leaves out the ones numbered up to it. |
| `note` | string or null | What the list cannot hold or no longer holds, when that is anything. |

### Band

The band's occupancy, one megahertz to a cell.

| Field | Type | |
|---|---|---|
| `noise_dbfs` | number or null | The noise floor every cell was measured against, dBFS. |
| `window_s` | number | The length of one measuring window, seconds. |
| `cells` | list of [`Cell`](#cell) |  |

### Census

The census, for its own route.

| Field | Type | |
|---|---|---|
| `devices` | list of [`Device`](#device) |  |

### Packets

A packet list, for its route.

| Field | Type | |
|---|---|---|
| `note` | string or null | Why the list is not every packet, when it is not: held, filtered, narrowed to a kind; for LE Coded, which tests its columns are. |
| `packets` | list of [`Packet`](#packet) | Newest first, as on screen; `since` leaves out those numbered up to it. |

### Hits

The hits, for their route.

| Field | Type | |
|---|---|---|
| `note` | string | Where the header fields come from, and whether the session heard more than it keeps. |
| `hits` | list of [`Hit`](#hit) | Oldest first. |

### Fer

The curves, for their route.

| Field | Type | |
|---|---|---|
| `note` | string | What the counts are, and why a device's rate is a ceiling. |
| `bins` | list of [`Bin`](#bin) |  |

### Survey

The survey, for its route.

| Field | Type | |
|---|---|---|
| `refused` | string or null | Why Wi-Fi's receiver does not run, when it does not. |
| `channels` | list of [`Channel`](#channel) |  |
| `least_busy` | [`Verdict`](#verdict) |  |

### Frames

The frames, for their route.

| Field | Type | |
|---|---|---|
| `channel` | integer or null |  |
| `refused` | string or null |  |
| `heard_session` | integer | Frames heard this session, including those the list let go. |
| `frames` | list of [`wifi::Frame`](#wififrame) | Newest first; `since` leaves out those numbered up to it. |

### Airtime

The airtime views, for their route.

| Field | Type | |
|---|---|---|
| `channel` | integer or null | The channel the radio is locked on, when it is. |
| `share` | [`Share`](#share) or absent |  |
| `ledger` | list of [`Spent`](#spent) or absent |  |
| `rhythm` | [`RhythmView`](#rhythmview) or absent |  |

### LogLevel

Severity of a log line, used by the log panel to pick a status lamp + colour. Derived from the message text (see `LogLevel::infer`) so the ~86 existing `push_log` call sites keep working unchanged.

One of: `"info"`, `"ok"`, `"warn"`, `"error"`.

### LostSamples

Samples lost this session: by the driver before the engine saw them, or by the engine's own intake (named `sdrtop` since the first release).

| Field | Type | |
|---|---|---|
| `all` | integer |  |
| `driver` | integer |  |
| `sdrtop` | integer |  |

### Feeds

The sample feeds, each one's refusals.

| Field | Type | |
|---|---|---|
| `spectrum` | [`server::Feed`](#serverfeed) |  |
| `net` | [`server::Feed`](#serverfeed) |  |
| `pager` | [`server::Feed`](#serverfeed) |  |
| `ft8` | [`server::Feed`](#serverfeed) |  |

### Process

What the process costs the machine.

| Field | Type | |
|---|---|---|
| `cpu_pct` | number |  |
| `rss_mb` | integer |  |

### view::Radio

The radio: which, where it is tuned, the gain chain, the boost, and the frequency reference its offsets are corrected by.

| Field | Type | |
|---|---|---|
| `name` | string |  |
| `serial` | string |  |
| `streaming` | boolean |  |
| `frequency_hz` | integer |  |
| `sample_rate_hz` | number |  |
| `stages` | list of [`Stage`](#stage) | Front to back, by the radio's own names. |
| `chain_db` | number or absent | The whole chain in dB, where every stage reads in dB here. |
| `boost` | [`Boost`](#boost) or absent |  |
| `reference` | [`Reference`](#reference) or absent |  |

### Signal

The signal's figures as the Lab benches print them: the frame taken at the last quarter-second tick, and channel power as the S-meter reads it.

| Field | Type | |
|---|---|---|
| `peak_to_nf_db` | number or absent |  |
| `channel_power_dbfs` | number or absent | Approximate: the S-meter's smoothed readout. |
| `occupied_bw_hz` | integer or absent | The carrier's, so a gap between bursts does not occupy the span. |
| `noise_floor_dbfs` | number or absent |  |

### Iq

The I/Q group's readings.

| Field | Type | |
|---|---|---|
| `dc_i` | number or absent |  |
| `dc_q` | number or absent |  |
| `dc_spike_dbfs` | number or absent | How tall the spike the DC makes in the spectrum is. |
| `dc_over_signal_db` | number or absent | How far the DC stands over (positive) or under the signal's rms. |
| `gain_imbalance_db` | number or absent |  |
| `phase_imbalance_deg` | number or absent |  |
| `image_rejection` | [`Irr`](#irr) or absent |  |
| `lo_offset_hz` | integer | Where the LO sits, Hz from the centre: zero on a radio that mixes straight to 0 Hz, where the DC above is the LO's leak. |
| `lo_level_dbfs` | number or absent | The spectrum's level at an LO off the centre. |
| `corrected_by` | string or null | Who corrected DC and balance before the engine saw the samples, when someone did: the readings are what is left. |
| `dc_block_on` | boolean |  |
| `correction_applied` | boolean |  |

### Vitals

The vitals group's readings.

| Field | Type | |
|---|---|---|
| `drops_per_s` | integer or absent |  |
| `lost_session` | [`Lost`](#lost) or absent | Samples lost this session, and whose loss each was. |
| `radio_drops` | [`RadioDrops`](#radiodrops) or absent | What the radio dropped in itself before anything reached the host, where it keeps such a count: no hole in the stream shows these. |
| `adc_saturation_pct` | number or absent |  |
| `adc_saturation_peak_pct` | number or absent |  |
| `cpu_pct` | number |  |
| `rss_mb` | integer |  |
| `usb_errors_session` | integer |  |
| `throughput` | [`Throughput`](#throughput) or absent |  |
| `fft_queue_peak_pct` | number or absent | How full the spectrum feed's queue got, at its fullest lately. |
| `fft_blocks_dropped` | [`Dropped`](#dropped) or absent | Blocks the spectrum feed had no room for: frames never drawn. |

### Recording

One recording's account.

| Field | Type | |
|---|---|---|
| `file` | string | The `.sigmf-data` file's name; its `.sigmf-meta` sits beside it. |
| `running` | boolean |  |
| `rate_hz` | number |  |
| `pairs` | integer |  |
| `bytes` | integer |  |
| `lost` | integer | Pairs missing from the file, of every cause. |
| `short` | number or null | The fraction of its rate the radio did not deliver without saying so, once that is past the recording's tolerance. |
| `radio_drops` | integer or null | Drops the radio counted in itself while recording, where it keeps such a count. |
| `ended` | string or null | Why it ended; none while it runs. |

### Pager

The pager's summary, for the snapshot.

| Field | Type | |
|---|---|---|
| `received_session` | integer | Pages received this session, including any the inbox let go. |
| `held` | integer | Pages held, and of those the ones shown. |
| `shown` | integer |  |
| `locked_baud` | integer or null | The rate a receiver is locked at now: a transmission is being read. |
| `masked` | boolean |  |
| `mine` | integer or null | The address the pager answers to; none for every address. |
| `feed` | [`pager::Feed`](#pagerfeed) |  |

### Ft8

The decoder's summary, for the snapshot.

| Field | Type | |
|---|---|---|
| `decoding` | boolean | Whether a cycle is being decoded now. |
| `decoded_session` | integer |  |
| `held` | integer | Decodes held, at most the newest few hundred. |
| `clock` | [`ClockVerdict`](#clockverdict) or absent | The clock as the band hears it: the decodes' median time offset. |
| `last_cycle` | [`Cycle`](#cycle) or absent |  |
| `feed` | [`pager::Feed`](#pagerfeed) |  |

### Demod

The demodulator's group's readings.

| Field | Type | |
|---|---|---|
| `running` | boolean | Whether blocks are being given to the demodulator at all. |
| `offset_hz` | integer | The channel's centre, Hz from the tuned frequency. |
| `channel_rate_hz` | number or absent | The rate the channel lands on after decimation. |
| `forced` | string or null | The demodulator forced in place of the classifier's choice, if one is. |
| `fm` | [`Fm`](#fm) or absent |  |
| `am` | [`Am`](#am) or absent |  |
| `pilot` | [`Pilot`](#pilot) or absent |  |
| `ctcss` | [`Ctcss`](#ctcss) or absent |  |
| `rds` | [`Rds`](#rds) or absent |  |
| `blocks_dropped` | integer | Audio blocks missing from the channel's run this session. |

### Net

The receivers' summary, for the snapshot.

| Field | Type | |
|---|---|---|
| `mode` | string | `survey`, `lock` and the like, as the panels' tag says it. |
| `survey_refused` | string or null | Why no survey runs where the mode says one should. |
| `addresses` | string | How addresses are shown: in full, by registrant, or masked. |
| `devices` | integer |  |
| `ble_packets_heard` | integer |  |
| `coded_packets_heard` | integer |  |
| `feed` | [`NetFeed`](#netfeed) |  |

### PageView

One page and the transmission it came in.

| Field | Type | |
|---|---|---|
| `n` | integer | Its number this session, rising. |
| `unix_s` | number |  |
| `centre_hz` | integer |  |
| `ric` | integer | The 21-bit address. |
| `function` | string | `A` to `D`. |
| `kind` | string | `tone`, `numeric` or `alpha`. |
| `length` | integer or null | Digits or characters; none for a tone. |
| `text` | string or absent | The text as the screen shows it, or why it is not shown. |
| `decided` | string | How the kind was decided: `by_function`, `over_function`, `by_content`, `ambiguous`, or `no_message` (an address alone). |
| `end` | string | How the page ended: `address`, `idle`, `undecipherable` or `cut`. |
| `bits_repaired` | integer |  |
| `codewords_refused` | integer |  |
| `link` | [`Link`](#link) |  |

### Decode

One decode: what it said, and what it measured.

| Field | Type | |
|---|---|---|
| `n` | integer | Its number this session, rising. |
| `cycle_unix` | number |  |
| `slot` | string | `even` or `odd`. |
| `band` | string or null |  |
| `dial_hz` | integer |  |
| `tone_hz` | number |  |
| `tone_sigma_hz` | number |  |
| `snr_db` | number |  |
| `dt_s` | number |  |
| `drift_hz` | number |  |
| `coherence` | number |  |
| `sync_of_21` | integer | Costas tones on their tone, of 21. |
| `ldpc_iterations` | integer |  |
| `bits_repaired` | integer |  |
| `osd_order` | integer or null |  |
| `block` | integer |  |
| `pass` | integer |  |
| `message` | string |  |
| `from` | string or null |  |
| `to` | list of string |  |
| `for_me` | boolean |  |
| `locator` | string or null | The locator this message carried. |
| `station_locator` | string or null | The last locator the sender sent itself, which the distance is from. |
| `lat` | number or null |  |
| `lon` | number or null |  |
| `km` | number or null |  |
| `bearing_deg` | number or null |  |
| `clock` | [`ClockVerdict`](#clockverdict) or null | The clock verdict the cycle was read with. |
| `margin_min_db` | number | The least margin a data symbol was decided by. |
| `held` | boolean or null | Whether the radio held its level in the cycle. |
| `floor_db_hz` | number or null |  |
| `unread` | string or null | `start` or `end`: the part of the transmission read as unknown. |

### Cell

One megahertz of the band.

| Field | Type | |
|---|---|---|
| `mhz` | number |  |
| `observed` | boolean | Whether anyone looked: an unobserved cell has no figures at all, which is not the same as a quiet one. |
| `duty_pct` | number or null | The share of windows something was on, where its uncertainty is within the resolution it is shown at. |
| `duty_sigma_pct` | number or null | The duty cycle's uncertainty, stated even when the value is not. |
| `coverage_pct` | number or null | The share of the cell's time it was watched. |
| `mean_dbfs` | number or null |  |
| `peak_dbfs` | number or null |  |
| `windows` | integer |  |

### Device

One transmitter of the census, every column the panel shows.

| Field | Type | |
|---|---|---|
| `address` | string | As the addresses are shown: in full, by registrant, or masked. |
| `kind` | string |  |
| `name` | string or null | What it advertised about itself, where it has. |
| `name_complete` | boolean or null |  |
| `tx_power_dbm` | integer or null |  |
| `company_id` | integer or null |  |
| `company` | string or null |  |
| `packets` | integer |  |
| `crc_failed` | integer |  |
| `crc_pass_pct` | number |  |
| `best_snr_db` | number or null |  |
| `mean_snr_db` | [`Sigma`](#sigma) or null |  |
| `crystal_offset_ppm` | [`Sigma`](#sigma) or null | Corrected for our oscillator as the panel shows it; the provenance's reference says what that is worth. |
| `modulation_index` | [`Sigma`](#sigma) or null |  |
| `pdu_types` | list of string |  |
| `advertising` | [`Advertising`](#advertising) |  |
| `first_seen_s` | integer |  |
| `last_seen_s` | integer |  |

### Packet

One packet: what it said, and what was measured of it.

| Field | Type | |
|---|---|---|
| `seq` | integer |  |
| `age_s` | integer |  |
| `channel` | integer |  |
| `phy` | string |  |
| `pdu_type` | string | The PDU's type, or an extended PDU's role. |
| `crc_ok` | boolean |  |
| `length` | integer |  |
| `address` | string or null | As the address mode shows it. |
| `address_kind` | string or null |  |
| `tx_add` | string | `public` or `random`. |
| `rx_add` | string or null | Only on a type with a target address. |
| `ch_sel` | boolean or null | Only on a type that defines it. |
| `advertised` | [`Advertised`](#advertised) |  |
| `snr_db` | number or null |  |
| `cfo_khz` | [`Sigma`](#sigma) or null | The transmitter's offset, corrected for our oscillator as on screen. |
| `cfo_ppm` | [`Sigma`](#sigma) or null |  |
| `start_khz` | [`Sigma`](#sigma) or null |  |
| `end_khz` | [`Sigma`](#sigma) or null |  |
| `mod_index` | [`Sigma`](#sigma) or null |  |
| `df1_avg_khz` | [`Sigma`](#sigma) or null |  |
| `df2_avg_khz` | [`Sigma`](#sigma) or null |  |
| `df2_df1_ratio` | [`Sigma`](#sigma) or null |  |
| `drift_khz` | [`Sigma`](#sigma) or null |  |
| `drift_rate_hz_per_us` | [`Sigma`](#sigma) or null |  |
| `fec_repairs` | integer or null | LE Coded: bits the FEC repaired. |
| `sid` | integer or null |  |
| `aux` | string or null | What became of an AuxPtr: `pending`, `heard`, `missed`, ... |

### Hit

One classic Bluetooth hit.

| Field | Type | |
|---|---|---|
| `age_s` | integer |  |
| `stream_us` | number | The hit's time on the stream's sample clock, microseconds: comparable only within one run of the stream. |
| `channel` | integer |  |
| `lap` | string | As the address mode allows; an inquiry code in hex, being no one's. |
| `lap_kind` | string | `piconet`, `paged`, or the inquiry code's name (`GIAC`, ...). |
| `uap` | string or null | Once it is one value. |
| `uap_candidates` | integer or null |  |
| `slot_residual_us` | number or null |  |
| `clock_ppm` | [`Sigma`](#sigma) or null | The piconet's slot clock against ours, from the same fit, corrected as the reference allows. |
| `header` | string | `decoded`, `none`, `not read: UAP unresolved` or `did not decode`. |
| `lt_addr` | integer or null |  |
| `packet_type` | string or null |  |
| `flow` | integer or null |  |
| `arqn` | integer or null |  |
| `seqn` | integer or null |  |

### Bin

One SNR bin of a frame error curve.

| Field | Type | |
|---|---|---|
| `scope` | string | `all`, or a census device's address as the address mode shows it. |
| `snr_from_db` | number or null |  |
| `snr_to_db` | number or null |  |
| `packets` | integer |  |
| `crc_failed` | integer |  |
| `fer_pct` | [`Sigma`](#sigma) or null | Only where the panel draws a rate. |

### Channel

One channel of the survey.

| Field | Type | |
|---|---|---|
| `channel` | integer |  |
| `busy` | [`Busy`](#busy) or absent |  |
| `airtime` | list of [`FamilyShare`](#familyshare) | The share of its time its frames stated, by family, over its visits. |

### Verdict

Which channel is least busy, or why none can be named yet.

| Field | Type | |
|---|---|---|
| `status` | string | `first_pass`, `no_spread_yet`, `alone`, `few` or `not_apart`. |
| `channels` | list of integer | The quietest channel and those within 2 sigma of it. |
| `visited` | integer or null | Channels visited so far, on the first pass. |
| `passes` | integer or null | Passes more that would settle it, where that can be said. |

### wifi::Frame

One frame.

| Field | Type | |
|---|---|---|
| `seq` | integer | Its place in the session's arrivals, from 1. |
| `epoch` | integer | The stream's clock it was heard on, and the run of it: starts compare only within one run of one clock. |
| `run` | integer |  |
| `start` | integer | The stream pair its first sample arrived at. |
| `age_s` | integer |  |
| `centre_hz` | number | The centre of the channel it was sent on: ours, or another's. |
| `family` | string or null | Its family, for a frame of this channel; another channel's is none. |
| `said` | [`Said`](#said) |  |
| `airtime_us` | integer or null | The airtime it stated, where it was read. |
| `gap_us` | number or null | From the end of the frame before it, where that is known. |
| `offset_khz` | [`Sigma`](#sigma) or null | Its carrier against its channel's centre, corrected for our oscillator as on screen. |
| `snr_db` | number or null |  |

### Share

The locked channel's share of the time over the last whole seconds.

| Field | Type | |
|---|---|---|
| `frames` | [`AirReading`](#airreading) | The frames' share of the time, mean and standard error. |
| `busy` | [`AirReading`](#airreading) | The energy's busy share. |
| `airtime` | list of [`FamilyShare`](#familyshare) |  |
| `columns` | integer |  |
| `heard_s` | number |  |
| `span_s` | number |  |

### Spent

Who spent the channel's airtime, by sender and rate.

| Field | Type | |
|---|---|---|
| `who` | string | The spender and its rate, as the ledger names them. |
| `frames` | integer |  |
| `stated` | integer | Frames that stated their length. |
| `air_us` | number |  |
| `share` | number | The share of the heard time. |
| `per_s` | number |  |
| `mean_us` | number or null |  |

### RhythmView

The gaps between frames, binned, as the rhythm strip draws them.

| Field | Type | |
|---|---|---|
| `bins` | list of integer |  |
| `under` | integer |  |
| `past` | integer |  |
| `corrected` | integer |  |
| `unplaced` | integer |  |

### server::Feed

One feed's refusals, lately and over the session.

| Field | Type | |
|---|---|---|
| `refused` | integer |  |
| `refused_session` | integer |  |

### Stage

One gain stage: what it is set to, and what that gives in dB.

| Field | Type | |
|---|---|---|
| `name` | string |  |
| `setting` | number | The stage's setting as stored: dB, or the radio's own step. |
| `db` | number or absent |  |

### Boost

The front end's boost (an amplifier, or the tuner's AGC) and whether it is on.

| Field | Type | |
|---|---|---|
| `name` | string |  |
| `on` | boolean |  |

### Reference

Our oscillator's error, what it was measured against, and how long ago.

| Field | Type | |
|---|---|---|
| `provenance` | string | `relative`, `referenced` or `traceable`. |
| `ppm` | number |  |
| `sigma_ppm` | number |  |
| `source` | string |  |
| `age_s` | integer |  |

### Irr

The image rejection the imbalance allows, capped where the formula runs away: as the imbalance approaches zero it gives hundreds of dB, and no front end measures that.

| Field | Type | |
|---|---|---|
| `db` | number |  |
| `at_least` | boolean | `db` is a floor: the formula said more, and more is arithmetic, not a radio. |

### Lost

Samples lost: by the driver before the engine saw them, or by the engine's own intake.

| Field | Type | |
|---|---|---|
| `all` | integer |  |
| `driver` | integer |  |
| `monas` | integer |  |

### RadioDrops

The radio's own count of what it dropped since the stream started.

| Field | Type | |
|---|---|---|
| `stream` | integer |  |
| `longest_ms` | number | The longest of them, in milliseconds of samples. |

### Throughput

The link's traffic: on the bus, where the radio's wire format is known, else what was delivered; and the ceiling the radio's top rate would need.

| Field | Type | |
|---|---|---|
| `what` | string | `bus` or `delivered`. |
| `mb_s` | number |  |
| `ceiling_mb_s` | number |  |
| `utilisation` | number | `mb_s` over the ceiling, from 0 to 1. |

### Dropped

A count lately and over the session.

| Field | Type | |
|---|---|---|
| `recent` | integer |  |
| `session` | integer |  |

### pager::Feed

A sample feed's account: what it took, what it lost, and how hard its worker works.

| Field | Type | |
|---|---|---|
| `blocks_in` | integer |  |
| `gaps` | integer | Breaks in the stream the worker saw. |
| `blocks_lost` | integer | Blocks lost inside those breaks. |
| `refused` | integer | Blocks the feed had no room for, lately and over the session. |
| `refused_session` | integer |  |
| `load` | number or null | The worker's time per block over the stream time it covered: above 1 it falls behind. |

### ClockVerdict

The clock as heard: the median of the decodes' DT, their spread, and how many it rests on.

| Field | Type | |
|---|---|---|
| `median_dt_s` | number |  |
| `spread_s` | number |  |
| `decodes` | integer |  |

### Cycle

One 15 s cycle and what became of it.

| Field | Type | |
|---|---|---|
| `start_unix` | number |  |
| `dial_hz` | integer |  |
| `outcome` | string | `decoded`, `cut` (the stream broke or was retuned inside it) or `skipped` (the decoder was still busy with the one before). |
| `decodes` | integer |  |
| `candidates` | integer |  |
| `malformed` | integer |  |
| `seconds` | number | How long the decode took. |
| `floor_db_hz` | number or null |  |

### Fm

| Field | Type | |
|---|---|---|
| `peak_dev_hz` | number |  |
| `rms_dev_hz` | number |  |
| `carrier_offset_hz` | number |  |

### Am

| Field | Type | |
|---|---|---|
| `depth_pct` | number |  |
| `positive_pct` | number |  |
| `negative_pct` | number |  |
| `carrier_dbfs` | number |  |

### Pilot

| Field | Type | |
|---|---|---|
| `state` | string | `absent`, `marginal` (detectable, below a trustworthy injection) or `locked`. |
| `deviation_hz` | number |  |
| `injection_pct` | number |  |

### Ctcss

| Field | Type | |
|---|---|---|
| `tone_hz` | number |  |
| `deviation_hz` | number |  |
| `margin_db` | number |  |

### Rds

| Field | Type | |
|---|---|---|
| `pi` | integer or null |  |
| `pty` | integer or null |  |
| `ps` | string or null | The station's name, as broadcast. |
| `rt` | string or null | RadioText, as broadcast. |
| `groups_ok` | integer |  |
| `groups_session` | integer |  |
| `sync` | boolean | Whether the block synchroniser is tracking the groups now. |
| `age_s` | number | Seconds since the last whole group. |
| `aged` | boolean | Past the few seconds after which the name is said with its age. |

### NetFeed

The NET feed's account.

| Field | Type | |
|---|---|---|
| `blocks_in` | integer |  |
| `pairs_in` | integer |  |
| `gaps` | integer | Breaks in the stream the worker saw, and blocks lost inside them. |
| `blocks_lost` | integer |  |
| `refused` | integer | Blocks the feed had no room for, lately and over the session. |
| `refused_session` | integer |  |
| `decode_load` | number or null | The decoders' time per block over the stream time it covered. |
| `last_loss_s` | number or null | Seconds since the feed last lost anything, if it ever has. |

### Link

The transmission's physics.

| Field | Type | |
|---|---|---|
| `baud` | integer |  |
| `measured_baud` | number | The rate it ran at, by this radio's clock. |
| `clock_ppm` | number |  |
| `shift_hz` | number |  |
| `offset_hz` | number |  |
| `decision_snr_db` | number or null |  |
| `level_dbfs` | number |  |
| `preamble_bits` | integer |  |
| `sync_errors` | integer |  |
| `batches` | integer |  |

### Sigma

A value and its standard uncertainty.

| Field | Type | |
|---|---|---|
| `value` | number |  |
| `sigma` | number |  |

### Advertising

The advertising timing as read in LOCK, or why there is none.

| Field | Type | |
|---|---|---|
| `status` | string | `estimated`, or why not: `not timed in LOCK`, `collecting 3 of 8`, ... |
| `interval_ms` | [`Sigma`](#sigma) or null |  |
| `events` | integer or null |  |
| `channel` | integer or null |  |
| `delay` | string or null | `none`, `uniform` or `not uniform`. |
| `delay_width_ms` | number or null |  |
| `grid` | string or null | `on`, `off` or `cannot tell`. |
| `grid_steps` | string or null |  |
| `grid_off_ms` | number or null |  |

### Advertised

What a packet advertised about its sender: the AD structures, flattened.

| Field | Type | |
|---|---|---|
| `name` | string or null | As the address mode allows it, as on screen. |
| `name_complete` | boolean or null |  |
| `flags` | list of string |  |
| `tx_power_dbm` | integer or null |  |
| `services` | list of string |  |
| `service_data` | list of string | `uuid=bytes`, the bytes as the address mode allows them. |
| `company_id` | integer or null |  |
| `company` | string or null |  |
| `mfr_data` | list of string |  |
| `other_ad` | list of string | `0xCODE=bytes` for structures with no column of their own. |
| `malformed` | string or null | Where the structures stopped making sense, and why. |

### Busy

A channel's busy share over its visits.

| Field | Type | |
|---|---|---|
| `mean` | number | The mean share, from 0 to 1. |
| `sigma` | number or null | The standard error of the mean, once there are enough visits. |
| `visits` | integer |  |
| `seconds` | number |  |

### FamilyShare

A share of the time, by family.

| Field | Type | |
|---|---|---|
| `family` | string |  |
| `share` | number | From 0 to 1. |

### Said

What a frame's signalling said, worded as the Frames list words it.

| Field | Type | |
|---|---|---|
| `format` | string | Its format (`HT`, `HE SU`, `11b long`), or for another channel's frame, how far off it is and what kind (`+5 MHz · OFDM`). |
| `rate` | string or null | `54 Mb/s`, `MCS 7`, where it was read. |
| `length_bytes` | integer or null | Its length in octets, where it was read. |
| `network` | string or null | HE and later: the BSS colour and the direction. |

### AirReading

| Field | Type | |
|---|---|---|
| `mean` | number |  |
| `sigma` | number or null |  |

---

← [monasd](monasd.md)
