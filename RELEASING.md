# 🕯️ THE RELEASE RITUAL 🕯️

```
                            .  *  .   .
                         .    \ | /    .
                      .   --== 📡 ==--   .
                         .    / | \    .
                            .  *  .   .

              here lie the instructions for summoning
                   a version into the material plane
              written in blood, mostly my own, at 23:40
```

*A Practical Grimoire for the Publishing of sdrtop, compiled in the smoking
aftermath of the Incident of the Two Evenings, so that it may never, ever
happen again.*

---

## PREFACE, in which we are honest about how we got here

There was a `.deb` matrix.

There was an **architecture matrix**. There was **QEMU**, emulating an armhf
machine, slowly, so that a `.deb` could be built for a computer nobody had, and
then *tested* on that same emulated computer, even more slowly, so that we could
be confident the package we did not need worked on the hardware we did not have.

There was, briefly, a conversation about a `.dmg`. For a terminal application.
On Linux. Let us move on.

Then came the reckoning: a **15.8 MB demo video**, sitting quietly in
`user_docs/pics/`, single-handedly making it impossible to publish to crates.io,
which caps a crate at 10 MB. The tarball had been 18 MB for months. Nobody had
looked.

Then docs.rs, asked politely for documentation, replied:

> **sdrtop-0.4.1 is not a library.**

Correct. Devastating. No notes.

This document is what grew out of that. Follow it in order. The machines handle
the parts that can hurt you, and there are more of those than you would like.

---

## ⚠️ THE TWO LAWS ⚠️

Learn them, or be taught them. The teaching is worse.

### THE FIRST LAW: crates.io remembers everything and forgives nothing

A published version may be **yanked** but never **replaced**, and its number is
never given back to you. Not "should not be reused". *Cannot.* There is no
button. There is no undo. There is no kindly maintainer at the other end of a
ticket who will make an exception, because there is no exception to make. The
number is spent. It is gone. It is on the internet with your name on it.

The publishing step is automated, and it runs **before** anything appears on
GitHub. Every single check capable of saving you therefore happens earlier than
it does. This is not an accident of ordering. This is the whole architecture,
and it exists because the alternative is a beautiful release page pointing at a
version that never published, which is somehow worse than no page at all.

### THE SECOND LAW: the release is born a draft

Nothing is visible to a living soul until you have read it with your own eyes
and pressed the button yourself.

This is your final moment of authority over the artefact. After this you are
merely the person who wrote it.

---

## PROVISIONS FOR THE JOURNEY

Versioning is SemVer. While the major is `0`, a minor bump is *permitted* to
break `~/.config/sdrtop/config.toml`, and when it does, `CHANGELOG.md` confesses
under *Changed*, because we are not animals.

**Steps 8 and 10 require the [GitHub CLI](https://cli.github.com).** It is not a
build dependency, CI brings its own, and your machine is quite possibly entirely
innocent of it:

```sh
gh --version || echo "install it: https://cli.github.com"
gh auth status
```

Everything through step 7 needs only `git` and `cargo`, as the ancients
intended, before YAML.

---

# THE RITE

## I. Inscribe the changelog 📜

In `CHANGELOG.md`, move everything beneath `## [Unreleased]` into a fresh
`## [X.Y.Z] - YYYY-MM-DD`, leave `## [Unreleased]` empty above it, and add the
version to the compare links at the foot of the file.

This text **becomes the release body**. Write it for a stranger who has not read
your commits and has no intention of ever starting.

`refactor stuff` is not a release note. `refactor stuff` is a distress signal.

Behold what you have wrought:

```sh
packaging/release-notes.sh X.Y.Z
```

Byte for byte, that is what the world will read. If it exits non-zero, CI will
refuse the release later anyway, so find out now, alone, with your dignity
still on.

## II. Tell the story 📖

Add a checkpoint to `user_docs/whats-new.md` and move the `*(you are here)*`
marker onto it.

The changelog is the receipt. This is the *story*. People genuinely read this
one, which is a tremendous responsibility and also secretly the nicest part.

## III. Speak the new number 🔢

```sh
# edit Cargo.toml: version = "X.Y.Z"
cargo check --offline          # updates Cargo.lock to match
packaging/version.sh           # must print X.Y.Z, and nothing else
```

`Cargo.lock` is not decorative. CI runs `--locked` in every job, and a lockfile
still whispering the previous version fails the build in a way that looks
profoundly mysterious for about ten minutes and then extremely stupid forever.

> *There used to be **two** answers to "what version is this". One read
> `Cargo.toml` with `sed`. The other started an entire container to run
> `cargo metadata` and then applied a regular expression to the resulting JSON,
> keeping the first `"version"` field it happened to see. They agreed, right up
> until the day they would not have. Now there is `version.sh`, and there is
> only ever one answer.*

## IV. Await the omens 🔮

Push to `main` (**not the tag**, we are nowhere near ready for the tag) and let
CI speak: formatting, clippy, the test suite, `cargo package`, the 1.88 MSRV
check, shellcheck.

Green means green. "Mostly green" is not a colour. It is a mood, and it is
lying to you.

## V. COMMUNE WITH THE ACTUAL RADIO 📻

**No machine can do this for you. It is the law of this project: hardware
support lands only after physical testing.**

Both radios. A HackRF One **and** an RTL-SDR. Yes, both. You own both. They are
on the desk. They are *right there*.

```sh
cargo build --release
./target/release/sdrtop --config /tmp/sdrtop-release-check.toml
```

Pass `--config` unless you actively *want* your release rehearsal to silently
overwrite your real config on quit, in which case enjoy rebuilding all your
markers from memory.

Press `Space`. Tune to something you *know* is transmitting. Look at the presets
you touched.

Every other step in this document is a computer inspecting a computer. This one
is a human being, looking at a screen, deciding whether the thing is any good.
It is the only step that cannot be faked and the only one that has ever caught
anything interesting.

## VI. THE SUMMONING 🏷️

```sh
git add Cargo.toml Cargo.lock CHANGELOG.md user_docs/whats-new.md
git commit -m "chore(release): X.Y.Z"
git tag -a vX.Y.Z -m "sdrtop X.Y.Z"
git push origin main --follow-tags
```

**That tag is the point of no return.**

Everything before it can be undone with a shrug and a `git tag -d`. After it,
machines begin acting on your behalf, and one of them is about to do something
permanent while you watch a progress spinner.

## VII. Observe the machines at their work ⚙️

`.github/workflows/release.yaml`. Three jobs, in this order, for this reason:

| Job | Deed | Reversible |
|---|---|---|
| `build` | tarball in the Debian 12 container, tag matched against `Cargo.toml`, changelog section confirmed to exist, glibc floor asserted, exact shared-library set asserted, provenance attestation signed | yes |
| `publish` | `cargo publish` to crates.io via Trusted Publishing | **no. never. under no circumstances whatsoever** |
| `release` | drafts the GitHub release with your changelog as its body | yes |

The irreversible act sits in the **middle**: guarded by everything `build`
checks, followed only by a step that is safe to retry until the heat death of
the universe.

If `build` fails, breathe out. Nothing has happened. Proceed to the Bestiary.

## VIII. Interrogate the draft before the public may 🔍

Draft assets are downloadable to those with write access, which is precisely why
this project needs no prerelease. Should `gh` decline, the draft's own page in
the browser holds the same files.

```sh
gh release download vX.Y.Z --dir /tmp/rel
cd /tmp/rel && tar -xzf sdrtop-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz
sh sdrtop-X.Y.Z-x86_64-unknown-linux-gnu/install.sh --prefix ~/.cache/rel-test
```

That was the offline path. Now the other two, which need the crates.io version
to be live, and by this point it is *aggressively* live:

```sh
sh packaging/install.sh --prefix ~/.cache/rel-test --from-source
sh packaging/install.sh --prefix ~/.cache/rel-test --git
sh packaging/install.sh --prefix ~/.cache/rel-test --uninstall
```

And demand that the attestation prove itself:

```sh
gh attestation verify sdrtop-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz --repo musithang/sdrtop
```

> *The checksum check in `install.sh` once lived inside an `if` whose every
> branch continued. A failed `SHA256SUMS` download, or a machine without
> `sha256sum`, meant the installer skipped verification entirely and said
> absolutely nothing about it. It looked like a check. It read like a check. It
> was a decoration. It now stops, loudly, and `--no-verify` exists for people
> who have a reason and know they have one.*

## IX. READ IT. With your eyes. 👁️

Open the draft. Read it as a stranger who has never heard of you, does not care
about your refactor, and will judge the entire project by this one page.

The title. The notes. The asset names. The typo in the third bullet that you
will otherwise find in eleven minutes.

*Then* press Publish.

## X. Confirm the world has accepted your offering 🌍

```sh
cargo install sdrtop --locked --force
sdrtop --version        # X.Y.Z and a commit hash. Both. Look at them.
```

Go outside. You have earned weather. 🎉

---

# 🐉 THE BESTIARY OF DISASTERS

*Creatures you may encounter, and the banishment of each.*

## 🟢 The Harmless One: `build` failed

Nothing is spent. The tag is a label, and labels come off:

```sh
git tag -d vX.Y.Z
git push --delete origin vX.Y.Z
# fix it, commit, tag again, push again
```

Nobody saw. It never happened. We do not speak of it again.

## 🔴 The Permanent One: `publish` succeeded and it should not have

The version number is gone. It belongs to crates.io now. It is not coming back,
and it is not coming back *especially* not because you asked nicely.

Do **not** attempt to reuse it. Bump the patch, write a changelog entry stating
plainly what was wrong, and go round again. Shipping 0.4.3 twenty minutes after
0.4.2 is not shameful. Every project you have ever admired has a version history
with a suspiciously rapid patch release in it, and behind every one of those is
a person who felt exactly how you feel right now.

```sh
cargo yank --version X.Y.Z
```

Yanking prevents *new* projects from selecting the broken version. It does not
delete it. It does not free the number. Anyone who already has it in a lockfile
keeps it forever. It is a warning sign nailed to a door, not an undo.

## 🟡 The Cosmetic One: `release` failed but `publish` succeeded

Be calm. Only the draft is missing. The crate is out there, healthy, being
downloaded by strangers who have no idea any of this is happening.

Re-run the job from the Actions page. It is idempotent, and it flatly refuses to
overwrite the assets of an already-published release, a refusal written by
somebody who had thought about it for an uncomfortably long time.

## 🟣 The Cryptic One: the token exchange failed in `publish`

Trusted Publishing on crates.io names this repository **and the workflow
filename**. Rename `release.yaml`, or add an `environment:` key to the `publish`
job, and the whole thing breaks until you edit the crates.io side to match.

It is `release.yaml`. **With an `a`.** Every example on the internet says
`.yml`. This one has eaten entire evenings belonging to better people than us
and it will happily eat yours.

## 🔵 The Ubuntu One: "it works on my machine, but not on my other machine"

Debian ships the rtl-sdr runtime as **`librtlsdr0`**. Ubuntu packages the
*identical upstream source* as **`librtlsdr2`**, soname `.so.2`. Kali and
Raspberry Pi OS follow Debian. Mint follows Ubuntu. Nobody is wrong. Everybody
is incompatible.

This is why exactly one prebuilt tarball is published, why `install.sh` decides
whether to use it by **running the binary** instead of reading your distribution
off a list, and why the fallback to `cargo install` is the design rather than an
error path.

It was also got wrong once, from memory, in a comment. The fix was to go and
read `packages.debian.org` and `packages.ubuntu.com` like an adult. Do that
before editing any package name in `install.sh`.

---

# 🎭 REHEARSAL WITHOUT CONSEQUENCE

`workflow_dispatch` on the release workflow builds the tarball, runs every
check, and produces a genuine attestation, while skipping both `publish` and
`release` entirely. Nothing escapes into the world. Use it after touching
anything under `packaging/` or the workflow itself.

Locally, `packaging/build-tarball.sh` performs the container build alone and
runs the same version and smoke assertions, with no internet ceremony
whatsoever.

> *The container image used to be rebuilt only if it did not already exist. Edit
> the `Containerfile`, run the build on a machine that had an image, and you got
> a tarball made by the **old recipe** with nothing on screen to tell you. CI
> never noticed, because a fresh runner has no image to skip. It now rebuilds
> every time and the layer cache makes that free.*

---

# 🤖 APPENDIX: automating step III, should you weaken

[`cargo-release`](https://github.com/crate-ci/cargo-release) will perform the
bump, the lockfile, the changelog heading, the commit, the tag and the push in a
single incantation.

It is deliberately **not** installed or configured here. The rite above requires
only `git` and `cargo`. A release process that itself depends on a tool you must
remember to install is a release process with a trapdoor in the floor, and we
have quite enough of those already.

But if you insist:

```sh
cargo install cargo-release
```

and add to `Cargo.toml`:

```toml
[package.metadata.release]
tag-name = "v{{version}}"
pre-release-replacements = [
  { file = "CHANGELOG.md", search = "## \\[Unreleased\\]", replace = "## [Unreleased]\n\n## [{{version}}] - {{date}}" },
]
```

Steps III and VI then collapse into `cargo release X.Y.Z --execute`.

Steps I, II, V, VIII and IX still demand a human with functioning eyes and an
actual radio on the desk.

That is not an oversight. That is the job.

```
                        73 de sdrtop 📻
```
