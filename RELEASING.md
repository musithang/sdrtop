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

And there are several of them now: `monas`, the engine, and the `monas-*`
crates it is made of; `sdrtop`, the terminal; `monasd`, the engine with no
screen. One version number, every crate, a spent number for each of them every
time. Once the Rite of the First Offering below has been
performed, each is set to **Trusted Publishing only** on crates.io, so the only
hand that can publish one is the workflow's, and a token that leaks out of
somebody's shell history publishes nothing at all.

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

**Steps 7 and 9 require the [GitHub CLI](https://cli.github.com).** It is not a
build dependency, CI brings its own, and your machine is quite possibly entirely
innocent of it:

```sh
gh --version || echo "install it: https://cli.github.com"
gh auth status
```

Everything through step 6 needs only `git` and `cargo`, as the ancients
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

## II. Speak the new number 🔢

```sh
# edit the root Cargo.toml, both places, in one pass:
#   [workspace.package]       version = "X.Y.Z"
#   [workspace.dependencies]  every "=OLD" pin becomes "=X.Y.Z"
cargo check --offline          # updates Cargo.lock to match
packaging/version.sh           # must print X.Y.Z, and nothing else
```

The root `Cargo.toml` is the only file a bump touches: every crate inherits its
version from `[workspace.package]`, and every crate names the others through an
exact `=X.Y.Z` pin, because a `monas` of one version works with an `sdrtop` of
the same version and no other. Forget a pin and cargo refuses to resolve before
you have finished typing; loosen one by accident (`"X.Y.Z"` without the `=`)
and `the_internal_pins_are_the_workspace_version` fails in CI, on the bump
commit, a long way before any tag.

`Cargo.lock` is not decorative. CI runs `--locked` in every job, and a lockfile
still whispering the previous version fails the build in a way that looks
profoundly mysterious for about ten minutes and then extremely stupid forever.

> *There used to be **two** answers to "what version is this". One read
> `Cargo.toml` with `sed`. The other started an entire container to run
> `cargo metadata` and then applied a regular expression to the resulting JSON,
> keeping the first `"version"` field it happened to see. They agreed, right up
> until the day they would not have. Now there is `version.sh`, and there is
> only ever one answer.*

## III. Await the omens 🔮

Push to `main` (**not the tag**, we are nowhere near ready for the tag) and let
CI speak: formatting, clippy, the test suite, `cargo package` of every crate in
the workspace, the 1.88 MSRV check, shellcheck.

Green means green. "Mostly green" is not a colour. It is a mood, and it is
lying to you.

While the machines deliberate, consult the one oracle that runs on your own
desk:

```sh
packaging/unused-pub.sh
```

Since the engine became libraries, the compiler no longer mourns a `pub`
function whose last caller died; in a library, a `pub` item is merely
*offered*, and nobody checks whether anyone came. This lists the ones nobody
came for. It is advisory and says so: it counts names, not paths, so it
cannot prove anything alive, only point at what looks dead. Bury what is dead,
or make it `pub(crate)` again. An empty list is the expected answer, and a
pleasant one.

## IV. COMMUNE WITH THE ACTUAL RADIO 📻

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

Then the same radio again, with no screen at all:

```sh
./target/release/monasd --config /tmp/sdrtop-release-check.toml
curl -s http://127.0.0.1:7373/v1/health      # in another terminal
```

`Ctrl-C` stops it and saves the radio's blocks into that same throwaway config,
which is the other reason for `--config`. A server that answers on a laptop with
the radio plugged in has passed its rehearsal. A server that answers with no
radio plugged in has passed a different, sadder test.

Every other step in this document is a computer inspecting a computer. This one
is a human being, looking at a screen, deciding whether the thing is any good.
It is the only step that cannot be faked and the only one that has ever caught
anything interesting.

## V. THE SUMMONING 🏷️

```sh
git add Cargo.toml Cargo.lock CHANGELOG.md   # the root Cargo.toml; the crates' own never change
git commit -m "chore(release): X.Y.Z"
git tag -a vX.Y.Z -m "sdrtop X.Y.Z"
git push origin main --follow-tags
```

**That tag is the point of no return.**

Everything before it can be undone with a shrug and a `git tag -d`. After it,
machines begin acting on your behalf, and one of them is about to do something
permanent while you watch a progress spinner.

## VI. Observe the machines at their work ⚙️

`.github/workflows/release.yaml`. Three jobs, in this order, for this reason:

| Job | Deed | Reversible |
|---|---|---|
| `build` | Linux tarball (both programs, both man pages) in the Debian 12 container, tag matched against `Cargo.toml`, changelog section confirmed to exist, glibc floor asserted, exact shared-library set asserted | yes |
| `build-macos` | macOS tarballs on an Apple Silicon and an Intel runner: architecture, system-only links, minimum macOS and ad-hoc signature asserted, then installed offline from the unpacked tarball and run | yes |
| `collect` | all three tarballs counted, `SHA256SUMS` written, provenance attestation signed | yes |
| `publish` | every crate to crates.io via Trusted Publishing, the engine first and the programs after it, each one asked about first and skipped if it is already there | **no. never. under no circumstances whatsoever** |
| `release` | drafts the GitHub release with your changelog as its body | yes |

The irreversible act sits in the **middle**: guarded by everything `build`,
`build-macos` and `collect` check, followed only by a step that is safe to retry
until the heat death of the universe.

If any of those three fails, breathe out. Nothing has happened. Proceed to the
Bestiary.

## VII. Interrogate the draft before the public may 🔍

Draft assets are downloadable to those with write access, which is precisely why
this project needs no prerelease. Should `gh` decline, the draft's own page in
the browser holds the same files.

```sh
gh release download vX.Y.Z --dir /tmp/rel
cd /tmp/rel && tar -xzf sdrtop-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz
sh sdrtop-X.Y.Z-x86_64-unknown-linux-gnu/install.sh --prefix ~/.cache/rel-test
```

That installs `sdrtop` **and** `monasd`, with a man page each. Check both
answered `--version` at the end of its output; a release whose second program
quietly failed to install is a release with one program in it.

That was the offline path. Now the other two, which need the crates.io version
to be live, and by this point it is *aggressively* live:

```sh
sh packaging/install.sh --prefix ~/.cache/rel-test --from-source
sh packaging/install.sh --prefix ~/.cache/rel-test --git
sh packaging/install.sh --prefix ~/.cache/rel-test --uninstall
```

The two macOS tarballs can't be tried here, there being no Mac on the desk.
`build-macos` already installed each one from its unpacked tarball on a real
Mac and ran it, which is what the offline line above does for Linux. Their
entries in `SHA256SUMS` and their attestations are checked like Linux's.

And demand that the attestation prove itself:

```sh
for t in x86_64-unknown-linux-gnu aarch64-apple-darwin x86_64-apple-darwin; do
    gh attestation verify sdrtop-X.Y.Z-$t.tar.gz --repo musithang/sdrtop
done
```

> *The checksum check in `install.sh` once lived inside an `if` whose every
> branch continued. A failed `SHA256SUMS` download, or a machine without
> `sha256sum`, meant the installer skipped verification entirely and said
> absolutely nothing about it. It looked like a check. It read like a check. It
> was a decoration. It now stops, loudly, and `--no-verify` exists for people
> who have a reason and know they have one.*

## VIII. READ IT. With your eyes. 👁️

Open the draft. Read it as a stranger who has never heard of you, does not care
about your refactor, and will judge the entire project by this one page.

The title. The notes. The asset names. The typo in the third bullet that you
will otherwise find in eleven minutes.

*Then* press Publish.

## IX. Confirm the world has accepted your offering 🌍

```sh
cargo install sdrtop --locked --force
cargo install monasd --locked --force
sdrtop --version        # X.Y.Z and a commit hash. Both. Look at them.
monasd --version        # the same X.Y.Z. If not, one of them is lying.
```

Go outside. You have earned weather. 🎉

---

# 🍼 THE RITE OF THE FIRST OFFERING

*Performed once in the life of each crate, and never again. Most releases skip
this page. The first release that carries a crate crates.io has never seen does
not, and the machines will tell you so by stopping.*

crates.io will let a crate trust this workflow only once the crate **exists**.
A crate that does not exist yet cannot be configured, so the workflow's token is
refused for it, and its first version has to go up the old way: by your hand,
with a token of your own, once. The `publish` job knows this. It asks crates.io
about every crate before publishing any, and when one has never been seen, it
publishes nothing, names it, and points here.

1. **Make a token** on crates.io, scoped to `publish-new` and nothing else, with
   an expiry measured in hours.

2. **Offer the new crates, the deepest first:** the building blocks, then the
   signal crates made of them, then the engine, then any program that is new.
   The `publish` job's message names exactly which ones are missing; offer
   those, in this order, and no others:

   ```sh
   cargo publish -p monas-dsp --locked   # everything stands on it
   cargo publish -p monas --locked       # every program depends on it
   cargo publish -p monasd --locked      # and then any program that is new
   ```

   A crate offered before one it depends on is refused by crates.io, which is
   at least a refusal you can undo by doing it in the right order.

   Not `sdrtop`, which already exists and already trusts the workflow. It goes
   up with the rerun in step 5, the way it always has.

3. **Teach each new crate whom to trust.** On crates.io, each crate's
   Settings, Trusted Publishing: repository `musithang/sdrtop`, workflow
   `release.yaml` (**with an `a`**), Environment left empty. Exactly as
   `sdrtop`'s is, or the Cryptic One below will have its evening.

4. **Close the door behind you.** Switch each crate, `sdrtop` included, to
   **Trusted Publishing only**, and revoke the token. Now nothing but the
   workflow can ever publish them again, which is the First Law with the safety
   catch on.

5. **Rerun the `publish` job** from the Actions page. It finds the new crates
   already there at this version, skips them, publishes whatever is left, and
   the `release` job drafts the page as usual.

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

## 🟠 The Half-Fed One: `publish` stopped halfway

`monas` went up, then something broke before the programs did. Do not panic,
do not bump, and above all do not try to publish the missing ones by hand with
a fresh token "just this once", because Trusted Publishing only will refuse you
and it will be right.

Rerun the `publish` job. It asks crates.io about each crate before it does
anything, leaves out the ones already there, and publishes the rest in order.
What went up stays up; what did not, goes up now. Nobody needs to know it took
two tries.

## ⚫ The Unborn One: `publish` stops and names a crate that was never published

Nothing has been published. The job checked every crate first, found one that
crates.io has never heard of, and stopped before spending a single number,
because a crate that does not exist cannot trust the workflow yet.

This is not a failure. It is the Rite of the First Offering, knocking. Perform
it, then rerun.

## 🟣 The Cryptic One: the token exchange failed in `publish`

Trusted Publishing on crates.io names this repository **and the workflow
filename**, separately for each crate, so a new crate configured from memory is
a new chance to get it wrong. Rename `release.yaml`, or add an `environment:` key to the `publish`
job, and the whole thing breaks until you edit the crates.io side to match.

It is `release.yaml`. **With an `a`.** Every example on the internet says
`.yml`. This one has eaten entire evenings belonging to better people than us
and it will happily eat yours.

## 🔵 The Ubuntu One: "it works on my machine, but not on my other machine"

The prebuilt Linux tarball needs x86_64 Linux with glibc 2.36+. The macOS
tarballs need macOS 11+ on Apple Silicon and 10.12+ on Intel. The installer
runs the binary to check compatibility. Other architectures, musl and older
glibc systems use the source-build fallback.

Native SDR libraries are optional runtime loads. Linux candidates are
`libhackrf.so.0` / `libhackrf.so` and
`librtlsdr.so.0` / `librtlsdr.so.2` / `librtlsdr.so`.
Debian's `librtlsdr0` and Ubuntu's `librtlsdr2` are both supported.
HackRF requires libhackrf 2023.01.1+ with all required symbols.

The release container builds without native SDR libraries or development headers.
The release workflow requires exactly `libc.so.6`, `libgcc_s.so.1` and
`libm.so.6` in `DT_NEEDED`. It explicitly rejects libhackrf and librtlsdr links.
`cargo publish` retains its verification build, for every crate, without installing SDR libraries.

`install.sh` installs native runtimes for current releases only with `--hackrf` or `--rtlsdr`.
`--soapy` selects SoapySDR and its driver modules. `--deps-only` installs only
selected runtimes. With no runtime flags it exits with a usage error.
Source-build packages contain compiler/linker tools.
Older releases can still need SDR development packages.
The installer recognizes sdrtop's missing-libhackrf build-script panic and
linker errors for missing `-lhackrf` or `-lrtlsdr`.
Only these failures trigger development package installation and one retry.
The retry pins the same release version. `--git` receives no legacy retry.
A built binary must pass its startup check before it replaces an existing installation.

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

# 🤖 APPENDIX: automating step II, should you weaken

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

and add to the root `Cargo.toml`:

```toml
[workspace.metadata.release]
shared-version = true
tag-name = "v{{version}}"
pre-release-replacements = [
  { file = "CHANGELOG.md", search = "## \\[Unreleased\\]", replace = "## [Unreleased]\n\n## [{{version}}] - {{date}}" },
]
```

Steps II and V then collapse into `cargo release X.Y.Z --execute`, provided the
version of cargo-release you installed also rewrites the exact pins in
`[workspace.dependencies]`. Read its diff before believing it. The pins test
will catch it if it does not, but the pins test catching it means the tag is
already in your hand.

Steps I, IV, VII and VIII still demand a human with functioning eyes and an
actual radio on the desk.

That is not an oversight. That is the job.

```
                        73 de sdrtop 📻
```
