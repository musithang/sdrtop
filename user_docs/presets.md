# Layout presets

← [Back](README.md)

A *preset* is a named arrangement of panels: which ones are on screen, where they
sit, and how much room each gets. sdrtop ships with twenty-eight, grouped in the
[menu](screens.md#the-menu), and you can write your own.

There are two places to put one, and they behave identically:

- a `[presets.my_view]` block in `config.toml`, for something small you want next
  to the rest of your settings;
- a file of its own in `~/.config/sdrtop/presets/`, for a layout you want to keep,
  share, or not lose in a growing config.

Either way it is merged with the built-ins at startup, and your `config.toml`
blocks are round-tripped verbatim on save, so hand-written presets survive
quitting untouched.

**Every preset is overridable, including the built-ins.** Define one with the same
name as a built-in (`command_rail`, `spectrum`, `waterfall`,
`spectrum_waterfall`, `main`, `lab_iq`, `lab_rf`, `lab_timing`, `lab_signal`,
`lab_sweep`, `micro_main`, `micro_signal`, `micro_gain`, `micro_health`,
`micro_sweep`, `net`, `net_survey`, `net_census`, `net_ble`, `net_bt`,
`net_piconet`, `net_bench`, `net_connection`, `net_coded`, `wifi_survey`,
`wifi_frames`, `wifi_airtime`, `radio_pocsag`, `observer`) and your version replaces it, so the number key that
opens it now opens your layout. Those names are the whole list; a name that isn't
on it is a new preset, which appears in the menu automatically rather than taking
over a key.

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

## Where it appears in the menu

Five optional fields put a preset somewhere in the [menu](screens.md#the-menu),
give it a number key and tell the menu's card what to say about it. Leave them
all out and the layout still works; it just lands under **Lab › Other** with no
shortcut.

```toml
[presets.my_view]
section = "lab"
slot    = 5
title   = "Nightwatch"
blurb   = "Waterfall and log, for leaving running"
about   = "The waterfall over the whole screen and the log beside it, for a night of watching one band."
panels  = [ ... ]   # exactly as above
```

| Field | What it does |
|-------|--------------|
| `section` | Which group it belongs to, by id: `command_rail` (**Spectrum**), `sweep` (**Sweep**), `lab` (**Receiver**), `micro` (**Compact**), `net` (**Overview**), `le` (**Low Energy**), `classic` (**Classic**), `coded` (**Long range**), `wifi_channel` (**Channel**), `pocsag` (**POCSAG**, under Radio), or a name of your own, which becomes a group under **Lab › Other**. The ids are older than the names and stay as they are, so a preset written for an earlier sdrtop still lands in the right place. `hidden` keeps it out of the menu entirely |
| `slot` | The number key, `1` to `9`, **within that group**. Leave it out and the layout is still listed, just without a shortcut |
| `title` | What the menu calls it. Defaults to the preset name |
| `blurb` | The half-line under the title. Optional, and worth writing: it is what tells you which of two similar layouts you want |
| `about` | A sentence or two for the menu's card: what you will see on this layout. Optional; without it the card leaves that paragraph out |

A few things follow from how these are read:

- **Numbers are per group**, so `slot = 5` in `lab` does not collide with
  `slot = 5` in `micro`. That is the whole point of the groups: nine keys per
  group, instead of one exhausted row.
- **Two presets wanting the same slot is not fatal.** The one whose *preset name*
  sorts first keeps the key, the other stays in the list without one, and the
  reason is written to the log so you can see it happened. Losing a shortcut is a
  better answer than losing the layout.
- **A section name sdrtop does not know becomes a group of its own**, under
  **Lab › Other**, titled with the name you gave it.
- **`hidden` is how the built-in `observer` preset stays out of the way.** It is
  loaded by sdrtop itself, so there is nothing to pick.

## A preset per file

Write the layout as its own file. Drop `nightwatch.toml` in
`~/.config/sdrtop/presets/` and `nightwatch` is a preset from then on, alongside
the built-in ones:

```toml
# ~/.config/sdrtop/presets/nightwatch.toml
panels = [
  { name = "header_slim",  position = "top",  height = 4 },
  { name = "command_rail", position = "left", width_pct = 28 },
  { name = "waterfall",    position = "body" },
  { name = "footer",       position = "bottom" },
]
```

That is the whole installation step. There is no list to register the name in and
nothing to rebuild; open the menu and it is there. The file is just the
`panels = [...]` part, without the `[presets.name]` header, because the file name
is the preset name.

The twenty-eight built-in presets are written in exactly this format, so the quickest
way to build on one is to copy it rather than transcribe it from the docs. They
live in the source tree under `src/config/presets/`.

Three things worth knowing:

- **A layout with no panels is refused**, and so is a file that will not parse.
  sdrtop skips it, writes the reason to `~/.config/sdrtop/sdrtop.log`, and loads
  the rest: one stray comma should not cost you your other layouts or stop the
  radio from starting.
- **Only `*.toml` is read**, so notes and backups in that directory are ignored.
- **`config.toml` wins over a file.** If a name is defined both as a
  `[presets.*]` block and as a file, the block is the one that loads, on the
  grounds that it is the file you edit by hand and the one sdrtop rewrites.

## Positions

| Position | Where it goes | Size field |
|----------|---------------|------------|
| `top`    | Full-width strip at the top    | `height` in rows |
| `bottom` | Full-width strip at the bottom | `height` in rows |
| `left`   | Left column of the body        | `width_pct`, percent of body |
| `right`  | Right column of the body       | `width_pct`, percent of body |
| `body`   | Centre column, fills what's left | none |

Position names are lowercase. A capitalised `"Top"` fails to parse, and what a
parse failure costs you depends on where the preset lives: in `config.toml` it
takes the [whole file down to defaults](config.md#where-the-config-lives), while a
file in `presets/` costs you only that one layout. Either way the reason is in
`~/.config/sdrtop/sdrtop.log`.

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

And one thing is attached to the **name**: a preset whose name begins with `lab_`
renders in **instrument mode**, with the frame colours cooled toward steel blue,
and it's the only kind of preset that draws the reference level and reference
trace overlays. If you want those, name accordingly.

## Panel names

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
`timing_vitals` · `system_resources`

**Sweep:** `sweep_panel` · `sweep_strip`

**Compact views:** `micro_panel` · `micro_signal_panel` · `micro_gain_panel` ·
`micro_health_panel` · `micro_sweep_panel`

**NET:** `net_capability` · `net_occupancy` · `net_coexist` ·
`net_decode_health` · `net_census` · `net_ble_packets` · `net_ble_detail` ·
`net_ble_connection` · `net_coded_packets` · `net_coded_detail` ·
`net_bt_hops` · `net_bt_piconets` · `net_bt_packets` · `net_bt_bench`. What each shows is on
[the NET page](net.md); three of these were renamed or replaced, which that
page's [last section](net.md#if-you-wrote-your-own-net-preset) lists.

**Wi-Fi:** `net_wifi_survey`, the thirteen channels side by side: how busy
each one's busiest megahertz was and the airtime its own frames stated, by
format, over the survey's visits ([Channel survey](net-wifi.md)).
`net_wifi_least_busy`, the survey's card: the least busy channel when the
visits can tell it apart, and in words when they cannot yet.
`net_wifi_river`, a capture of the channel's air as lanes of frames, each
in its format's colour, as wide as its airtime and as tall as its SNR, at
1 ms, 40 µs or 4 µs a cell.
`net_wifi_frames`, the capture's frames in time order from its trigger:
their format and what their signalling said, the gap before each, its
airtime, offset and SNR, and another channel's frames by how far away
they are.
`net_wifi_card`, the Frames view's card: this channel's frames a second
over whole seconds, by format, and the selected frame field by field with
its clock and its channel, or the account of every preamble found.
`net_wifi_airtime`, what is spending the channel's time, a column a
second: how busy its busiest megahertz was beside the airtime its frames
stated, by format, 0 to 100 %, with the reading in its title where the
card has no room.
`net_wifi_ledger`, what spent that time: a row a format and rate, its
share of the time heard, frames a second and airtime a frame.
`net_wifi_share`, the Airtime view's card: the share of the time the
frames took over the last 20 s with its 1 sigma, the energy's beside it,
and the rhythm, the gaps between frames against SIFS and DIFS.

**Radio:** `pager_face`, a POCSAG pager as one looks: the page on its
dot-matrix display, its address and what it took repairing, the buttons
with their keys, and the link along the bottom. `pager_inbox`, every page
received this session, newest first, with its text. `pager_card`, the page
on the display as it was received: rate and clock error, shift, offset,
level and decision SNR, preamble and sync, codewords and repairs, polarity,
address, what decided its kind, how it ended, and one codeword's bits with
the repaired ones lit. `ft8_map`, the world around you as FT8 hears it:
centred on your position (`[station]`), every station in the true direction
of its path and drawn only where it said it was, the land coloured by the
sun for day, grey line and night, on a logarithmic or a linear range.

**Observer:** `observer`

A name sdrtop doesn't recognise is skipped rather than fatal, so a typo costs you
that panel and not the layout.

---

Presets decide *which* panels you see. [What you see on screen](screens.md)
explains what each one is telling you, and [The Lab presets](lab.md) covers the
measurement benches in depth. For the rest of the config file, see
[Configuration](config.md).

← [Back](README.md)
