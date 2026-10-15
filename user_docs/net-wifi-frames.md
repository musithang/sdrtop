# Frames · `Channel 2` *(focus `e`)*

← [The NET section](net.md)

**What is on this channel, frame by frame?** The [survey](net-wifi.md)
tells you how busy a channel is. This view sits on one channel and shows
you the traffic itself: every frame heard on the channel's centre, what
its signalling said, and how the frames follow one another. An access
point and the laptop it is talking to take turns sixteen microseconds
apart, and here you can watch them do it.

Lock the radio on a channel first, from the survey (`L`) or with `← →`.
Like every Wi-Fi view it needs exactly 20 Msps.

## The river

The top panel is the channel's time, drawn as lanes you read like lines of
text: left to right, then down. The newest frame ends the last lane.

- **Colour** is the frame's format, the same colours the survey uses.
- **Height** is its SNR, over a fixed 0 to 24 dB across the lane's two rows.
- **Width** is the airtime the frame said it would take.
- A frame that stated no length is drawn as its start, `▌`. That is how
  another channel's OFDM frame looks: the receiver can tell where it
  began, and nothing it said.
- `▒` is a frame of this channel whose signalling could not be read.

`z` steps the scale, and only `z` does: a river that rescaled itself with
the traffic would change every frame's width from one second to the next.

| Scale | What you see |
|-------|--------------|
| 1 ms a cell | The load: each cell shaded by how much of it was on the air, in quarters, in the colour of the format with most of it |
| 40 µs a cell | The exchanges: a data frame, the gap, its acknowledgement |
| 4 µs a cell | One frame's anatomy: its preamble drawn paler than the rest, and the gap after it |

The labels on the left count back from the newest frame shown, the same
zero the list uses. The frame selected in the list is lit, and the river
moves back to keep it in view. After a break in the stream (a lost block,
a retune) the clock starts again, so frames from before it are not placed
on the new one's time.

## The list

Under the river, every frame heard on this channel's centre, newest first.
Frames heard on other channels during the survey's visits are those
channels' business and are not listed here.

| Column | What it says |
|--------|--------------|
| when | How long before the newest frame shown |
| gap | From the end of the frame before (its stated airtime) to this one's start, in µs, and in ms once it passes one. `·` when the frame before stated no length, or a break came between them. A negative gap is an overlap: usually another channel's frame inside one of ours |
| format | The format its signalling named, or how far away another channel's frame was (`-5 MHz · OFDM`, `+10 MHz · 11b`) |
| rate, length | Only where a field read today states them. A VHT or HE frame's L-SIG always says 6 Mb/s, a placeholder that carries its airtime, so it is never shown as its rate |
| airtime | What the frame said it would take |
| offset | Its carrier against our tuning, in kHz with 1 sigma, relative to our own oscillator unless a [reference](net-trust.md) takes its error out |
| SNR | In dB, with the same height mark the river draws |

An HE or EHT frame adds its BSS colour and direction (`colour 23 ↑`) where
the width allows. An 802.11b frame's SNR comes from how closely its SYNC
matches the Barker code, the way a BLE packet's comes from its detector.

The list keeps the last 2048 frames, and its title says how many and how
far back they reach: seven seconds of a busy channel, a minute and a half
of a quiet one. A held list is a slice, not the whole story.

## The card

On the right, **frames a second**: this channel's frames in each whole
second back from the newest one shown, with its 1 sigma once three seconds
are in (fewer have no spread worth a number), and under a second, no rate
at all. The seconds have to be unbroken: frames lost with a dropped block
would be missing from the count, so after a break in the feed it starts
again, and the card says that is why. Frames from other channels are said beside it, never counted in
it. Under the number, the same frames split by format.

Below that, **the selected frame**, field by field: its preamble's offset
and SNR; its symbol clock beside its carrier over the same data, where the
frame was long enough to read one, and the session's check of the two
against each other; its L-SIG (marked as the airtime when it belongs to a
later format); HT-SIG, VHT-SIG-A, HE-SIG-A or U-SIG; an 802.11b frame's
header; and the channel across its 52 tones, its magnitude against its own
mean, on a window of at least 6 dB so a half-decibel ripple does not pass
for a notch. The MAC header has its line, saying it is not read yet.

`d` turns the lower part to the **decode account**: every preamble the
receiver found and what became of it, the Wi-Fi rows of
[decode health](net-band.md#decode-health).

## Keys

| Key | What it does |
|-----|--------------|
| `e` | Focus the list |
| `↓` | Hold the list at its newest frame and select further down |
| `↑` | Select back up (the list stays held) |
| `H` | Hold the list, or let it run |
| `End` | Back to live |
| `z` | The river's scale: 1 ms, 40 µs or 4 µs a cell |
| `d` | The card: the selected frame, or the decode account |
| `Enter` | The selected frame in place of the list, where the card has no room |

Held, the list and the river stop at the same moment and both say
PAUSED. `Esc` leaves the focus and lets go of the selection; the hold, the
scale and the card's face stay as you left them.

## When there is nothing to show

- **Listening**: nothing heard on this channel yet. The lanes stay live
  and empty; give it a few seconds before calling the channel quiet.
- **Only other channels**: every frame heard came from a channel 5, 10 or
  15 MHz away. They are drawn and listed as what little was read of them.
- **Not measured**: the radio is idle, at the wrong rate, or tuned off a
  channel centre. The river says why, and which key helps.

## On a narrow screen

Under 140 columns the card gives way and the river and the list take the
width. The frames a second ride in the river's title instead, and `Enter`
opens the selected frame in the list's place (`d` works there too).
`Enter` again, or `End`, brings the list back.
