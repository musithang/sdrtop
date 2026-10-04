# Contributing to sdrtop

Thank you for wanting to help. Some of the best things in sdrtop came from people
with a radio I don't own, a bug I had walked past, or a fix I didn't know was
needed: the RTL-SDR backend, the tinySA, a SoapyRemote radio that listed fine and
would not open.

This page is the short version of getting a change in: a few rules, and a
checklist. The rules sdrtop is built on are
[the ten in POLICY.md](POLICY.md#part-two-the-rules), one sentence each, and
where a rule here leans on one of them, it links to it.

(The rest of POLICY.md is the story of how those ten came about. It is set on
another planet. You don't need it to contribute, but it's there if you enjoy that
sort of thing.)

Everyone here follows the [Code of Conduct](CODE_OF_CONDUCT.md).

---

## Ask first, for anything big

**Small things, just send them.** A bug fix, a typo, a wrong sentence in the
docs, a report from your radio: open the pull request or the issue, no need to
ask.

**Anything bigger, open an issue first.** A new panel, a decoder, a device
backend, a new dependency, or anything that changes what a measurement means. Say
what you want to build and roughly how, and wait until we have agreed on it
before you start.

This isn't gatekeeping. The worst outcome for both of us is you spending a week
on something good that I then can't merge. Ten minutes in an issue prevents
that, and I would much rather say "yes, and here's how" at the start than
"sorry" at the end.

---

## The rules

### 1. sdrtop decodes signals itself

Every part of the signal chain is written in this repository: filters,
oscillators, demodulators, symbol timing, CRCs, protocol decoders. Two crates are
the only exceptions, and both are named in
[the honest ledger](user_docs/demodulator.md#the-honest-ledger):

- `rustfft`, for the forward FFT
- `num-complex`, for the complex number type

No other crate does signal processing or decoding here. Not in the app, and not
in the tests either. A reference to test against is written from the
specification itself, the way the Bluetooth receiver is held to a reference
transmitter and tester built from the Bluetooth SIG's own test documents.

This is the rule that matters most, and the easiest one to break with good
intentions. sdrtop is a measuring instrument. When one of its readings disagrees
with your bench equipment, the answer has to be a paragraph about what our code
does, not a link to somebody else's issue tracker. A library can be excellent and
still not be ours. It makes sdrtop slower to build, and that is the price.

### 2. A new dependency needs an issue first

Any new crate, in `[dependencies]`, `[dev-dependencies]` or
`[build-dependencies]`, is agreed in an issue before the pull request. The answer
is usually no, and yes only when there is genuinely no other way to do the job. A
crate that does signal processing or decoding is already answered by rule 1.

### 3. Never invent a number

If sdrtop cannot measure something or ask the device for it, the screen says so
and stops. No plausible default, no zero standing in for "unknown", no estimate
wearing a measurement's label. A field the data does not carry is blank, and
every label names what was actually measured. A plausible number is the worst
thing a measurement tool can produce. ([Rule Two](POLICY.md#two))

### 4. Say what you tested, and on what

A claim about hardware needs the hardware. Write down the radio, its firmware,
the host, the sample rate and what you did. Code written from a datasheet or a
specification and never run on a real device is welcome, as long as it says so,
on screen and in the docs. Reasoned and verified are different words, and sdrtop
keeps them apart. ([Rule One](POLICY.md#one), and
[the marked exception](POLICY.md#the-marked-exception) for how SoapySDR does it)

### 5. The docs change with the code

If someone using sdrtop can see the change, the same pull request updates
[`user_docs/`](user_docs/README.md) and adds a line under `## [Unreleased]` in
[CHANGELOG.md](CHANGELOG.md). Each fact lives on one page and the others link to
it, so edit the page that owns the topic instead of explaining it a second time
somewhere else. ([Rule Six](POLICY.md#six))

You don't have to write like the rest of the docs. Clear and correct is enough,
and I'll tune the wording before a release.

Leave `user_docs/whats-new.md`, the version number and the release files to me.
They belong to [the release](RELEASING.md).

### 6. What sdrtop does not do

No audio, no transmit, no mouse input. These are decisions, not missing
features, and pull requests adding them will be declined.
([Rule Eight](POLICY.md#eight))

### 7. AI help is fine, if you say so

Use whatever tools help you. This project is built with them too
([CREDITS.md](CREDITS.md)). Say so in the pull request, and make sure you
understand and have run every line you send. In review, you are the one
answering for it.

---

## Before you open the pull request

Run what CI runs. All four should pass:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all --locked
cargo +1.88 check --locked --all-targets   # the oldest Rust sdrtop supports
```

And check these by hand:

- [ ] New `.rs` files start with the line every other file has:
      `// SPDX-License-Identifier: GPL-3.0-or-later`
- [ ] Commits look like the rest of the history: `type(scope): what it does`,
      lowercase, for example
      `fix(soapy): keep the remote address in the open markup`. The body says why.
- [ ] After bringing `main` into your branch, no conflict marker is left
      behind: `git grep -n -E '^(<<<<<<<|>>>>>>>)( |$)|^=======$'` prints
      nothing. CI fails on one, wherever it is.
- [ ] One topic per pull request. A big change goes in as a few smaller ones,
      each working on its own.
- [ ] The description says what changed, why, and how you tested it.
- [ ] No new dependency, or the issue where we agreed on it is linked.
- [ ] Docs and changelog are updated (rule 5).
- [ ] AI assistance is mentioned, if you used it (rule 7).

---

## Bugs, and reports from your radio

An issue is a contribution too. For a bug,
[Getting help](user_docs/troubleshooting.md#getting-help) lists what to include.
The log file matters most.

If you run sdrtop on a radio nobody here owns, please tell me how it went,
working or not. "It works, here is what it reported" is as useful to me as a
crash, and it is how a grey row in the hardware table turns green.

Found a security problem? Not in a public issue, please. See
[SECURITY.md](SECURITY.md).

---

## What happens next

I review every pull request myself, in my spare time, so give it a few days. If
something can't be merged as it is, I'll say why and which parts can be kept.
Everyone whose work is merged is named in [CREDITS.md](CREDITS.md).

## License

sdrtop is [GPL-3.0-or-later](LICENSE), and anything you contribute is under the
same license.

Thank you. 📻
