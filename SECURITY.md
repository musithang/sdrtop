# Security

sdrtop only listens. It never transmits, it opens no network port of its own,
and most of what it does happens between a USB cable and your terminal. That
still leaves real places for a security problem to hide, and if you have found
one, I want to hear about it. Privately.

---

## Reporting a problem

**Not in a public issue, please.** Use one of these:

1. **GitHub's private reporting** (preferred):
   [report a vulnerability](https://github.com/musithang/sdrtop/security/advisories/new)
2. **E-mail**: viktor.laszlo92@protonmail.com

Tell me:

- `sdrtop --version`, and how you installed it
- what the problem is, and what someone could do with it
- how to reproduce it

## What happens then

This is a one-person hobby project, so I can't promise a security team's
response times. What I can promise:

- **An answer within 7 days**, even if the answer is "got it, looking".
- **If it's real, it is fixed before the next feature**, and the release notes
  say what was fixed and who found it, unless you would rather not be named.
- **Please give me time to ship the fix** before you write about it publicly.

## Which versions get fixes

The latest release, and `main`. There are no backports: a fix lands in a new
release, and upgrading is how you get it. Re-running the installer or
`cargo install sdrtop --locked --force` does that. On Arch, your AUR helper
does, once the package has been updated.

---

## Where to look

What counts:

- **The installer.** `packaging/install.sh` is meant to be piped into a shell,
  and its checksum check once skipped itself without a word. Anything that lets
  it install something other than what was published is exactly what this page
  is for.
- **The release.** The tarball, its checksums, and its build provenance
  attestation.
- **Anything that arrives over the air.** A Bluetooth device chooses its own
  name, and sdrtop prints it in your terminal. Control characters are replaced
  before that happens; a name that gets past it, or any received data that can
  crash or fool a decoder, is in scope.
- **Files sdrtop reads**: the config, presets, themes, and whatever a
  SoapyRemote server sends back.
- **Files sdrtop writes**: recordings, exports and the config.

What doesn't:

- **Bugs in libhackrf, librtlsdr, SoapySDR or its driver modules.** sdrtop loads
  them at runtime, and the project behind each one is the right place to report.
- **Anything that needs an attacker already logged in as you.** At that point
  sdrtop is not the problem.
