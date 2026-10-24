# The pager · Radio › Pagers › POCSAG

← [User guide](README.md)

Before everyone carried a phone, people carried a pager: a small plastic box
that beeped, showed a number or a line of text, and asked nothing of you but
to call someone back. Hospitals, fire brigades and radio amateurs still use
them, and most of them speak **POCSAG**, the paging code of ITU-R M.584
Annex 1. This view is a pager. It shows what was sent, the way a pager
would, and then does what no pager ever did: it tells you how the message
got there.

The decoder is sdrtop's own, written from the Recommendation: no paging
library, no audio piped through another program. There is no sound at all,
as everywhere in sdrtop; a pager's beep is the NEW lamp.

## Before you listen

**Whether you may read pages depends on where you are.** In many countries
receiving a message not meant for you is fine and repeating it is not; in
some, even receiving is restricted. A hospital's pages are about patients.
sdrtop shows the text by default and `m` masks it everywhere at once (the
display, the list and the card), which is also what you want for a
screenshot.

**DAPNET**, the radio amateurs' own paging network, sends public pages on
439.9875 MHz in many places. It is a good first frequency: legal to listen
to, and somebody is usually sending the time.

## Getting there

Open the menu, choose the **Radio** tile, and go in: **Pagers › POCSAG ›
Pager**. Opening it retunes nothing. Press `f` and type the frequency, start
RX with `Space`, and wait. With nothing heard the display says
`SEARCHING` and the frequency it is listening on.

Any radio that reaches the frequency will do. The channel is the tuned
frequency itself; the radio's DC offset, which sits right there, is taken
off before the decoder sees anything. What the work costs is on the strip
as **LOAD**, a share of one core: about 7 % at 2.4 Msps on a modest laptop,
20 % at 10 Msps. A pager channel is 12.5 or 25 kHz wide, so the lowest sample
rate your radio offers is plenty, and the rest is spent on nothing.

## The face

A pager, drawn as one: the case, a dot-matrix display in its bezel, a red
NEW lamp, four buttons, a speaker grille that does nothing (like on the
real thing), and an engraved strip along the bottom.

**The display.** Sixteen characters by three lines at full size, fewer on a
smaller screen. Every cell shows its unlit dots, as a real character display
does. Above the text:

- **The signal bars:** the page's measured level, one bar for each 10 dB
  above -70 dBFS, all four from -30.
- **✉ N NEW:** pages not yet read.
- **The time, the function (A to D) and the kind** (NUMERIC, ALPHA or
  TONE) of the page shown.

Below the text: the address (**RIC**), which page of how many, the bits it
took repairing (**FIX**), codewords that could not be read (**LOST**), `CUT`
if the signal stopped mid-page, and which screen of a long page you are on.

A character that was lost on the air is drawn as a **checker**, never as a
letter: a hole in a message reads as a hole.

**The buttons** carry their keys under them: `←` `→` page through, `Enter`
reads on (a long page a screen at a time, and the page counts as read),
`m` masks. The rest are engraved below. All of them work the moment the
view is on screen, no focus needed, as a pager's do in the hand; the full
list is in [the keyboard guide](keys.md#general).

**The strip:**

- **SYNC:** locked, and at which rate, or searching.
- **LEVEL:** the page's level as a bar from -70 to -30 dBFS, and the figure.
- **LOAD:** what decoding costs, as above. Past 80 % it turns to a warning:
  the receiver is near falling behind, and a lower sample rate fixes it.
- **FEED LOSS:** the sample feed lost blocks in the last ten seconds. A
  lost block in the middle of a page cuts it there.
- **LINK:** the page's link in one line: the rate and its clock error, the
  shift, the decision SNR, the bits repaired. It is here so the physics stays
  on screen when the cards beside the face give way on a narrow terminal.

**The display's look** is yours: `b` switches between day (a lit glass,
dark text) and night (a dark glass, lit text), and `l` steps through its
colours: the theme's own first, then green, amber, blue, grey and red. Both
are kept in the config's `[pager]` block. A theme sets its own display with
the `pager_` and `lcd_` keys in [Themes](themes.md).

A stopped radio dims the display and marks the frame `[STALE]`: the last
page is still there to read, but nothing on the face pretends it is live.

## My pager

A real pager answers to one address. `u` makes the shown page's address
yours: only its pages are listed, the NEW lamp lights for its unread ones,
and the case says `my pager 0200001`. `u` again goes back to every address.
The frame says `[FILTERED]` while it is on, so a quiet list is not mistaken
for a quiet network.

## The inbox

Every page received this session, newest first: whether it is unread, its
number, the time, the address, the function, the kind, its length, the bits
repaired and its level, with the text under it (or its kind and length,
masked). The page on the display is the marked row, and the list scrolls to
keep it in view. `c` empties it; the count in the title of pages received
stays, because they were.

## This page

The card beside the list is the page on the display as it was received,
each reading against what it is measured by:

| Row | What it says |
|-----|--------------|
| **Rate** | 512, 1200 or 2400 baud, and how far the transmitter's clock is from it in ppm, by this radio's clock. Measured from the time of every bit transition, not from the receiver's own tracking |
| **Shift** | Half the distance between the two frequencies, against the standard's ±4.5 kHz |
| **Offset** | Where the carrier sat from the tuned frequency, worth what [the offset tag](net-trust.md#what-an-offset-is-worth) says |
| **Level** | The channel's level over the page, and the decision SNR: how far the bits stood from their own spread |
| **Preamble** | How many alternating bits came before the sync word, and how many of the sync word's 32 bits were wrong |
| **Codewords** | How many there were, how many bits were repaired, how many could not be read |
| **Polarity** | Which frequency carried a 1. Transmitters do it both ways, and both are read |
| **Address** | The 21-bit address, its function, and the frame it was sent in |
| **Content** | The kind and length, and what decided the kind (below) |
| **Ended** | At the idle codeword, at the next address, at two unreadable codewords, or cut |
| **Received** | When, on which frequency, over how many batches |

Under them, one codeword's 32 bits as corrected, the repaired ones lit.

## How a page is read

Three receivers listen at once, at 512, 1200 and 2400 baud, and whichever
finds a sync word reads what follows. A sync word is taken with up to two
wrong bits only right after a preamble; in noise alone it must be exact.
Noise read as a batch would pass the code's check about a quarter of the
time and print addresses nobody sent, which is the one kind of page this
view will not show you.

Each codeword carries a BCH(31,21) check and a parity bit: one or two wrong
bits are repaired (and counted), three are refused rather than "repaired"
into a different codeword. A refused codeword's characters are lost, and
two in a row end the page, as §2.5.1 asks.

**A transmission ends when its signal does.** Some transmitters send the
next sync word and then simply stop. Read on regardless, the noise after it
is sixteen more "codewords", and enough of them pass the check to print a
page or two from addresses nobody sent, a second after the real one. So
every word must stand on the two levels the transmission's own bits stood
on; noise has no levels to stand on, and the first word that does not ends
the transmission there. Nothing read after it reaches a page or the card.

**Numeric or text?** The Recommendation gives numeric pages function A and
text pages function D, and calls this "regarded as standard", which is
standards language for "usually". A pager knows its format from how it was
set up, not from those two bits, and senders do put text under A. The very
first page this view received off the air was "sdrtop is awesome", sent
under function A, and it came out as `372U 9][68382-37058[]`. Every bit was
right. So now: content under A that reads as text is shown as text, content
under D that is digits and not text is shown as digits, and the card says
the content overruled the function. Functions B and C name no format, and
there the content decides alone, or the card says both readings fit.

Numeric pages use the Recommendation's own table: digits, `U` for urgent,
space, hyphen, and two brackets drawn as `]` and `[` (several references say
`)` and `(`; the Recommendation draws square ones). Its spare code shows as
`*`.

## What it does not do

No transmitting, no audio, no FLEX. The alphanumeric set is the
international 7-bit one; national variants, where `[` is an `Ä`, are not
offered yet. A later Lab page will measure the signal itself: the eye
diagram, the shift per transmitter, the clock over time.
