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

Overrides are preserved when you quit with `q`, along with everything else in
your config.

---

## Write your own theme

Overrides are for changing a colour or two. When you want a whole scheme, write it
as a file.

Call it whatever you like. Drop `tokyonight.toml` in `~/.config/sdrtop/themes/`
and `tokyonight` is a theme from then on, sitting alongside the six built-in ones:

```toml
[theme]
base = "tokyonight"
```

or from the command line, without touching the config at all:

```sh
sdrtop --theme tokyonight
```

That is the whole installation step. There is no list to register the name in and
nothing to rebuild.

A theme file names itself, gives all fourteen colours from the table above, and
lists the gradient the spectrum and waterfall are painted with:

```toml
name = "tokyonight"

border_dim     = "#57607a"
border_default = "#3c7891"
border_accent  = "#00d7ff"
border_focused = "#ffffff"
label          = "#8296b2"
value          = "#c3d2dc"
value_hi       = "#ffaf00"
status_ok      = "#00d282"
status_warn    = "#ffaf00"
status_crit    = "#ff5a5a"
peak_hold      = "#ffd700"
noise_floor    = "#505f78"
stale          = "#3c414b"
observer       = "#6496ff"

# Cold (weak signal) to hot (strong). `at` runs 0.0 to 1.0 and must climb.
palette = [
    { at = 0.00, color = "#0a0a50" },
    { at = 0.25, color = "#0050b4" },
    { at = 0.45, color = "#00d2d2" },
    { at = 0.60, color = "#00d250" },
    { at = 0.78, color = "#ffd700" },
    { at = 1.00, color = "#ff3214" },
]
```

Those are the built-in `sdr` colours verbatim under a new name, so the quickest
start is to copy `sdr`'s values and change the hex.

Three things worth knowing:

- **Every colour is required.** A theme missing one would render that part of the
  screen in some other theme's colour, which is a blend nobody chose, so sdrtop
  refuses the file instead.
- **A broken file is skipped, not fatal.** sdrtop falls back to a working theme
  and writes the reason to `~/.config/sdrtop/sdrtop.log`; a stray comma in a
  colour scheme should not stop a radio from starting.
- **Reusing a built-in name replaces it.** If you do name your file `nord.toml`,
  yours is the Nord sdrtop loads. That is occasionally handy if you just want to
  fix one thing about a built-in and keep calling it by its name, but you almost
  certainly want a new name instead.

Per-field `[theme]` overrides still apply on top of your own theme too, so you can
keep a scheme in a file and still nudge one colour from your config.

Six themes ought to be enough to argue about. If they aren't, the overrides and
theme files are here so you can out-bikeshed me entirely, no judgment.
