# Configuration

← [Back](README.md)

---

## Where the config lives

sdrtop saves your settings automatically when you quit (`q`). The file is at:

```
~/.config/sdrtop/config.toml
```

You can open and edit it by hand — it's plain text. Changes take effect next time you start sdrtop.

---

## What's in the config

```toml
[radio]
frequency_hz = 92800000   # center frequency in Hz
sample_rate  = 2000000.0  # samples/sec — HackRF 2–20M · RTL-SDR 0.9–3.2M
lna_gain     = 24         # HackRF LNA (0–40 dB, step 8) / RTL-SDR tuner gain
vga_gain     = 30         # VGA gain (0–62 dB, step 2) — HackRF only
amp_enabled  = false      # HackRF RF amplifier / RTL-SDR tuner AGC

[display]
active_preset      = "main"   # which layout to use at startup
waterfall_max_rows = 64       # how many rows of history the waterfall keeps

[theme]
base = "nord"   # which color theme to use
```

> The config is **device-agnostic** — the same file works for a HackRF or an RTL-SDR. Any value outside the active device's range is clamped into it at startup rather than rejected, so a config saved on a HackRF (e.g. 2.4 GHz / 10 Msps) still boots an RTL-SDR at a legal frequency and rate instead of failing. With both radios connected, pick one with `--device hackrf|rtlsdr`. RTL-SDR support is **new** — see [Supported Hardware](hardware.md).

---

## Runtime input: frequency and sample rate

While sdrtop is running, you can change settings with `f` (frequency) and `s` (sample rate). See [Advanced Features](advanced.md#custom-input-modes-frequency-and-sample-rate) for input formats and examples.

---

You can save named frequency markers. They appear as vertical lines on the spectrum with a label.

```toml
[[display.spectrum_markers]]
freq_hz = 92800000
label   = "FM Radio"

[[display.spectrum_markers]]
freq_hz = 156800000
label   = "Marine ch16"
```

You can add as many as you like. You can also place them from within sdrtop using the `m` key in spectrum focus mode.

---

## Sweep scanner

The `lab_sweep` preset (`9`) and the `micro_sweep` field view scan a band wider
than one sample-rate window by retuning across it. The band and dwell time are
set in the config:

```toml
[sweep]
start_hz = 400000000   # scan from 400 MHz
stop_hz  = 500000000   # scan to 500 MHz
dwell_ms = 200         # measure each position for 200 ms (50–2000)
```

The step between positions is derived from the sample rate automatically (about
90 % of it, for a small overlap). You don't have to edit the config to change the
band — while in the sweep panel's focus mode (`g`), `s` and `e` prompt for the
start and end frequency in MHz, `+` / `-` nudge the dwell live, `←` / `→` move
the cursor, `M` toggles peak/mean, and `Enter` jumps the radio to the cursor
frequency. Your last band and dwell are saved on quit.

A sweep cycle takes a couple of seconds, so it's for *finding* signals, not
real-time monitoring — once you spot one, `Enter` tunes to it.

---

## Custom layout presets

A *preset* is a named arrangement of panels. sdrtop ships with built-in presets you switch between with the number keys, but you can also define your own in the config file. Your presets are merged with the built-in ones at startup, and they survive a save — sdrtop never erases hand-written presets.

**Every preset is overridable** — including the built-ins. If you define a preset with the same name as a built-in (`main`, `spectrum`, `lab_iq`, `lab_rf`, `lab_timing`, `lab_signal`, `micro_main`, …), your version replaces it, and the number key that triggers it now opens your layout. New names you invent join the `[P]` cycle automatically.

A preset is a list of panels. Each panel has a `name`, a `position`, and optionally a size:

```toml
[presets.my_view]
panels = [
  { name = "header",   position = "top",    height = 5     },
  { name = "spectrum", position = "body"                    },
  { name = "log",      position = "right",  width_pct = 30  },
  { name = "footer",   position = "bottom"                  },
]
```

**Positions:**

| Position | Where it goes | Size field |
|----------|---------------|------------|
| `top`    | Full-width strip at the top    | `height` (rows) |
| `bottom` | Full-width strip at the bottom | `height` (rows) |
| `left`   | Left column of the body        | `width_pct` (% of body) |
| `right`  | Right column of the body       | `width_pct` (% of body) |
| `body`   | Centre column (fills remaining space) | — |

**Available panel names:** `header`, `header_slim`, `command_rail`, `lab_banner`, `lab_marker`, `spectrum`, `waterfall`, `log`, `footer`, `signal_strip`, `telemetry`, `gains`, `throughput`, `sample_rate`, `usb_sr`, `rf_chain`, `level_diagram`, `adc_loading`, `iq_diagnostics`, `iq_constellation`, `iq_histogram`, `image_scope`, `hardware_health`, `signal_metrics`, `signal_characterization`, `fm_demod`, `system_resources`, `timing_panel`, `timing_diagnostics`, `timing_stripchart`, `timing_vitals`, `sweep_panel`, `sweep_strip`, `observer`, `micro_panel`, `micro_signal_panel`, `micro_gain_panel`, `micro_health_panel`, `micro_sweep_panel`.

**Panels that do work, not just drawing.** Most panels only render. Two of them
drive a background job, and that job follows the panel rather than the preset's
name, so both work in a preset of your own:

- `fm_demod` runs the demodulator. Listing it anywhere gets you a working demod
  bench; leaving it out costs nothing on every other layout.
- A centre column that is exactly `spectrum` followed by `waterfall` bonds the two
  into one instrument, with a single shared frequency ruler between them instead of
  two facing borders.

The `lab_` name prefix does still mean something: a preset whose name starts with
it renders in "instrument mode", with the frame colours cooled toward steel blue.
That is the one place a preset's name changes how it behaves.

See [Advanced Features](advanced.md#defining-custom-presets) for the full guide to creating and managing custom presets.

Quick example:

```toml
[presets.my_view]
panels = [
  { name = "header",   position = "top",    height = 2  },
  { name = "spectrum", position = "body"                 },
  { name = "log",      position = "right",  width_pct = 20 },
  { name = "footer",   position = "bottom", height = 1  },
]
```

To make it accessible via a key, name it `lab_timing`, `micro_signal`, etc. (reserved names in [Advanced Features](advanced.md#preset-system-and-layout-configuration))