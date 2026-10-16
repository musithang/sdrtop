# Frames · `Channel 2` *(focus `e`)*

← [The NET section](net.md)

**What is on this channel, frame by frame?** The [survey](net-wifi.md)
tells you how busy a channel is. This view sits on one channel and shows
you the traffic itself: the frames, what their signalling said, and how
they follow one another. An access point and the laptop it is talking to
take turns sixteen microseconds apart, and here you can watch them do it.

Lock the radio on a channel first, from the survey (`L`) or with `← →`.
Like every Wi-Fi view it needs exactly 20 Msps.

## Captures, not a stream

A busy channel carries a few hundred frames a second. Scrolled past as
they came, that is a list nobody can read. So the view works like an
oscilloscope: it takes **captures**. A capture is a window of air, as long
as the river shows at its scale, and everything heard in it, frozen. It
stays on screen until the next one, which comes after the **holdoff**
(1 s, give or take a tenth: an exact second is ten beacon intervals, and
every capture would land on a beacon, the way a scope's trigger locks onto
a periodic signal).

The **trigger** decides what starts a capture: any frame, or one kind (a
format, 802.11b, another channel's, an unreadable one). The frame that set
it off sits a tenth of the way in, so you see what led up to it, and it is
**time zero**: everything before it counts down to it, everything after
counts on. A capture is shown only once its whole window has been heard,
so half of one never passes for a quiet channel.

The numbers do not stop for any of this. The card's frames a second is
measured over everything heard, not over the capture you happen to be
looking at.

## The river

The top panel is the capture, drawn as lanes you read like lines of text:
left to right, then down.

- **Colour** is the frame's format, the same colours the survey uses.
- **Height** is its SNR, over a fixed 0 to 24 dB across the lane's two rows.
- **Width** is the airtime the frame said it would take.
- A frame that stated no length is drawn as its start, `▌`. That is how
  another channel's OFDM frame looks: the receiver can tell where it
  began, and nothing it said.
- `▒` is a frame of this channel whose signalling could not be read.
- `╱` is a span the feed lost (a dropped block). The stream's clock ran
  on through it, so the frames either side are in their right places, but
  nobody heard what was there: hatched, so it does not pass for quiet air.

`z` steps the scale, and only `z` does: a river that rescaled itself with
the traffic would change every frame's width from one second to the next.

| Scale | One capture | What you see |
|-------|-------------|--------------|
| 1 ms a cell | about 440 ms | The load: each cell shaded by how much of it was on the air, in quarters, in the colour of the format with most of it |
| 40 µs a cell | about 18 ms | The exchanges: a data frame, the gap, its acknowledgement |
| 4 µs a cell | about 1.8 ms | One frame's anatomy: its preamble drawn paler than the rest, and the gap after it |

(The lengths are for a 191-column terminal; a capture is always as long as
the river you have.) The labels on the left count from the trigger. The
frame selected in the list stands up off the river like a raised key:
drawn in the focus colour on a lighter ground, with a darker edge either
side of it and a `▸` at its lane, so it stands out even where
a theme's focus colour is close to a format's.
The title says which capture this is,
its scale, the trigger, the holdoff, and `STOP` when you have stopped it.

## The list

Under the river, the capture's frames **in time order**, oldest first, the
way the river reads.

| Column | What it says |
|--------|--------------|
| when | From the trigger: `+7.420 ms`, or `-1.204 ms` for a frame before it |
| gap | From the end of the frame before (its stated airtime) to this one's start, in µs, and in ms once it passes one. `·` when the frame before stated no length, or a break came between them. A negative gap is an overlap: usually another channel's frame inside one of ours |
| format | The format its signalling named, or how far away another channel's frame was (`-5 MHz · OFDM`, `+10 MHz · 11b`) |
| rate, length | Only where a field read today states them. A VHT or HE frame's L-SIG always says 6 Mb/s, a placeholder that carries its airtime, so it is never shown as its rate |
| airtime | What the frame said it would take |
| offset | Its carrier against the centre of the channel it was sent on, in kHz with 1 sigma, relative to our own oscillator unless a [reference](net-trust.md) takes its error out |
| SNR | In dB, with the same height mark the river draws |

The frame that set the capture off is marked `◂ trigger`. An HE or EHT
frame adds its BSS colour and direction (`colour 23 ↑`) where the width
allows. An 802.11b frame's SNR comes from how closely its SYNC matches the
Barker code, the way a BLE packet's comes from its detector. A span the
feed lost is a line of its own, where it fell: `3.2 ms not received`.

Before the first capture the list says what it is waiting for (`armed:
waiting for 802.11b`).

## The card

On the right, **frames a second**: this channel's frames in each whole
second back from the newest one heard, with its 1 sigma once three seconds
are in (fewer have no spread worth a number), and under a second, no rate
at all. The seconds have to be unbroken: frames lost with a dropped block
would be missing from the count, so after a break in the feed it starts
again, and the card says that is why. Frames from other channels are said
beside it, never counted in it. Under the number, the same frames split
by format.

Below that, **the selected frame**, field by field, its time from the
trigger in the rule above it: its preamble's offset and SNR; its symbol
clock beside its carrier over the same data, where the frame was long
enough to read one, and the session's check of the two against each other;
its L-SIG (marked as the airtime when it belongs to a later format);
HT-SIG, VHT-SIG-A, HE-SIG-A or U-SIG; an 802.11b frame's header; and the
channel across its 52 tones, its magnitude against its own mean, on a
window of at least 6 dB so a half-decibel ripple does not pass for a
notch. The MAC header has its line, saying it is not read yet.

`d` turns the lower part to the **decode account**: every preamble the
receiver found and what became of it, the Wi-Fi rows of
[decode health](net-band.md#decode-health).

## Keys

| Key | What it does |
|-----|--------------|
| `e` | Focus the list |
| `↑` `↓` | Select a frame of the capture, and stop there (Single), so it does not change under you |
| `H` | Stop the captures, or run them again |
| `End` | Run again, starting on the trigger frame |
| `z` | The river's scale: 1 ms, 40 µs or 4 µs a cell |
| `t` | The trigger: any frame, a format, another channel's, unreadable |
| `+` `-` | The holdoff: 0.5, 1, 2 or 5 s |
| `d` | The card: the selected frame, or the decode account |
| `Enter` | The selected frame in place of the list, where the card has no room |

Stopped, a capture stays as long as you like: it is a copy, so nothing
heard since can push it out. `Esc` leaves the focus and the selection goes
back to the trigger frame; Single, the scale, the trigger and the card's
face stay as you left them.

## When there is nothing to show

- **Not locked on a channel**: the survey is walking the band, and a view
  of one channel's frames needs the radio to stay put. Press `m` to lock
  where it is and `← →` to step to the channel you want, or lock one on the
  Survey (`L`).
- **Listening**: nothing heard on this channel yet. The lanes stay empty;
  give it a few seconds before calling the channel quiet.
- **Only other channels**: every frame heard came from a channel 5, 10 or
  15 MHz away. They are captured and listed as what little was read of
  them.
- **Not measured**: the radio is idle, at the wrong rate, or tuned off a
  channel centre. The river says why, and which key helps.

## On a narrow screen

Under 140 columns the card gives way and the river and the list take the
width. The frames a second ride in the river's title instead (the trigger
and the holdoff give way there; the list's keys still name them), and
`Enter` opens the selected frame in the list's place (`d` works there
too). `Enter` again, or `End`, brings the list back.
