# What's New

← [Back](README.md)

The story of sdrtop so far, not as a wall of dates but as **checkpoints**: the
moments where the app levelled up. Newest first, so scrolling down goes backwards
in time.

> **A note on the keys in the older entries.** Checkpoint 19 grouped the layouts
> into sections and gave each section its own numbers, so a bare `(7)` in an entry
> below means whatever that key meant at the time. Everything from 19 onward
> writes them as section then number, like `Lab 3`. The current keys are always in
> [Keyboard shortcuts](keys.md); the entries below are a record of what happened,
> not a reference.

> **Where we are now:** the interactive TUI is feature-complete and both radios
> are fully supported. The current arc is instrument-grade polish: the **Command
> Rail** cockpit, the redrawn **Lab IQ**, the rebuilt **Lab RF** bench, the **Lab
> Timing** real-time bench and the **FM MPX · Demod** instrument with RDS, most
> recently gone over reading by reading for anything that was not strictly true,
> and now reachable through a menu instead of a row of numbers that had run out.
> The ongoing work is polishing the UI, sharpening the radio math, and squashing
> bugs. So if something looks off or behaves oddly, that's exactly what we're
> hunting.

---

## 🧭 Checkpoint 19: A front door *(you are here)*

sdrtop had fifteen layouts and ten digits to reach them with. The arithmetic had
been losing for a while: `main` never got a key at all, `0` walked the micro views
in a fixed order whether or not you wanted the one it stopped on, and `?` opened a
help overlay that had been quietly wrong for several releases. Nothing tied that
overlay to the keys the app actually handled, so when the keys moved, the help
did not.

**There is a menu now.** `Esc` opens it, and it is the first thing you see when
sdrtop starts. Two columns: the families of layout on the left, the layouts in the
selected family on the right, each with the number that opens it.

**Layouts are grouped into four sections**, and each section has its own numbers.
`Command Rail` holds the general views, `Lab` the four measurement benches,
`Sweep` the band scan in both sizes, `Micro` the field views. `2` is the RF bench
inside Lab and the spectrum inside Command Rail. The docs write that as `Lab 2`.

That is the whole idea: the same nine digits, four times over, instead of one row
of ten that had run out. `p` now steps to the next layout in the section you are
in rather than walking every preset in the app alphabetically, which used to drop
you into a micro view halfway through comparing two benches.

The menu opens with the cursor on the layout you are already using, so `Enter`
puts you back. At startup that is the layout you quit from, which makes the first
`Enter` of the session a resume rather than a choice.

A few things fall out of it:

- **`main` is reachable.** It is `Command Rail 5`. It has been in sdrtop since
  0.2.0 and the docs' honest answer was "press `p` until it comes round".
- **The compact sweep moved.** It used to be the fifth stop in the `0` cycle;
  it is `Sweep 2` now, next to the full-size sweep it is a small version of.
  Grouping by what a view is for beats grouping by how small it is.
- **The key reference cannot go stale again.** It is a pane in the menu, and the
  test suite checks it against the app's own key dispatch in both directions: a
  key with no entry fails the build, and an entry for a key that no longer exists
  fails it too. That check is the actual fix. The overlay was not wrong because
  nobody updated it; it was wrong because nothing could tell.
- **Your own layouts can join in.** A preset can declare which section it belongs
  to, which number opens it, what the menu calls it and a half-line describing it.
  See [Layout presets](presets.md#where-it-appears-in-the-menu).
- **There is an Options pane, and it is empty.** It says so when you open it. It
  is there ahead of its first setting so that adding one is a row rather than a
  rebuild.

Gone: `?` and `0`. The first is the menu's Keys pane, the second is the Micro
section.

---

## 📦 Checkpoint 18: You can just install it now

No new panels, no new maths. This one is about how sdrtop reaches your machine,
which up to now was "clone it and hope your Rust is new enough".

- **sdrtop is on [crates.io](https://crates.io/crates/sdrtop).** `cargo install
  sdrtop --locked` is now the shortest path to a working binary, on any
  architecture and any distribution, because it compiles on your machine and
  links what your machine actually has. No release page to visit, no name to
  get right.
- **The installer stopped pretending to be a build system.** It used to carry
  its own download-and-compile pipeline, a second unmaintained copy of the build
  recipe. Now it does the two jobs cargo cannot (your distribution's libraries,
  and a Rust new enough to matter) and hands the rest to `cargo install`.
- **Downloads are verified, properly.** The checksum check used to be skippable
  by a network hiccup: if `SHA256SUMS` failed to download, the install carried on
  without a word. Now it stops. `--no-verify` exists if you insist, and it says
  so out loud.
- **Device permissions are reported, not meddled with.** The installer no longer
  writes its own udev rules or edits your groups. The `libhackrf` and `rtl-sdr`
  packages ship rules already, and two sets of rules that agree only by
  coincidence is worse than one. It tells you what it found and what to do if
  there is nothing.
- **`--version` says which build you have**, like `sdrtop 0.4.2 (a1b2c3d)`, with
  a `-dirty` marker if it came from an edited tree. Bug reports stop being
  ambiguous, which matters now that `install.sh --git` can hand you the `main`
  branch on request.
- **The release tarball is reproducible and signed for.** Build the same commit
  twice and you get the same bytes, and `gh attestation verify` will confirm a
  download really came out of this repository's release workflow rather than
  merely arriving intact. Both from 0.4.2 onward.

New installer flags: `--git` to build `main`, and `--no-verify` for the
determined. Everything else works as it did.

---

## 🔭 Checkpoint 17: Two fixes on the plots

- **The frequency zoom now works on the spectrum-only view.** `+` and `-` in
  spectrum focus set one zoom for the whole instrument, but the standalone
  spectrum was ignoring it: the keys did nothing on preset `2`, and a zoom you had
  set on the Command Rail was thrown away the moment you pressed `2` to look at
  the same signal larger. The plot and its frequency axis now follow the zoom in
  every layout.
- **The waterfall fills its panel again.** On preset `3` the waterfall gets the
  whole screen, and each row of the plot holds two rows of history, so a tall
  terminal needed more history than the buffer kept. It ran out and left a blank
  strip above the bottom border that never filled. The buffer is deeper now, and
  is raised to a usable depth at startup even if your saved config asks for less,
  so this also gives `J` / `K` a lot more to scroll back through.

---

## 🔬 Checkpoint 16: Small things that were quietly wrong

A pass over the parts of the code nobody had split up yet. Nothing here is a new
feature; three things that had been subtly wrong on screen are now right.

- **The field gain view lines up on an RTL-SDR.** The Tuner, VGA and AGC rows
  each started in a different column, because the label widths were set for
  three-letter names and `Tuner` is five. HackRF was never affected, which is why
  it went unnoticed.
- **The callback strip chart's caption is no longer cut off.** It has three
  lengths and picked the longest one for the panel width, but the widths it
  compared against were guesses: at 48 columns it read `...from the expe`, at 80
  `...from the expected p`. It now picks the longest sentence that actually fits.
- **The layout keys tell the truth.** `1` to `4` logged "Preset: spectrum" even
  when that layout was missing and nothing had changed. They now say the layout
  is not available, the way `5` to `9` already did. The log also names the layout
  exactly as you would write it in `config.toml`.

Behind the scenes, every panel can now be tested without a radio attached, which
is how all three of these were found.

---

## 📏 Checkpoint 15: One number, one meaning

The SAT reading used to disagree with itself. The Command Rail called it calm
below 10 %, while the micro views and the lab benches went amber at 1 % and red
at 5 %, so the same saturation could show green in one corner of the screen and
red in another at the same instant, with nothing to say which was right.

- **There is now one saturation scale**: green under 1 %, amber to 5 %, red
  above, everywhere it appears. If SAT is red, it is red on every screen.
- **The rail's advice moved to clip headroom**, which is what the reading beside
  a gain control actually wants. The Bench card's CHAIN verdict says "optimal"
  while the peak sits in the same window the auto-gain leaves alone, "hot" above
  it, "low" when there is gain going unused, and "clipping" once samples really
  are hitting the rails. The rail was given a laxer scale so it would not cry
  wolf at a well-driven front end; this gives it that without the number having
  to mean two things.
- **The `⚠ last clip` memory** under SAT now fires at the same point the reading
  turns red, rather than at a threshold of its own.
- **Staleness is decided in one place too.** A couple of panels kept their own
  copy of "how old is too old". They happened to agree, and one of them already
  disagreed about the moment before the first reading arrives.

Nothing you do changes; a few numbers change colour a little earlier in the
rail, and they now agree with the rest of the app.

---

## 🎨 Checkpoint 14: Bring your own

Themes and layouts both became things you can just *write*, plus one overdue
bug fix.

- **Per-field colour overrides survive a save.** They always loaded correctly, but
  quitting with `q` rewrote the config and deleted them, so anyone using them had
  to keep a separate file and point `--config` at it. That workaround is no longer
  needed.
- **You can write a whole theme now**, not just override a colour or two. Drop
  `tokyonight.toml` in `~/.config/sdrtop/themes/` and `tokyonight` is a theme from
  then on, usable from `[theme] base` or `--theme`, alongside the six built-in
  ones. Nothing to register, nothing to rebuild. The built-ins are written in
  exactly the same format, so the quickest start is to copy one and change the
  hex, and a broken file is skipped with a note in the log rather than stopping
  the radio from starting. See [Themes](themes.md#write-your-own-theme).

- **Layouts work the same way now.** Drop `nightwatch.toml` in
  `~/.config/sdrtop/presets/` and `nightwatch` is a preset from then on, in the
  `p` cycle alongside the built-in ones. Nothing to register, nothing to rebuild.
  You can still define presets inline in `config.toml` with `[presets.my_view]`;
  a file is just easier to keep, share, and not lose in a growing config. See
  [Configuration](presets.md#a-preset-per-file).

Under the hood these are one change: the six built-in themes and the sixteen
built-in layouts stopped being Rust code and became data files. That is what makes
your own possible, and it means the built-ins are now the worked examples. Copy
one and change it.

---

## 🔬 Checkpoint 13: It checks itself

No new instrument this time. Instead, **Lab Signal** (`8`) was taken apart reading
by reading and asked one question: is this number true? Several were not, and one
wrong number was feeding four others.

- **Occupied bandwidth was measuring the captured span, not the signal.** A clean
  broadcast station reported 7.49 MHz of occupancy. It now finds the carrier and
  measures the 99 % bandwidth across *that*, which fixes the four readings that
  hung off it: the modulation badge stops guessing from a span, the
  adjacent-channel bands stop overlapping the channel they are compared against
  (they used to read 0.0 dB on both sides with full red bars), and the verdict
  stops permanently announcing "adjacent splatter" for a signal with none. A
  broadcast now reads around 100 kHz, which is what a real programme actually
  occupies, and the panel explains why that is narrower than the 200 kHz
  allocation
- **Adjacent-channel spacing follows the modulation**: ±200 kHz for broadcast FM,
  ±25 kHz for narrowband, ±9 kHz for AM, with the row labels naming whichever
  offset was measured so the two can never disagree
- **The noise floor is now also given as a density** in dBFS/Hz beside the per-bin
  figure. The per-bin number rises with the bin width, so it changes when you
  change the sample rate and describes the analyser as much as the radio; the
  density divides that out and reports the same receiver as the same receiver
- **RDS stopped outliving its station.** Retune and everything the decoder holds
  is dropped, including the station identity, because none of it describes the new
  frequency. Lose the subcarrier without retuning and the name ages visibly first,
  then goes. The reverse also got fixed: a dropped block used to throw away
  seconds of accumulated name and text, and now only block synchronisation
  restarts while what was already decoded stays
- **RDS reads accents.** RDS is not ASCII, and every accented letter used to
  arrive as a blank. A Hungarian title now reads as a Hungarian title. `Groups`
  shows two numbers, the total on this channel and the current unbroken run, so a
  low count tells you whether the problem is the signal or your machine. And when
  the machine is the problem the panel says so outright, instead of looking like a
  station without RDS
- **The demod panel behaves at every terminal size.** Idle, it shows one centred
  message instead of five empty section headings. Cramped, it sheds bar graphs and
  secondary numbers rather than letting the RDS section fall off the bottom.
  Roomy, the MPX trace grows to three rows, which is the difference between seeing
  the 19 kHz stereo pilot and not. The `AUDIO` placeholder that had nothing under
  it is gone, `Offset` in the deviation section is now `Carrier` (the headline
  already used "offset" for the opposite measurement), and `C` logs a snapshot in
  AM as well, where it used to answer "no measurement to snapshot" with the depth
  bar on screen right above it
- **Capitals work everywhere.** Every key hint in sdrtop is written as a capital,
  and roughly half the panels only accepted lowercase. `Shift+Q` now quits, like
  it always looked as though it should
- **Custom presets get the real thing.** The demodulator used to run only in the
  preset literally named `lab_signal`. It now follows the `fm_demod` panel, so
  your own layout gets a working demod bench. Bandwidths also print identically
  everywhere; there were five slightly different formatters disagreeing about the
  same number
- **The documentation was rebuilt to match.** Every page checked against the code
  rather than against the other pages, which turned up fifteen places where the
  docs described something the program doesn't do and eight features that were
  shipped but written down nowhere, including the whole measurement banner
  workflow

---

## 📻 Checkpoint 12: It listens

Every instrument so far described a signal from the outside: how strong, how wide,
how clean. The new **FM MPX · Demod** panel in **Lab Signal** (`8`, focus `m`)
opens the channel up and reads what is inside it. There is still no audio anywhere
in sdrtop. This is a measurement instrument, and it reports things a spectrum plot
cannot.

- **Deviation**: peak and RMS deviation measured *about the carrier*, so a
  mistuned radio reports its tuning error as offset instead of inflating the
  modulation figure, with the bar drawn against ±75 kHz for broadcast or ±5 kHz
  for narrowband
- **MPX baseband**: the demodulated composite from 0 to 60 kHz as a live profile,
  with the 19 k pilot, 38 k stereo difference and 57 k RDS subcarrier all visible
  where they live
- **Pilot / stereo**: STEREO, MARGINAL or MONO, plus the pilot's **injection
  percentage** against the 8 to 10 % broadcast norm, which reads a transmitter's
  health rather than your reception
- **RDS**: station name, PI code, programme type, traffic flags and **RadioText**,
  decoded off the 57 kHz subcarrier. Nothing appears until it has been confirmed
  twice, so a mis-decoded character never flickers on screen and gets corrected in
  front of you
- **CTCSS** for narrowband FM: the subaudible tone that opens a repeater's
  squelch, picked out of the standard 40-tone table with the margin it won by, and
  an honest "searching" state while it fills its half-second window
- **AM depth**: modulation depth with positive and negative peaks reported
  separately, because a negative peak approaching 100 % pinches the carrier off
  and splatters
- The demodulated channel tunes **inside** the captured spectrum with `←` / `→`,
  `P` snaps it to the strongest carrier while ignoring the radio's own LO leakage,
  and `T` forces the demodulator when the automatic classifier is too coarse to
  pick one

---

## ⏱️ Checkpoint 11: Lab Timing

An instrument for a question the others could not answer: is your computer keeping
up with the radio in real time? The radio ships samples in steady USB bursts, one
callback at a time, and your machine has to catch every one on schedule or the
buffer backs up and samples drop. **Lab Timing** (`7`) watches that handoff and
grades it.

- **Timing Diagnostics** (`t`): measured callback period versus the expected
  period at your sample rate, host clock drift in ppm, jitter, and per-callback
  deviation percentiles (p95 / p99 / peak) drawn against a **deadline budget**
  that scales with the rate, plus a late-callback count and a plain verdict
  (Excellent / Good / Marginal / Poor)
- **Callback Interval Strip Chart**: every point is one real callback, plotted as
  how far its arrival drifted from the expected interval. Late deliveries climb,
  early ones dip, and anything past the deadline band gets tagged (▲ late,
  ▼ early), so a host hiccup is something you watch happen instead of guess at
- **Hardware Vitals** (`v`): the supporting cast. Sample drops, ADC saturation and
  CPU / RAM as 60 second trends, USB link utilization against the device's real
  ceiling, ring-buffer overrun headroom, and uptime
- Quieter fixes that rode along: **Lab IQ** now gates carrier and image detection
  on the noise floor (an idle radio reads "no signal" instead of flagging a noise
  bin as a carrier), gains clamp to each device's own model on load, the RTL-SDR
  AGC indicator stays honest after a manual gain nudge, and the spectrum and
  waterfall cursor respects each radio's true frequency range

---

## 🎛️ Checkpoint 10: The instrument cockpit

The polish arc grew teeth: the UI started reading like a real radio's front panel,
not a table of numbers.

- **Command Rail** (`1`, now the default): a left instrument rail with a big
  segmented **frequency hero**, an analog **S-meter**, the HUNT·MONITOR·BENCH mode
  tabs whose lead card follows what you're doing, recall slots with live activity
  pips, and a **SIGNAL** zone where SNR·PWR·NF·SAT each ride their own braille
  oscilloscope trace beside the value
- **Lab IQ, reimagined**: IQ diagnostics redrawn as analog **null-meters** (centre
  is ideal, the needle shows the deviation), paired with a **persistence
  constellation**, a density-coloured I/Q cloud with a fitted imbalance ellipse
  whose stretch is amplitude imbalance and whose tilt is phase imbalance
- **Lab RF, rebuilt as a front-end bench** (`6`): three panels that teach one idea,
  that level climbs stage by stage, the signal/noise gap *is* the SNR set at the
  antenna, and gain only parks that gap in the ADC window. **RF Diagnostics** (gain
  lineup, staging, Friis noise figure, sensitivity, verdict), a **Gain-Staging
  Level Diagram** (signal and noise traces climbing ANT▸LNA▸MIX▸VGA▸ADC), and an
  **ADC Loading** panel (signed-sample histogram bell, loading stats, a modeled
  linearity card). Focus it with `D` and press `A` to auto-stage the gain, or `⎵`
  to freeze the bench. The dBm are honestly labelled *modeled / relative*, never a
  wattmeter
- A shared braille-instrument language (oscilloscope traces, ⅛-block gain bars,
  gradient fills) applied across the rail, with the radio math left exactly as
  honest as it always was. No "AI-enhanced" anything; the only thing that learns
  here is you

---

## 📡 Checkpoint 9: A second radio

sdrtop stopped being a one-device app.

- **RTL-SDR support** (R820T / R828D / E4000) lands alongside the HackRF One,
  behind a clean `SdrDevice` abstraction layer. The HackRF path is untouched, the
  RTL path shares the same RX → FFT → UI pipeline
- The UI **adapts to the hardware**: HackRF's LNA/VGA/AMP versus RTL-SDR's single
  tuner gain plus AGC, the right frequency and sample-rate ranges, and N/A where a
  measurement doesn't apply (no BB filter, no Friis NF)
- Plug in more than one radio and a **device picker** greets you at launch;
  `--device hackrf|rtlsdr` pins one
- Confirmed on real hardware in normal RX *and* observer mode, with FM reception,
  tuner gain, AGC and sweep all checked out. The open question is the zoo of RTL
  clones, which no single person owns. **So this is where you come in:** run it on
  yours and [open an issue](../../../issues) with how it went

---

## 🔧 Checkpoint 8: Polish

The feature list closed. This checkpoint was about taste: refining layout and
readability, **reworking the micro view's UI**, double-checking every radio
calculation, and fixing the rough edges. The groundwork that made the next leap
safe to land.

---

## ✅ Checkpoint 7: It scans

- **Frequency sweep** (`9`): scan a band wider than one window can show, and
  sdrtop stitches it into one curve with band-plan labels. Focus with `g`, set the
  band live with `S` / `E`, and press `Enter` on a peak to tune straight to it
- **Micro field views** (`0`): deliberately tiny single-glance read-outs (overview
  · signal · gain · health · sweep) for slim splits, SSH sessions, and cyberdeck
  screens

---

## ✅ Checkpoint 6: The lab bench

Bench-engineer views for people who care about the numbers, not just the picture.

- **Lab presets** on `5` to `8`: IQ · RF · timing · signal
- Derived measurements worth trusting: **NF**, **MDS**, **IRR**, **PAPR**,
  sample-rate accuracy, and USB **timing/jitter** with a quality verdict
- **Hardware Vitals** now tracks sdrtop's own CPU/RAM with trend graphs
- Every lab panel marks itself **[STALE]** the instant RX stops, so a frozen
  number is never mistaken for a live one

---

## ✅ Checkpoint 5: It analyzes

The spectrum and waterfall grew real tools, driven by a single highlighted
**focus** key per panel.

- **Spectrum focus** (`e`): tune with `←`/`→`, **zoom**, **hold** a ghost frame to
  compare, a **cursor** read-out, **band-plan** labels, and named **markers** that
  persist
- **Waterfall focus** (`l`): adjustable color scale, scroll-back through history,
  and **frame averaging** to stretch the visible time window

---

## ✅ Checkpoint 4: It plays nice

Less crashing, more cooperating.

- **Observer mode**: if another app already holds the radio, sdrtop watches what
  it can instead of falling over, then reclaims it when free
- Live **sample-rate control** (`s`) without restarting
- A big **performance overhaul**: far lower CPU/RAM at 30 fps, smooth even at high
  sample rates

---

## ✅ Checkpoint 3: It diagnoses

The part that makes sdrtop more than a pretty spectrum.

- **Hardware health**: drops, ADC saturation, USB errors, buffer fill,
  sample-rate accuracy
- **RF chain**: gain stages, frequency plus wavelength, estimated **noise figure**
  and **minimum detectable signal**
- **IQ diagnostics**: DC offset, imbalance, **image rejection ratio**, plus an ADC
  amplitude **histogram**

---

## ✅ Checkpoint 2: It remembers

sdrtop stopped being forgetful.

- Settings (frequency, gains, sample rate, layout) **persist** across restarts in
  `~/.config/sdrtop/config.toml`
- Atomic, safe saves; a missing or broken config falls back to sane defaults
- **Six themes** (`sdr`, `nord`, `dracula`, `gruvbox`, `catppuccin`, `solarized`)
  and switchable **layout presets**

---

## ✅ Checkpoint 1: It receives

The foundation: talk to the radio safely, pull IQ off the wire, and show it.

- Solid USB FFI layer with a clean shutdown on every exit path
- Live **spectrum analyzer**: FFT with peak hold, noise floor, dBFS and frequency
  axes
- Scrolling **waterfall**: truecolor / 256-color / 16-color, with a graceful
  fallback on basic terminals

---

```
  ┌─[ sdrtop · 2026 ]──────────────────────────────────
  │  $ ./sdrtop --scan-for-hype
  │  > 0 LLMs detected in the signal path
  │  > no neural nets, no "AI-powered" sticker
  │  > just honest FFTs and a person who likes radios
  │  > carry on.
  └────────────────────────────────────────────────────
```

In a year where everything claims to be AI-powered, sdrtop is proudly powered by
math you can check yourself. The dBFS numbers are real, the bugs are mine. 📻
