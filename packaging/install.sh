#!/bin/sh

# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# Install sdrtop on any Linux distribution.
#
#   curl -fsSL https://raw.githubusercontent.com/musithang/sdrtop/main/packaging/install.sh | sh
#   sh install.sh --prefix ~/.local        # no root anywhere
#   sh install.sh --from-source            # skip the prebuilt binary
#   sh install.sh --git                    # build the main branch, not a release
#   sh install.sh --uninstall
#
# There are two ways to get sdrtop and this script picks between them, so nobody
# else has to:
#
#   1. the prebuilt tarball, x86_64 glibc only, published on the release page,
#   2. `cargo install sdrtop --locked`, which works everywhere because it
#      compiles on the machine it is installing to.
#
# It decides by asking rather than by guessing: it unpacks the binary, tries to
# run it, and falls through to cargo if that fails for any reason at all.
#
# That fallback is the whole design. sdrtop links `librtlsdr`, and Debian ships
# it as `librtlsdr.so.0` while Ubuntu ships the same upstream as `.so.2`, so no
# single prebuilt binary can serve both. Running the binary catches that, and
# also catches musl, aarch64, a missing loader and every future soname bump,
# none of which a list of distribution names would have covered.
#
# What this script does NOT do, deliberately: build sdrtop itself. It used to
# carry its own download-and-compile pipeline, which was a second unmaintained
# copy of the build recipe. `cargo install` is that job, better tested than
# anything here could be, because the whole Rust ecosystem runs it daily.
set -eu

REPO=musithang/sdrtop
CRATE=sdrtop
WANT_SOAPY=0
RAW_DEPS_ONLY=0
FROM_SOURCE=0
FROM_GIT=0
NO_VERIFY=0
UNINSTALL=0
PREFIX=""
TAG=""

say()  { printf '%s\n' "$*"; }
step() { printf '\n== %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
sdrtop installer

  --prefix DIR    install under DIR (default /usr/local, or ~/.local without root)
  --version TAG   install a specific release, e.g. v0.4.1 (default: the latest)
  --from-source   compile with cargo even on x86_64, skipping the prebuilt binary
  --git           compile the main branch instead of a release (unversioned)
  --no-verify     install a download without checking its checksum (say why first)
  --soapy         also install libSoapySDR and its driver modules (optional)
  --deps-only     install the library dependencies and stop
  --uninstall     remove what a previous run installed
  --help          this
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)  PREFIX="${2:?--prefix needs a directory}"; shift 2 ;;
        --version) TAG="${2:?--version needs a tag}"; shift 2 ;;
        --from-source) FROM_SOURCE=1; shift ;;
        --git)         FROM_GIT=1; FROM_SOURCE=1; shift ;;
        --no-verify)   NO_VERIFY=1; shift ;;
        --soapy)       WANT_SOAPY=1; shift ;;
        --deps-only)   RAW_DEPS_ONLY=1; shift ;;
        --uninstall)   UNINSTALL=1; shift ;;
        --help|-h)     usage; exit 0 ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

[ "$(uname -s)" = Linux ] || die "sdrtop is Linux only; this is $(uname -s)"

# `--git` installs whatever main happens to be, which has no version number to
# ask for. Silently ignoring one would install something other than what was
# asked for, which is the failure this whole flag exists to make visible.
if [ "$FROM_GIT" -eq 1 ] && [ -n "$TAG" ]; then
    die "--git and --version are mutually exclusive: --git installs the main branch"
fi

# ── Where things go ─────────────────────────────────────────────────────────
# Root is not required. Without it the default moves to ~/.local, which is on
# PATH on most systems and needs no privileges at all.
SUDO=""
if [ "$(id -u)" -ne 0 ]; then
    if command -v sudo >/dev/null 2>&1; then
        SUDO="sudo"
    fi
fi
if [ -z "$PREFIX" ]; then
    if [ "$(id -u)" -eq 0 ] || [ -n "$SUDO" ]; then
        PREFIX=/usr/local
    else
        PREFIX="$HOME/.local"
        say "no root and no sudo, installing into $PREFIX"
    fi
fi
# Writing inside your own home never needs sudo, whatever the prefix defaulted to.
case "$PREFIX" in "$HOME"/*) SUDO="" ;; esac
as_root() { if [ -n "$SUDO" ]; then $SUDO "$@"; else "$@"; fi; }

BIN_DIR="$PREFIX/bin"
MAN_DIR="$PREFIX/share/man/man1"
DOC_DIR="$PREFIX/share/doc/sdrtop"

if [ "$UNINSTALL" -eq 1 ]; then
    step "Removing sdrtop from $PREFIX"
    as_root rm -f "$BIN_DIR/sdrtop" "$MAN_DIR/sdrtop.1" "$MAN_DIR/sdrtop.1.gz"
    as_root rm -rf "$DOC_DIR"
    say "done. Config and logs in ~/.config/sdrtop were left alone."
    exit 0
fi

# ── Which package manager, and what it calls the two libraries ──────────────
# Candidate lists, tried left to right until one installs. A wrong first guess
# costs a failed attempt, not a failed installation, because the last candidate
# in every list is the -dev/-devel package: distributions rename the runtime
# library when its soname changes but keep the development package name stable,
# and installing it pulls the runtime library in as a dependency.
PM=""
for c in apt-get dnf yum pacman zypper apk xbps-install emerge nix-env; do
    if command -v "$c" >/dev/null 2>&1; then PM="$c"; break; fi
done

# The names below were read off each distribution's own package index, not
# remembered. The one that surprises: Debian calls the rtl-sdr runtime
# `librtlsdr0` all the way through trixie, where the package holds
# librtlsdr.so.0 pointing at librtlsdr.so.2.0.1, while Ubuntu packaged the same
# upstream as `librtlsdr2` with soname .so.2. Kali and Raspberry Pi OS follow
# Debian, Mint follows Ubuntu. Both names are in the list, so neither family
# needs to be detected.
UDEV_PKGS=""
# SoapySDR is optional: sdrtop opens it at runtime and works without it, so
# these are only used by --soapy. Two lists, because the library alone finds no
# devices: SOAPY_LIB is what sdrtop loads, SOAPY_MODULES is what actually talks
# to a radio.
#
# **Confidence varies, and that matters more than tidiness here.** The apt names
# were read off this machine's own index; Arch, Void, Gentoo and nixpkgs come
# from repology. Fedora, openSUSE and Alpine could not be checked and are
# best-effort: `install_first` tries each candidate and warns if none take, so a
# wrong guess costs a warning rather than a failed install. If you are on one of
# those three and it warns, the right fix is to read your distribution's index
# and correct the list, not to add another guess.
SOAPY_LIB=""
SOAPY_MODULES=""
case "$PM" in
    apt-get)
        LIB_HACKRF="libhackrf0 libhackrf-dev"
        LIB_RTLSDR="librtlsdr0 librtlsdr2 librtlsdr-dev"
        SOAPY_LIB="libsoapysdr0.8 libsoapysdr0.7 libsoapysdr-dev"
        SOAPY_MODULES="soapysdr-module-all"
        DEV_PKGS="libhackrf-dev librtlsdr-dev pkg-config build-essential" ;;
    dnf|yum)
        LIB_HACKRF="hackrf hackrf-devel"
        LIB_RTLSDR="rtl-sdr rtl-sdr-devel"
        SOAPY_LIB="SoapySDR"
        SOAPY_MODULES="SoapySDR-hackrf SoapySDR-rtlsdr SoapySDR-airspy SoapySDR-plutosdr"
        DEV_PKGS="hackrf-devel rtl-sdr-devel pkgconf-pkg-config gcc" ;;
    pacman)
        LIB_HACKRF="hackrf"
        LIB_RTLSDR="rtl-sdr"
        SOAPY_LIB="soapysdr"
        SOAPY_MODULES="soapyhackrf soapyrtlsdr soapyairspy soapyplutosdr"
        DEV_PKGS="hackrf rtl-sdr pkgconf base-devel" ;;
    zypper)
        LIB_HACKRF="libhackrf0 hackrf libhackrf-devel"
        LIB_RTLSDR="librtlsdr0 rtl-sdr rtl-sdr-devel"
        SOAPY_LIB="libSoapySDR0_8 SoapySDR SoapySDR-devel"
        SOAPY_MODULES="SoapySDR-module-hackrf SoapySDR-module-rtlsdr"
        DEV_PKGS="libhackrf-devel rtl-sdr-devel pkg-config gcc" ;;
    apk)
        # Alpine splits further than anyone else: the library, the headers and
        # the udev rules are three packages.
        LIB_HACKRF="hackrf-libs hackrf-dev"
        LIB_RTLSDR="librtlsdr librtlsdr-dev"
        SOAPY_LIB="soapysdr soapysdr-dev"
        SOAPY_MODULES="soapysdr-hackrf soapysdr-rtlsdr"
        UDEV_PKGS="hackrf-udev librtlsdr-udev"
        DEV_PKGS="hackrf-dev librtlsdr-dev pkgconf build-base" ;;
    xbps-install)
        LIB_HACKRF="hackrf hackrf-devel"
        LIB_RTLSDR="rtl-sdr rtl-sdr-devel"
        SOAPY_LIB="SoapySDR SoapySDR-devel"
        SOAPY_MODULES="SoapyHackRF SoapyRTLSDR"
        DEV_PKGS="hackrf-devel rtl-sdr-devel pkg-config base-devel" ;;
    emerge)
        LIB_HACKRF="net-wireless/hackrf"
        LIB_RTLSDR="net-wireless/rtl-sdr"
        SOAPY_LIB="net-wireless/soapysdr"
        SOAPY_MODULES="net-wireless/soapyhackrf net-wireless/soapyrtlsdr"
        DEV_PKGS="net-wireless/hackrf net-wireless/rtl-sdr" ;;
    nix-env)
        LIB_HACKRF="hackrf"
        LIB_RTLSDR="rtl-sdr"
        SOAPY_LIB="soapysdr-with-plugins soapysdr"
        SOAPY_MODULES=""
        DEV_PKGS="hackrf rtl-sdr pkg-config" ;;
    "")
        LIB_HACKRF=""; LIB_RTLSDR=""; DEV_PKGS="" ;;
esac

pm_install() {
    case "$PM" in
        apt-get) as_root apt-get install -y -qq "$1" ;;
        dnf)     as_root dnf install -y "$1" ;;
        yum)     as_root yum install -y "$1" ;;
        pacman)  as_root pacman -S --needed --noconfirm "$1" ;;
        zypper)  as_root zypper --non-interactive install "$1" ;;
        apk)     as_root apk add "$1" ;;
        xbps-install) as_root xbps-install -Sy "$1" ;;
        emerge)  as_root emerge --noreplace "$1" ;;
        nix-env) nix-env -iA "nixpkgs.$1" ;;
        *) return 1 ;;
    esac
}

install_first() {
    for p in $1; do
        if pm_install "$p" >/dev/null 2>&1; then
            say "  $p"
            return 0
        fi
    done
    return 1
}

have_libs() {
    ldconfig -p 2>/dev/null | grep -q 'libhackrf\.so' \
        && ldconfig -p 2>/dev/null | grep -q 'librtlsdr\.so'
}

# SoapySDR is not a dependency. sdrtop opens it at runtime if it is there and
# behaves exactly as it always did if it is not, so this asks rather than
# requires.
have_soapy() {
    ldconfig -p 2>/dev/null | grep -q 'libSoapySDR\.so'
}

install_soapy() {
    if have_soapy; then
        say "libSoapySDR is already present"
    elif [ -z "$PM" ] || [ -z "$SOAPY_LIB" ]; then
        warn "no SoapySDR package list for this system; install libSoapySDR yourself"
        return 0
    else
        step "Installing libSoapySDR with $PM"
        install_first "$SOAPY_LIB" || warn "could not install libSoapySDR automatically"
    fi
    # The library alone finds nothing. Each radio needs its own driver module,
    # and a machine with the library and no modules is the most confusing
    # possible outcome: sdrtop loads SoapySDR successfully and then lists no
    # devices.
    for p in $SOAPY_MODULES; do
        if pm_install "$p" >/dev/null 2>&1; then say "  $p"; fi
    done
}

install_runtime_deps() {
    if have_libs; then
        say "libhackrf and librtlsdr are already present"
        return 0
    fi
    if [ -z "$PM" ]; then
        warn "no known package manager; install libhackrf and librtlsdr yourself"
        return 0
    fi
    step "Installing libhackrf and librtlsdr with $PM"
    say "(this can take a moment, and is the only step that needs root)"
    # Not fatal. A stale or unreachable mirror must not end the installation
    # before the libraries have even been tried, and they may be cached already.
    if [ "$PM" = apt-get ]; then
        as_root apt-get update -qq || warn "apt-get update failed, trying anyway"
    fi
    install_first "$LIB_HACKRF" || warn "could not install libhackrf automatically"
    install_first "$LIB_RTLSDR" || warn "could not install librtlsdr automatically"
    # Where a distribution ships the udev rules as their own package, take them.
    # This is the only udev handling here, and it is a package install like any
    # other. See "Device access" at the bottom for why nothing is hand-written.
    for p in $UDEV_PKGS; do
        if pm_install "$p" >/dev/null 2>&1; then say "  $p"; fi
    done
}

install_runtime_deps
[ "$WANT_SOAPY" -eq 1 ] && install_soapy
[ "$RAW_DEPS_ONLY" -eq 1 ] && exit 0

BINARY=""
SRC_DIR=""

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

# ── An unpacked tarball installs itself, with no network at all ─────────────
# install.sh travels inside the release tarball, so a copy of it sitting beside
# an `sdrtop` binary and the `user_docs` directory the tarball carries is an
# unpacked release rather than a coincidence. Tested that precisely: `$0` says
# nothing useful when this script arrives through a pipe, so a looser check
# could pick up an unrelated file named `sdrtop` in whatever directory the
# `curl | sh` happened to run in.
case "$0" in
    */install.sh|install.sh)
        d=$(cd "$(dirname "$0")" 2>/dev/null && pwd) || d=""
        if [ -n "$d" ] && [ -x "$d/sdrtop" ] && [ -d "$d/user_docs" ] \
           && [ "$FROM_SOURCE" -eq 0 ]; then
            step "Installing from the unpacked tarball beside this script"
            if "$d/sdrtop" --version >/dev/null 2>&1; then
                BINARY="$d/sdrtop"
                SRC_DIR="$d"
            else
                say "that binary does not run here; fetching a release instead"
            fi
        fi
        ;;
esac

# ── The prebuilt binary, if it can possibly work here ───────────────────────
if [ -z "$BINARY" ] && [ "$FROM_SOURCE" -eq 0 ]; then
    command -v curl >/dev/null 2>&1 || die "curl is required"
    command -v tar  >/dev/null 2>&1 || die "tar is required"
    fetch() { curl -fsSL "$1" -o "$2"; }

    if [ -z "$TAG" ]; then
        step "Finding the latest release"
        # No jq: the API's own field, first match, nothing else on the line.
        TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
              | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
        [ -n "$TAG" ] || die "could not determine the latest release; pass --version"
    fi
    VERSION="${TAG#v}"
    say "sdrtop $TAG"

    ARCH=$(uname -m)
    if [ "$ARCH" != x86_64 ]; then
        say "no prebuilt binary for $ARCH; compiling instead"
    else
        step "Downloading the prebuilt binary"
        BASE="https://github.com/$REPO/releases/download/$TAG"
        # The asset name is the full Rust target triple from 0.4.2 onwards.
        # v0.4.1, the only release older than that with a tarball attached,
        # carries the short form `sdrtop-0.4.1-x86_64-linux.tar.gz`, and
        # `--version v0.4.1` has to keep working. So both are tried, newest
        # spelling first. Drop the legacy candidate once no supported release
        # uses it, which is the moment 0.4.1 stops being worth installing.
        NAME=""
        for candidate in \
            "sdrtop-$VERSION-x86_64-unknown-linux-gnu" \
            "sdrtop-$VERSION-x86_64-linux"
        do
            if fetch "$BASE/$candidate.tar.gz" "$WORK/$candidate.tar.gz"; then
                NAME="$candidate"
                break
            fi
        done

        if [ -z "$NAME" ]; then
            warn "no prebuilt tarball for $TAG; compiling instead"
        else
            # Verification is fatal, and that is the fix for the version of this
            # script that wrapped it in an `if` whose every branch continued. A
            # failed SHA256SUMS download, or a machine without sha256sum, then
            # installed an unverified binary and said nothing. A check that a
            # network hiccup can skip is not a check.
            if [ "$NO_VERIFY" -eq 1 ]; then
                warn "--no-verify: installing $NAME.tar.gz without checking its checksum"
            else
                command -v sha256sum >/dev/null 2>&1 \
                    || die "sha256sum is needed to verify the download (or pass --no-verify)"
                fetch "$BASE/SHA256SUMS" "$WORK/SHA256SUMS" \
                    || die "could not download SHA256SUMS for $TAG (or pass --no-verify)"
                grep " $NAME.tar.gz\$" "$WORK/SHA256SUMS" > "$WORK/expected.sha256" \
                    || die "SHA256SUMS for $TAG does not list $NAME.tar.gz"
                ( cd "$WORK" && sha256sum -c expected.sha256 >/dev/null ) \
                    || die "checksum mismatch on $NAME.tar.gz; refusing to install it"
                say "  checksum verified"
            fi

            tar -xzf "$WORK/$NAME.tar.gz" -C "$WORK"

            # The honest test, and the reason this script needs no distribution
            # list: run the thing. `--version` returns before any device is
            # opened, so it needs no radio, and it fails for every reason that
            # matters here.
            #
            # Running it beats reading `ldd` output. On musl `ldd` is the musl
            # loader, which cannot load a glibc binary at all and says so
            # without ever printing "not found", so a grep for that phrase would
            # conclude the binary was fine and install something that cannot
            # start.
            if "$WORK/$NAME/sdrtop" --version >/dev/null 2>&1; then
                BINARY="$WORK/$NAME/sdrtop"
                SRC_DIR="$WORK/$NAME"
            else
                say "the prebuilt binary does not run on this system:"
                ldd "$WORK/$NAME/sdrtop" 2>&1 | grep -E 'not found|Error|error' \
                    | sed 's/^/    /' || say "    (it failed to start)"
                say "compiling instead, which links what you do have"
            fi
        fi
    fi
fi

# ── Or compile it, which is one cargo command ───────────────────────────────
if [ -z "$BINARY" ]; then
    step "Compiling with cargo"

    # Only if they are not already there. Someone who has built sdrtop before,
    # or who has any SDR development environment, should not be asked for a root
    # password to install packages they have. pkg-config is asked first, but
    # librtlsdr ships no .pc file on some distributions, so the header is the
    # fallback question.
    have_dev() {
        { pkg-config --exists libhackrf 2>/dev/null \
            || [ -e /usr/include/libhackrf/hackrf.h ] || [ -e /usr/include/hackrf.h ]; } \
        && { pkg-config --exists librtlsdr 2>/dev/null \
            || [ -e /usr/include/rtl-sdr.h ]; }
    }
    if have_dev; then
        say "the build dependencies are already installed"
    elif [ -n "$PM" ]; then
        say "installing the build dependencies (this will ask for your password)"
        for p in $DEV_PKGS; do pm_install "$p" >/dev/null 2>&1 || true; done
    fi

    # Distribution Rust is usually too old: sdrtop needs 1.88 for
    # `slice::as_chunks`, and Debian 12 ships 1.63. A toolchain between the
    # lockfile floor and that one compiles the lockfile and then fails on the
    # source, which is a confusing way to find out.
    need_rustup=1
    if command -v cargo >/dev/null 2>&1; then
        rv=$(cargo --version 2>/dev/null | sed -n 's/^cargo \([0-9]*\)\.\([0-9]*\).*/\1 \2/p')
        # shellcheck disable=SC2086 # deliberate: two fields into $1 and $2
        set -- $rv
        if [ "${1:-0}" -gt 1 ] || { [ "${1:-0}" -eq 1 ] && [ "${2:-0}" -ge 88 ]; }; then
            need_rustup=0
        else
            say "cargo $(cargo --version | cut -d' ' -f2) is too old, sdrtop needs 1.88"
        fi
    fi
    if [ "$need_rustup" -eq 1 ]; then
        say "installing Rust with rustup (into ~/.rustup and ~/.cargo, no root)"
        curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
            | sh -s -- -y --profile minimal --default-toolchain stable >/dev/null
        # shellcheck disable=SC1091 # written by rustup a moment ago
        . "$HOME/.cargo/env"
    fi

    say ""
    say "This compiles sdrtop from source. Expect a few minutes on a laptop and"
    say "considerably longer on a Raspberry Pi. Nothing is wrong if it goes quiet."
    say ""

    # Into a throwaway root, not straight into $PREFIX. Compiling as an ordinary
    # user and then copying one file with sudo keeps root to a single `install`
    # command, instead of handing a whole build to it. It also means both paths
    # through this script end the same way, with $BINARY pointing at a file.
    # One command, three sources. `cargo install` takes the crate name
    # positionally whichever source it reads from, so only the source arguments
    # differ and there is no reason to write the command out three times.
    src_args=""
    src_label="the latest $CRATE from crates.io"
    if [ "$FROM_GIT" -eq 1 ]; then
        src_args="--git https://github.com/$REPO"
        src_label="the main branch from git"
    elif [ -n "$TAG" ]; then
        src_args="--version ${TAG#v}"
        src_label="$CRATE ${TAG#v} from crates.io"
    fi
    say "building $src_label"
    # PATH is prefixed with the throwaway root only for this command, to silence
    # cargo's "be sure to add ... to your PATH" advice. That advice names the
    # temporary directory, which stops existing seconds later, and this script
    # prints the correct PATH line about $BIN_DIR at the end anyway. Two pieces
    # of contradictory advice is worse than one.
    # shellcheck disable=SC2086 # deliberate: $src_args is an argument list
    PATH="$WORK/cargo/bin:$PATH" \
        cargo install "$CRATE" --locked --root "$WORK/cargo" $src_args \
        || die "the build failed; see the output above"

    BINARY="$WORK/cargo/bin/sdrtop"
    [ -x "$BINARY" ] || die "cargo reported success but produced no binary"
    # No SRC_DIR: `cargo install` delivers a binary and nothing else, so this
    # path installs no README, man page or user_docs. That is what the command
    # means, and the documentation lives at github.com/musithang/sdrtop.
fi

# ── Install ─────────────────────────────────────────────────────────────────
step "Installing into $PREFIX"
as_root install -Dm755 "$BINARY" "$BIN_DIR/sdrtop"
say "  $BIN_DIR/sdrtop"
if [ -n "$SRC_DIR" ]; then
    if [ -f "$SRC_DIR/sdrtop.1" ]; then
        as_root install -Dm644 "$SRC_DIR/sdrtop.1" "$MAN_DIR/sdrtop.1"
        say "  $MAN_DIR/sdrtop.1"
    fi
    for f in "$SRC_DIR/README.md" "$SRC_DIR/LICENSE"; do
        [ -f "$f" ] && as_root install -Dm644 "$f" "$DOC_DIR/$(basename "$f")"
    done
    if [ -d "$SRC_DIR/user_docs" ]; then
        for f in "$SRC_DIR"/user_docs/*.md; do
            [ -f "$f" ] && as_root install -Dm644 "$f" "$DOC_DIR/user_docs/$(basename "$f")"
        done
    fi
fi

# ── SoapySDR, if it happens to be here ──────────────────────────────────────
# Reported, not required, for the same reason the udev rules below are reported
# rather than written: sdrtop works without it, and a machine that has it should
# be told what that buys.
step "SoapySDR"
if have_soapy; then
    say "libSoapySDR is present, so SoapySDR devices will be offered too."
    if command -v SoapySDRUtil >/dev/null 2>&1; then
        say "  'SoapySDRUtil --find' lists what it can see."
    else
        say "  Install the SoapySDR tools for 'SoapySDRUtil --find', which is the"
        say "  first thing to check if a device does not appear."
    fi
    say "  That backend is beta: written from the API rather than from owning the"
    say "  radios. Reports either way are genuinely welcome."
else
    say "libSoapySDR is not installed, and sdrtop does not need it."
    say "  With it, sdrtop also reaches Airspy, SDRplay, PlutoSDR, LimeSDR,"
    say "  bladeRF, USRP and anything else with a SoapySDR driver."
    say "  Re-run this installer with --soapy to add it."
fi

# ── The radio has to be openable ────────────────────────────────────────────
# Reported, never written. The libhackrf and rtl-sdr packages ship their own
# udev rules, so a machine with the libraries has the permissions, and a second
# set of rules written here would be a second answer to one question that agrees
# with the first only until someone edits one of them.
#
# On Alpine the rules are separate packages, and those were installed above with
# the libraries, which is the same mechanism rather than an exception to it.
step "Device access"
if grep -rqs -e 'idVendor.*1d50' -e 'idVendor.*0bda' \
     /usr/lib/udev/rules.d /lib/udev/rules.d /etc/udev/rules.d; then
    say "udev rules for SDR hardware are installed"
else
    warn "no udev rules for SDR hardware were found on this system"
    say "  sdrtop will need root to open the radio until they exist. They come"
    say "  with the libhackrf and rtl-sdr packages on most distributions; see"
    say "  user_docs/troubleshooting.md if installing those did not supply any."
fi

if ! id -nG 2>/dev/null | tr ' ' '\n' | grep -qx plugdev; then
    if getent group plugdev >/dev/null 2>&1; then
        say ""
        say "You are not in the plugdev group, which those rules usually grant"
        say "access through. To join it:"
        say "    sudo usermod -aG plugdev $(id -un)"
        say "then log out and back in, because group membership only applies to"
        say "new sessions."
    fi
fi

# ── Say what happened ───────────────────────────────────────────────────────
step "Done"
if "$BIN_DIR/sdrtop" --version >/dev/null 2>&1; then
    say "$("$BIN_DIR/sdrtop" --version) installed in $BIN_DIR"
else
    die "$BIN_DIR/sdrtop was installed but will not run"
fi

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) say ""; say "$BIN_DIR is not on your PATH. Add it:"
       say "    export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac

say ""
say "Run 'sdrtop'. It opens on its menu: Enter takes a layout, Space starts"
say "receiving, Esc brings the menu back, q quits and saves."
