# Bench · `Classic 3` *(focus `c`)*

← [The NET section](net.md)


The same piconet, each end of it side by side: three columns, MODULATION,
CARRIER and TIMING, every reading two rows, `▶` the master's in the
piconet's colour and `◀` the slave's in the ordinary ink, each with its bar
on the same row, held against the limit the column's heading cites. The
heading also says what the readings rest on (`258 hdr`, `1204 hits`). A side
with nothing yet says so in words, never a zero. `3` opens it on the
piconet selected in the Piconets view; `← →`, with the bench focused, step
through the piconets.

- **Modulation**: each end's BR modulation index and deviation against
  0.28 to 0.35, read from the Core Specification, from every header's
  symbols. Read the way the SIG's test suite defines them, which took some
  doing. A tester commands the device to send `00001111` and `10101010`
  and reads particular bits of them; sdrtop cannot ask a stranger's
  headphones for anything, so it reads every bit whose two neighbours make
  it one of those bits (same either side, or opposite either side), which
  for this kind of modulation is the same measurement, and a test holds it
  to the suite's own figures. Each header is taken again from the raw
  samples and read at bit centres timed from the packet's own sync word.
  The receiver that finds the packets guesses where a bit's centre is,
  which is fine for finding packets and was not fine for measuring them:
  until this was built it read a perfectly healthy transmitter's
  `df2/df1` as about 0.45, which is how this whole section learned to
  check itself against a reference first. The suite's own recommended
  filter turned out to be the wrong one for traffic (it is built for a
  tester reading its own test patterns, and bends ordinary traffic by a
  few percent), so the reading uses a wider one. The price is the next
  channel: up to 20 dB below the transmitter being measured it moves the
  figures by less than 1 %, and a header with anything louder next door at
  the same moment is not read at all, just counted ("headers not read: the
  next channel was busy"). A weak signal makes `df2` scatter, one reading
  per bit at its centre carrying all the noise; the `±` beside it says how
  much, and at 20 dB above the noise it also leans a percent or so, which
  the `±` does not cover. Stronger is better, as with most things in
  radio.
- **Carrier**: each end's initial carrier (f0, from the four preamble bits)
  and its drift, read as the test suite defines them from the access code
  and header of every header measured. The drift and its rate are the worst
  header's, since the limits are on every packet; f0 is the mean, relative
  to our own oscillator like the clock below, and held against the
  specification's ±75 kHz only once a reference makes it absolute. The
  drift is held to the 40 kHz every packet type has to meet: a one-slot
  packet's 25 kHz is over its whole length, which a header alone cannot
  show. Every bit read here is either known (the whole access code) or
  decided again from the measurement itself, never taken from the part of
  the receiver that finds packets, because one wrong bit there looked like
  30 kHz of drift. Noise makes drift too, as it does on BLE: see the
  Modulation note in the Advertising view's detail.
- **Timing**: each end's timing on the piconet's 625 µs slot grid, fitted to
  every member's hits. Below eight hits it is collecting; hits that do not
  line up on a grid beyond chance are refused as one, never forced onto it.
  Given enough periods to try, a dozen points will line up with almost
  anything, and the panel would rather say "no grid" than find one it
  wanted to find. (The piconet's clock from the same grid is the whole
  piconet's, so it is on Classic 1.)

  Each end's **jitter** is its own packets' scatter about their own average
  timing, which is how 2.2.5 states it, held against its 1 µs. The grid
  belongs to neither end, so where each end sits on it means little alone;
  how far the slave's packets sit from the master's (`offset`) is the
  difference of the two averages, and that the grid cannot move. Under it,
  the residuals as a shape, the ±1 µs limits ruled, **stacked by who sent
  them**: the master's in the piconet's colour, the slave's in the ordinary
  ink, and the packets of unknown sender in the stale grey on top, never
  put on a side. One hump is one timing; a slave answering a little late on
  every slot stands as a hump of its own colour.

Under MODULATION the index and under CARRIER f0 are **plotted** over the
last minute: both ends on one time axis and one scale, each in its colour,
so whether the two read alike and whether either is moving shows at a
glance. Each point is the mean of a few packets (the title says how many),
because one header's reading is noisy and a line of single readings is a
band that shows the noise and hides the drift. The scale's ends are written
on the left: a line that fills the plot across `0.333` to `0.335` is a
transmitter holding steady, one across `0.30` to `0.34` is warming up. A
slowly moving f0 in the minutes after a device switches on is exactly that.

Narrower than three columns, the bench draws as many sections as fit, in
order, and names the rest ("+ TIMING on a wider panel"). Shorter than its
sections, a column shrinks its plots first (six rows, then three, then
none), keeps its readings whole, and says the plots are on a taller panel;
only then do readings give way from the bottom, and it says that too.

---

← [The NET section](net.md)
