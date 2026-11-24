# The rules

> **An instrument that cannot be questioned is not evidence.**
> **It is a rumour with a power supply.**

sdrtop does not ask to be trusted. It asks to be checked. Those are opposite
requests, and only one of them is ever honoured.

These are the ten rules sdrtop is built on, and Monas and monasd with it. They
are not a mission statement. They constrain the code, and several parts of it
exist only to enforce them. When a reading, a panel or an API response is in
question, these decide.

Wherever a rule says *the screen*, it means every place a number leaves the
engine: sdrtop's terminal, monasd's API, an exported file. A number does not
get to be more honest in one of them than in another.

---

### ONE

> **A radio is supported after it has been held.**

A radio gets a native backend after it has been on this desk. Powered. Tuned to
something genuinely transmitting. Watched by a person who was in a position to
notice something subtly wrong and permitted, at that moment, to be disappointed.
Not after the datasheet was read closely. After it was **held**.

A datasheet cannot be asked a follow-up question, and everything that has ever
mattered was in the follow-up question. This rule is expensive, and the expense
is why the supported list grows only as fast as radios arrive on the bench.
Every radio says which it is, verified or reasoned, on its badge and in the
API's `verified` field. The one place this rule bends is
[the marked exception](#the-marked-exception).

### TWO

> **What cannot be asked is refused, never invented.**

Where sdrtop cannot work something out, that reading says so and stops. No
plausible default. No interpolation from a similar device. No last known value
carried quietly forward.

The same holds wherever the number goes. Over the API a reading the screen would
dash arrives as the reason it is missing, in words, and never as a zero a script
could mistake for a measurement.

A missing number is an inconvenience. A crash is embarrassing and completely
honest. A wrong number that looks right is neither, and it is the only thing
these rules are trying to prevent.

### THREE

> **A payload is not testimony.**

Printing `51 bytes` is a courier's job. The question is *why 51*: at what
spreading factor, across what bandwidth, how far above the noise, how much of the
margin the preamble spent, how many symbols were repaired on the way in, and
whether the decoder was confident or merely lucky.

A signal is a physical event, and the payload is very nearly the least
interesting thing about it. Every digital demodulator on the roadmap is held to
this or it does not ship.

The API is held to the same line, and more strictly, because a program on a
server is a more tempting sniffer than one on a desk. It serves what the screen
shows and nothing more: a pager's text only as the pager view shows it,
addresses only as the address mode shows them, and never the samples. A
recording stays on the machine that made it.

### FOUR

> **Every reading is dated.**

Every lab panel marks itself **[STALE]** the moment RX stops. A number on a
screen is a claim about *now*, and the instant that stops being true while the
number stays put, the screen is lying in the most persuasive way available to any
interface: by not changing.

Over the API there is no grey to cool a number with, so every group of readings
carries the time it was taken and whether it has gone stale, and an hour the
radio was missing is sent as an outage with its length, not as an hour of
silence.

### FIVE

> **One quantity, one account.**

No reading may say two different things in two places. One scale for one
quantity, everywhere it appears, and every label names what was actually
measured so the label and the number cannot drift apart.

The terminal and the API are two places reading one account: what each number
is, or why it is missing, is decided once in the engine, and a test holds every
panel that shows one to the answer the API gives.

### SIX

> **The docs are checked against the code, never against each other.**

Documentation is held to these rules exactly as the code is. Each fact lives in
one place and the other pages point at it. The same thing explained two ways in
two files is not thoroughness, it is two pages that have started drifting and
have not been caught. Where the docs are unsure they say so in the same font as
everything else.

### SEVEN

> **A safeguard that cannot fail is an ornament.**

If a check cannot be made to fail on demand, it is not a check. A reference
generated from the code is trusted; a reference typed out by hand is not, however
carefully it was typed. That is why the man pages, the help text and the API
reference are written from the code. A verification step whose failure path
continues anyway is worse than none at all, because now there is a tick mark in
front of the thing nobody checked.

### EIGHT

> **What there is no instrument for is declined.**

There is no audio. There is no transmit, on hardware perfectly capable of it.
The terminal handles no mouse input. The dBm figures on the RF bench are modelled
and relative and the panel says so on its own face.

Every one of those is a decision, and a decision is not a missing feature. What
another program draws on top of the API is that program's business: these rules
bind the numbers it is given, not the pointer it moves. An honest estimate,
labelled an estimate, is a measurement. The same number with the label removed
is perjury with better typesetting.

### NINE

> **The second opinion has to come from somebody who does not care how it comes
> out.**

An instrument that refuses to invent will visibly not know things, all over its
own interface, permanently. Those gaps are not failures to paper over. They are
the invitation, and the most valuable ones sit in front of radios nobody here has
ever held.

### TEN

> **A record nobody finishes has recorded nothing.**

Accuracy is necessary and has never once been sufficient: a manual nobody reads
to the end has measured nothing for anybody. So the docs are allowed a voice, on
one absolute condition: **the joke lives in the frame and never touches the
reading.** Be as funny as you like about the history, the desk or the author.
Never about what a number means.

The gradient runs the other way as the reader nears the machinery.
`troubleshooting.md` is close to humourless on purpose, because nobody arrives
there in a good mood. Plain is not the same as dry, and not one page here is
permitted to be dry.

---

## The marked exception

Rule One is not satisfied for any radio reached through **SoapySDR**: LimeSDR,
bladeRF, USRP, the PlutoSDR boards and SDRplay RSPs that are not on this desk,
and whatever sits at the far end of SoapyRemote. None has been here. The backend
was written from the API headers.

It is admitted anyway, with an instruction that does not get quieter further
down the page:

**Every claim sdrtop makes about a radio it has never held is reasoned, not
verified. Those are different words for different things, and this project will
not let them touch.**

The conditions, all already met:

- **No device table exists in the source.** Frequency range, sample rates, gain
  range, AGC, baseband filter, sample format and how many of its bits carry
  meaning: every one is asked of the driver at runtime.
- **Rule Two is untouched.** Where the driver cannot answer, the reading is
  absent. The RF bench's noise figure and linearity card does not appear at all
  on a SoapySDR device, because SoapySDR does not publish a front end. Inventing
  it would have taken an evening and would have been the worst thing in this
  program.
- **Another party's claim is entered as another party's claim.** `SoapyHackRF`
  announces 0 to 7.25 GHz where the datasheet says 1 MHz to 6 GHz. sdrtop repeats
  the driver's number and the log names the driver that said it. It does not
  silently correct what it cannot verify, and it does not launder it either.
- **The marking is everywhere the claim is.** On the badge, in the status block,
  in the API, and in a hardware table that is not flattering and is not going to
  be improved.

Every one of those radios stays grey until somebody who owns one stands it on a
desk and files a report. Then that one, and only that one, is entered properly.
That is how the Airspys, the RSP1B and the PlutoSky went from grey to green.

**In my own words.** I broke my own rule here, deliberately, in one place, and
then spent considerably longer making sure it could not spread than I spent
writing the backend. Obeyed to the letter, Rule One would have kept this
instrument small, correct and unused, and been right about it the entire time. A
rule that guarantees its own irrelevance has stopped protecting anything. I'd
rather have a rule with one marked scar on it than one nobody has ever had a
reason to test: the grey badge is the first evidence that the green one means
anything.

If you own one of those radios, a report saying "it works, here is what it
showed" is worth as much to me as a crash report, and arguably more, because a
crash tells me where it broke and a working report tells me where it held.

---

If this program ever tells you something it does not actually know, that is not a
bug report. That is perjury, and it outranks every open issue and every feature
anybody is waiting for. Say so, loudly. You will be believed.

*The dBFS numbers are real. The bugs are mine. Check both.*

```
                        73 de sdrtop 📻
```
