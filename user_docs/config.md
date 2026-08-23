# Configuration

← [Back](README.md)

---

## Where the config lives

sdrtop saves your settings automatically when you quit with `q`. The file is at:

```
~/.config/sdrtop/config.toml
```

It's plain text, and editing it by hand is safe. Changes take effect next launch.
Writes are atomic (temp file, then rename), so an interrupted save can't leave you
with half a config.

> **One thing to know before you edit.** If the file doesn't parse, sdrtop doesn't
> guess which line you meant. It falls back to **all defaults** for the whole file
> and carries on, so a single stray character can look like "sdrtop forgot
> everything". It does say so, but the message goes to `~/.config/sdrtop/sdrtop.log`
> rather than the screen, because by then the TUI owns the terminal. If your
> settings vanish after a hand edit, that log is the first place to look.

The config is **device-agnostic**: the same file works for a HackRF or an
RTL-SDR. Any value outside the active device's range is clamped into it at
startup rather than rejected, so a config saved on a HackRF at 2.4 GHz and
10 Msps still boots an RTL-SDR at a legal frequency and rate instead of failing.
With both radios connected, pick one with `--device hackrf|rtlsdr`.

---

## The whole file

Everything sdrtop understands, in one place. Every field is optional; anything you
leave out gets its default, and any field sdrtop doesn't recognise is ignored
rather than treated as an error.

```toml
[radio]
frequency_hz = 92800000    # tuned center frequency, in Hz
sample_rate  = 2000000.0   # samples/sec. HackRF 2–20 M · RTL-SDR 0.9–3.2 M
lna_gain     = 24          # HackRF LNA (0–40 dB, step 8) / RTL-SDR tuner gain
vga_gain     = 30          # HackRF VGA (0–62 dB, step 2). Ignored on RTL-SDR
amp_enabled  = false       # HackRF RF amplifier / RTL-SDR tuner AGC
recall_hz    = [0, 0, 0]   # Command Rail recall slots. 0 means empty

[display]
active_preset      = "command_rail"  # which layout to open on
waterfall_max_rows = 64              # rows of history the waterfall keeps
waterfall_palette  = "classic"       # classic · amber · ice · phosphor
spectrum_style     = "braille"       # braille · fill · scatter

[theme]
base = "nord"                        # see themes.md for the six palettes

[sweep]
start_hz = 400000000       # scanner band start
stop_hz  = 500000000       # scanner band end
dwell_ms = 200             # measure time per step (50–2000)
```

Markers and your own presets get their own blocks, described below.

**`[radio]`** is the tuning state, and it's what `q` writes back. `recall_hz` holds
the Command Rail's three recall slots, which you set with `M` in rail focus, so
tuning memory lives with the radio rather than with the display.

**`[display]`** is what the screen looks like. `waterfall_palette` and
`spectrum_style` are the two you're most likely to want to set by hand, because
they're otherwise only reachable through `p` in waterfall focus and `d` in
spectrum focus, and both persist once you've picked one.

**`[theme]`** takes a base palette plus optional per-field overrides. See
[Themes](themes.md).

**`[sweep]`** configures the band scanner, described below.

---

## Runtime input: frequency and sample rate

While sdrtop is running, `f` prompts for a frequency and `s` for a sample rate,
both **in MHz**, both as a plain number. No units, no suffixes: `92.8`, `433.92`,
`1090`, `2400`.

A value outside your device's range is clamped into it rather than rejected, so
typing `9000` on an RTL-SDR gets you its ceiling instead of an error. Something
that isn't a number at all does get an error, in the log, naming the valid range.

---

## Spectrum markers

Named frequencies that appear as labelled vertical lines on the spectrum. Place
them with `m` in spectrum focus, or write them here:

```toml
[[display.spectrum_markers]]
freq_hz = 92800000
label   = "FM Radio"

[[display.spectrum_markers]]
freq_hz = 156800000
label   = "Marine ch16"
channel_bw_hz = 25000     # optional: the channel width to draw around it
```

Add as many as you like. `channel_bw_hz` is what `b` cycles in spectrum focus
(6.25, 12.5, 25, 50, 100, 200, 500 kHz, then none); leave it out and the marker is
just a line.

---

## Sweep scanner

The `lab_sweep` preset (`9`) and the `micro_sweep` field view scan a band wider
than one sample-rate window by retuning across it. The band and dwell time are set
in `[sweep]` above.

The step between positions is derived from the sample rate automatically (about
90 % of it, for a small overlap). You don't have to edit the config to change the
band: in the sweep panel's focus mode (`g`), `S` and `E` prompt for the start and
end frequency in MHz, `+` and `-` nudge the dwell live, `←` and `→` move the
cursor, `M` toggles peak against mean, and `Enter` jumps the radio to the cursor
frequency. Your last band and dwell are saved on quit.

A sweep cycle takes a couple of seconds, so it's for *finding* signals rather than
real-time monitoring. Once you spot one, `Enter` tunes to it.

---

## Custom layout presets

A *preset* is a named arrangement of panels. sdrtop ships with built-in presets on
the number keys, and you can define your own here. Yours are merged with the
built-ins at startup and round-tripped verbatim on save, so hand-written presets
survive quitting untouched.

**Every preset is overridable, including the built-ins.** Define one with the same
name as a built-in (`command_rail`, `spectrum`, `waterfall`,
`spectrum_waterfall`, `main`, `lab_iq`, `lab_rf`, `lab_timing`, `lab_signal`,
`lab_sweep`, `micro_main`, `micro_signal`, `micro_gain`, `micro_health`,
`micro_sweep`, `observer`) and your version replaces it, so the number key that
triggers it now opens your layout. Those names are the whole list; a name that
isn't on it is a new preset, which joins the `p` cycle automatically rather than
taking over a key.

A preset is a list of panels, each with a `name`, a `position`, and optionally a
size:

```toml
[presets.my_view]
panels = [
  { name = "header",   position = "top",    height = 5     },
  { name = "spectrum", position = "body"                    },
  { name = "log",      position = "right",  width_pct = 30  },
  { name = "footer",   position = "bottom"                  },
]
```

### Positions

| Position | Where it goes | Size field |
|----------|---------------|------------|
| `top`    | Full-width strip at the top    | `height` in rows |
| `bottom` | Full-width strip at the bottom | `height` in rows |
| `left`   | Left column of the body        | `width_pct`, percent of body |
| `right`  | Right column of the body       | `width_pct`, percent of body |
| `body`   | Centre column, fills what's left | none |

Position names are lowercase. A capitalised `"Top"` fails to parse, and per the
warning at the top of this page, a parse failure costs you the whole file.

**You can stack panels in the same position.** Several `top` panels stack downward
in the order you list them, several `bottom` panels likewise, and a `body` column
can hold more than one. That's how the lab presets fit a banner under the header
and a marker bar, a log and a footer along the bottom.

Panels with no `height` ask for their own preferred height, which is usually what
you want for `footer` and the thin bars.

Two arrangements have behaviour attached:

- A centre column that is exactly `spectrum` followed by `waterfall` **bonds** the
  two into one instrument sharing a single frequency ruler, instead of two panels
  facing each other across a pair of borders.
- Listing `fm_demod` anywhere runs the demodulator. The job follows the panel, not
  the preset name, so your own layout gets a working demod bench, and leaving it
  out costs nothing on every other layout.

And one thing is attached to the **name**: a preset called `lab_something` renders
in **instrument mode**, with the frame colours cooled toward steel blue, and it's
the only kind of preset that draws the reference level and reference trace
overlays. If you want those, name accordingly.

### Panel names

These are the valid values for `name`. What each one actually draws is in
[What you see on screen](screens.md).

**Structure:** `header` · `header_slim` · `footer` · `log`

**Spectrum and waterfall:** `spectrum` · `waterfall`

**Cockpit and lab chrome:** `command_rail` · `lab_banner` · `lab_marker`

**Signal:** `signal_strip` · `signal_metrics` · `signal_characterization` ·
`fm_demod`

**RF front end:** `rf_chain` · `level_diagram` · `adc_loading`

**IQ:** `iq_diagnostics` · `iq_constellation` · `iq_histogram` · `image_scope`

**Timing and health:** `timing_diagnostics` · `timing_stripchart` ·
`timing_vitals` · `timing_panel` · `hardware_health` · `system_resources`

**Small read-outs:** `telemetry` · `gains` · `throughput` · `sample_rate` ·
`usb_sr`

**Sweep:** `sweep_panel` · `sweep_strip`

**Micro field views:** `micro_panel` · `micro_signal_panel` · `micro_gain_panel` ·
`micro_health_panel` · `micro_sweep_panel`

**Observer:** `observer`

A name sdrtop doesn't recognise is skipped rather than fatal, so a typo costs you
that panel and not the layout.

---

## What survives a save

`q` rewrites the config from the running state, which means it overwrites
`[radio]`, `active_preset`, the markers, the sweep band and dwell, the waterfall
palette and the spectrum style with whatever they are at that moment. Your
`[presets.*]` blocks are preserved verbatim.

> **Per-field theme overrides do not survive a save.** Only `theme.base` is
> written back, so any `border_accent = "…"` style lines you added are gone from
> the file the first time you quit with `q`. They work perfectly while sdrtop is
> running, and they load correctly every launch; they just don't get written back
> out. If you use them, keep them in a config you don't quit-and-save over: point
> sdrtop at it with `--config`, or re-add the lines after a save. This is a bug
> rather than a design decision, so expect it to change.

If you want a hand-edited value to stick in general, quit with `Ctrl+C` rather
than `q`. And if you're driving sdrtop from a script or testing a layout,
`--config /tmp/whatever.toml` keeps the experiment away from your real settings
entirely.

---

For workflows and less-obvious behaviour, see
[Advanced Features](advanced.md). For every key, including the focus modes that
drive these settings live, see [Keyboard Shortcuts](keys.md).
