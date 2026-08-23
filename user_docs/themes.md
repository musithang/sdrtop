# Themes

← [Back](README.md)

sdrtop has six built-in color themes. Switch with `--theme <name>` on startup, or
set it in your config file.

---

## Available themes

| Name         | Description                                        |
| ------------ | -------------------------------------------------- |
| `sdr`        | The default: dark background, cyan and green accents |
| `nord`       | Cool blue-grey palette, easy on the eyes           |
| `dracula`    | Purple and pink on a dark background               |
| `gruvbox`    | Warm brown and yellow tones                        |
| `catppuccin` | Soft pastel colors on a dark background            |
| `solarized`  | Classic Solarized dark scheme                      |

The theme also sets the waterfall's default gradient. If you'd rather have a
gradient that ignores the theme, `p` in waterfall focus cycles four fixed ones
(classic, amber, ice, phosphor), and `classic` is the one that follows the theme.

Presets whose name starts with `lab_` render in **instrument mode**, which cools
the frame colours of whatever theme you're running toward steel blue. That's
deliberate, not a theme bug: a measurement bench should look like one.

---

## Switching theme

**At startup:**
```sh
sdrtop --theme gruvbox
```

**In your config file** (takes effect next launch):
```toml
[theme]
base = "gruvbox"
```

---

## Custom colors

You can override individual colors without touching the rest of the theme. Any
field you leave out keeps its default from the base theme. Values are `"#rrggbb"`
strings.

```toml
[theme]
base = "nord"
border_accent = "#88c0d0"
value_hi      = "#ebcb8b"
```

The full set of fields:

| Field | What it colors |
|-------|----------------|
| `border_dim` | Background panels: log, system resources, gains |
| `border_default` | Most measurement panels |
| `border_accent` | The primary visual panels: spectrum and waterfall |
| `border_focused` | Whichever panel is currently in focus mode |
| `label` | Dim field labels, like "Frequency" or "LNA gain" |
| `value` | Normal values |
| `value_hi` | Highlighted values: frequency, total gain, board name |
| `status_ok` | Healthy status indicators |
| `status_warn` | Warning status indicators |
| `status_crit` | Critical status indicators |
| `peak_hold` | The spectrum's peak-hold trace |
| `noise_floor` | The spectrum's noise-floor line |
| `stale` | `[STALE]` titles and their dimmed borders |
| `observer` | Observer mode's status dot and accent |

> **Heads up: overrides don't survive a save.** Only `theme.base` is written back
> when you quit with `q`, so any override lines you add are removed from the file
> the first time you do. They load and work perfectly; they just aren't preserved.
> Until that's fixed, keep them in a config you don't quit-and-save over and point
> sdrtop at it with `--config`. Details in
> [Configuration](config.md#what-survives-a-save).

Six themes ought to be enough to argue about. If they aren't, the overrides are
here so you can out-bikeshed me entirely, no judgment.
