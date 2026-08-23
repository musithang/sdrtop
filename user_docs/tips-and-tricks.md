# Tips and Tricks

← [Back](README.md)

Practical recipes: how to get gain right, how to find something that's barely
there, and how to set up a capture that survives being left alone overnight.

---

## Setting gain

### What "right" looks like

A well-set gain fills the middle of the ADC's range: enough headroom above to
catch peaks, enough margin below to stay clear of the noise. Too little and you're
digitising mostly noise; too much and the ADC clips, which is worse than it
sounds. A clipped ADC doesn't politely round up, it makes things up, and a
confident lie is the worst kind of data.

The fastest way to see it is the **ADC Loading** bell in Lab RF (`6`): a shape
that fills the middle without piling up on the rails. If you want exact numbers,
add the `iq_histogram` panel to a [layout of your own](presets.md)
and aim for:

- **Low** under 5 %: the ADC isn't wasting bits on empty space
- **Mid** 60 to 80 %: the healthy zone
- **Clip** 0 to 5 %: room for peaks, but not clipping

### Let it do it for you

Lab RF (`6`), focus with `d`, press `A`. That stages LNA and VGA to the target in
one press, filling LNA first to protect the noise figure. Press `A` again once
you're at the optimum and it latches a continuous track that follows level drift.
Any manual gain key drops the latch, so it never fights you.

This is usually better than the table below, because it reads your actual signal
rather than a guess about your situation.

### Starting points by hand

| Situation | HackRF LNA / VGA | RTL-SDR tuner |
|-----------|------------------|---------------|
| Weak signal, quiet band | 40 / 60 | Near the top of the table |
| Moderate signal | 24 / 20 | Around the middle |
| Strong local transmitter | 8–16 / 10 | Low, or AGC on |
| Urban, many strong signals | 0 / 0 | Lowest, and work up |

Use these as starting points, not rules. Every antenna, frequency and environment
is different.

If you're clipping regularly on a HackRF, turn **VGA** down first: it's the finer
control, and it doesn't cost you noise figure the way pulling LNA down does. On an
RTL-SDR there's one gain, so step it down with `↓`, or hand the problem to the
tuner's **AGC** with `a`.

---

## Pulling a weak signal out of the noise

This is what the [measurement banner](lab.md#the-measurement-banner) is for, and
it's the most useful thing in sdrtop that nothing on screen advertises.

1. Open any lab preset and focus the banner with `b`.
2. Press `]` a few times to raise **trace averaging**, up to 16.

Watch the noise floor flatten while the signal stays put. Something that was
indistinguishable from the grass at averaging 1 can be obvious at 16. Why that
works, and what it costs you in reaction time, is explained on the page linked
above.

Then drop back to 1 when you need to watch something happen rather than measure
it.

### Before and after, without relying on memory

Same panel, the `C` key:

1. Set averaging up so the trace is stable.
2. Press `C` to capture a **reference trace**. It stays on screen as a ghost.
3. Change one thing: swap the antenna, add a filter, move the coax, turn on the
   DC block.
4. Read the difference directly off the screen.

This beats writing numbers down, because you get the difference at every frequency
at once rather than at the one you happened to note. Press `C` again to clear it.

A reference **level** (`↑` / `↓` on the same banner) is the other half: put the
line at whatever level makes a signal worth chasing, and anything poking above it
is a candidate.

---

## Finding things

### Quick manual scanning

Press `f`, type a frequency in MHz, `Enter`, look for a second or two, repeat. It
sounds primitive and it's often the fastest thing when you have a shortlist.

For a band rather than a list, use **Lab Sweep** (`9`) instead. It scans wider
than one window by retuning across the band, and `Enter` on the cursor tunes
straight to whatever you found.

### Recall slots: three frequencies, one keypress

The Command Rail (`1`) keeps three recall slots, and they're the fastest way to
compare signals.

1. Press `c` to focus the rail.
2. Tune to something interesting.
3. Press `M` to save it into the next slot.
4. Repeat for a second and third frequency.
5. Now `1`, `2` and `3` jump between them instantly.

Each slot shows a little activity pip when that frequency has a signal on screen
right now, so you can watch three channels at once without tuning to any of them.
The slots persist in your config, so tomorrow they're still there.

This is the setup for "is the repeater up?", "which of these three is stronger?",
and "did that intermittent thing come back?".

### Markers as a personal band plan

Place markers at the edges and channels you care about, and they stay with you:

1. Press `e` for spectrum focus.
2. `j` / `k` to move the cursor.
3. `m` to place a marker, then type a name and `Enter`.

Now tuning across the band always shows you where things are. Markers persist in
the config, and you can write them there directly; see
[Configuration](config.md#spectrum-markers).

Once a marker exists, `b` cycles a **channel bandwidth** onto the nearest one
(6.25 kHz through 500 kHz, then none), which draws the channel width around it.
Handy for seeing at a glance whether a signal fits the channel it's supposed to be
in.

---

## Reading the spectrum

### Hold: compare two things

`h` freezes the current trace as a ghost while live data keeps drawing over it.

- **Compare two signals.** Freeze on one, tune to another, read the difference.
- **Watch a range.** Freeze, then watch the live trace rise and fall against it.
- **Catch a change.** Freeze before you touch something, look after.

`h` again releases it. This is the quick version of the reference trace above; the
banner's `C` is the one that survives retuning and works with averaging.

### Zoom the level axis

`↑` in spectrum focus expands the dBFS axis to spread out weak signals; `↓`
compresses it to fit a wide dynamic range on screen. Useful when one strong local
transmitter is squashing everything else flat.

This is a different axis from `+` / `-`, which zoom **frequency**. Both live in
spectrum focus and it's easy to reach for the wrong one.

### The cursor

`j` / `k` walk a cursor across the trace, reading out the exact frequency and
level as it goes. Point it at an unknown spike, read the frequency, and if it's
worth keeping, `m` marks it.

### Trace style

`d` cycles braille, filled and scatter. Braille is the most precise, filled reads
best at a glance across a room, and scatter is easiest on a slow terminal. Your
choice is saved.

---

## Reading the waterfall

### Scroll back through history

Press `l` for waterfall focus, then `j` / `k` to move through the stored history.

- **Intermittent signals**: scroll back and see when they came and went.
- **Duration**: count rows between start and end.
- **Interference patterns**: many interferers are periodic, and the pattern is
  usually obvious once you can see a minute at a time.

### Slow motion

`[` averages more frames into each row, stretching the visible time window; `]`
speeds it back up. If something appears and vanishes before you can look at it,
slow the waterfall down.

### Zoom and palette

`+` and `-` zoom the frequency span, narrowing onto the centre. Note this is the
*inverse* of the spectrum's `↑`/`↓`: the waterfall zoom changes frequency span,
the spectrum zoom changes level range.

`p` cycles the color palette (classic, amber, ice, phosphor). `classic` follows
your [theme](themes.md); the others are fixed gradients. Beyond taste, a different
gradient genuinely makes different things visible, so it's worth trying another
one when a signal is right at the edge of showing up.

---

## A capture-setup checklist

The lab presets are built for exactly this, one concern each. A typical run
through:

1. **Tune** to your target with `f`, and **start RX** with `Space`.
2. **Gain, Lab RF (`6`).** Focus with `d`, press `A`, and check the ADC Loading
   bell fills the middle without touching the rails. Read **NF** and **MDS** to
   confirm the receiver is sensitive enough for what you're after.
3. **Quadrature, Lab IQ (`5`).** The constellation should be a bright ring
   comfortably inside the unit circle; smeared out to the edge means clipping.
   **IRR** above 30 dB and **DC spike** below −40 dBFS mean clean quadrature. If
   the DC spike is in your way, press `D`. If mirror images are, park on a strong
   carrier and press `C`.
4. **Stability, Lab Timing (`7`).** The timing verdict should read Good or
   Excellent, and the ring-buffer fill should be flat rather than trending up.
5. **The bench you'll actually watch.** Set averaging, and capture a reference
   trace before you change anything.

If all four check out, the run is worth starting.

---

## During a long capture

Keep **Hardware Vitals** in view (Lab Timing, `7`, focus `v`):

- **Buffer fill trending toward the ceiling** is the warning that matters, and it
  arrives *before* the drop counter does. Act on this one.
- **Drops climbing**: USB or CPU can't keep up. Lower the sample rate with `s`, or
  try a different cable.
- **CPU trending up**: something else on the machine woke up. Close it.

If you want to know whether the problem is your computer rather than your radio,
the **Callback Interval Strip Chart** in the same preset shows every USB callback
as it arrives, so a scheduler stall is something you watch happen instead of
deduce.

---

## SSH, tmux and small screens

sdrtop is built to live in a pane. It needs a real terminal (piping or redirecting
its output gets you nothing), but that's the only requirement.

```sh
# a radio on a Pi, from your desk
ssh pi@raspberrypi.local sdrtop --theme nord --frequency 433920000

# a permanent corner of your tmux session
tmux split-window -v -p 30 'sdrtop --lna 24'
```

On anything cramped, press `0` for the [micro field views](screens.md#micro-field-views),
which strip each concern down to one glance and stay readable down to about 40
columns.

One habit worth having: if you're scripting sdrtop or trying out a layout, pass
`--config /tmp/something.toml`. `q` saves, so a script that runs sdrtop without it
will quietly rewrite your real settings.

---

← [Back](README.md)
