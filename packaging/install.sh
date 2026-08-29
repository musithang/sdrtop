#!/bin/sh
# Install sdrtop on any Linux distribution.
#
#   curl -fsSL https://raw.githubusercontent.com/mustang6139/sdrtop/main/packaging/install.sh | sh
#   sh install.sh --prefix ~/.local        # no root anywhere
#   sh install.sh --from-source            # skip the prebuilt binary
#   sh install.sh --uninstall
#
# One prebuilt tarball is published, for x86_64 glibc. Everything else this
# script handles by building from source, and it decides which it is by asking
# rather than by guessing: it unpacks the binary and tries to run it, and falls
# back to a source build if that fails for any reason at all.
#
# That fallback is the whole design. sdrtop links `librtlsdr`, and Debian ships
# it as `librtlsdr.so.0` while Ubuntu ships the same upstream as `.so.2`, so no
# single prebuilt binary can serve both. Running the binary catches that, and
# also catches musl, a missing loader and every future soname bump, none of
# which a list of distribution names would have covered.
set -eu

REPO=mustang6139/sdrtop
RAW_DEPS_ONLY=0
FROM_SOURCE=0
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
  --version TAG   install a specific release, e.g. v0.4.0 (default: the latest)
  --from-source   build from source even on x86_64
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
        --deps-only)   RAW_DEPS_ONLY=1; shift ;;
        --uninstall)   UNINSTALL=1; shift ;;
        --help|-h)     usage; exit 0 ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

[ "$(uname -s)" = Linux ] || die "sdrtop is Linux only; this is $(uname -s)"

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
case "$PM" in
    apt-get)
        LIB_HACKRF="libhackrf0 libhackrf-dev"
        LIB_RTLSDR="librtlsdr0 librtlsdr2 librtlsdr-dev"
        DEV_PKGS="libhackrf-dev librtlsdr-dev pkg-config build-essential" ;;
    dnf|yum)
        LIB_HACKRF="hackrf hackrf-devel"
        LIB_RTLSDR="rtl-sdr rtl-sdr-devel"
        DEV_PKGS="hackrf-devel rtl-sdr-devel pkgconf-pkg-config gcc" ;;
    pacman)
        LIB_HACKRF="hackrf"
        LIB_RTLSDR="rtl-sdr"
        DEV_PKGS="hackrf rtl-sdr pkgconf base-devel" ;;
    zypper)
        LIB_HACKRF="libhackrf0 hackrf libhackrf-devel"
        LIB_RTLSDR="librtlsdr0 rtl-sdr rtl-sdr-devel"
        DEV_PKGS="libhackrf-devel rtl-sdr-devel pkg-config gcc" ;;
    apk)
        # Alpine splits further than anyone else: the library, the headers and
        # the udev rules are three packages.
        LIB_HACKRF="hackrf-libs hackrf-dev"
        LIB_RTLSDR="librtlsdr librtlsdr-dev"
        UDEV_PKGS="hackrf-udev librtlsdr-udev"
        DEV_PKGS="hackrf-dev librtlsdr-dev pkgconf build-base" ;;
    xbps-install)
        LIB_HACKRF="hackrf hackrf-devel"
        LIB_RTLSDR="rtl-sdr rtl-sdr-devel"
        DEV_PKGS="hackrf-devel rtl-sdr-devel pkg-config base-devel" ;;
    emerge)
        LIB_HACKRF="net-wireless/hackrf"
        LIB_RTLSDR="net-wireless/rtl-sdr"
        DEV_PKGS="net-wireless/hackrf net-wireless/rtl-sdr" ;;
    nix-env)
        LIB_HACKRF="hackrf"
        LIB_RTLSDR="rtl-sdr"
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
    # Where the rules are their own package, take them: otherwise the radio
    # installs fine and then needs root to open.
    for p in $UDEV_PKGS; do pm_install "$p" >/dev/null 2>&1 && say "  $p" || true; done
}

install_runtime_deps
[ "$RAW_DEPS_ONLY" -eq 1 ] && exit 0

BINARY=""
SRC_DIR=""

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

# ── Which release ───────────────────────────────────────────────────────────
if [ -z "$BINARY" ]; then
fetch() { curl -fsSL "$1" -o "$2"; }
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar  >/dev/null 2>&1 || die "tar is required"

if [ -z "$TAG" ]; then
    step "Finding the latest release"
    # No jq: the API's own field, first match, nothing else on the line.
    TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
          | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
    [ -n "$TAG" ] || die "could not determine the latest release; pass --version"
fi
VERSION="${TAG#v}"
say "sdrtop $TAG"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

# ── The prebuilt binary, if it can possibly work here ───────────────────────
ARCH=$(uname -m)

if [ "$FROM_SOURCE" -eq 0 ] && [ "$ARCH" = x86_64 ]; then
    step "Downloading the prebuilt binary"
    NAME="sdrtop-$VERSION-x86_64-linux"
    BASE="https://github.com/$REPO/releases/download/$TAG"
    if fetch "$BASE/$NAME.tar.gz" "$WORK/$NAME.tar.gz"; then
        # The checksums are published beside the tarball; a download that does
        # not match one is not unpacked.
        if fetch "$BASE/SHA256SUMS" "$WORK/SHA256SUMS" \
           && command -v sha256sum >/dev/null 2>&1; then
            ( cd "$WORK" && grep " $NAME.tar.gz\$" SHA256SUMS | sha256sum -c - ) \
                || die "checksum mismatch on $NAME.tar.gz"
        else
            warn "could not verify the checksum"
        fi
        tar -xzf "$WORK/$NAME.tar.gz" -C "$WORK"

        # The honest test, and the reason this script needs no distribution
        # list: run the thing. `--version` returns before any device is opened,
        # so it needs no radio, and it fails for every reason that matters here.
        #
        # Running it beats reading `ldd` output. On musl `ldd` is the musl
        # loader, which cannot load a glibc binary at all and says so without
        # ever printing "not found", so a grep for that phrase would conclude
        # the binary was fine and install something that cannot start.
        if "$WORK/$NAME/sdrtop" --version >/dev/null 2>&1; then
            BINARY="$WORK/$NAME/sdrtop"
            SRC_DIR="$WORK/$NAME"
        else
            say "the prebuilt binary does not run on this system:"
            ldd "$WORK/$NAME/sdrtop" 2>&1 | grep -E 'not found|Error|error' \
                | sed 's/^/    /' || say "    (it failed to start)"
            say "building from source instead, which links what you do have"
        fi
    else
        warn "no prebuilt tarball for $TAG; building from source"
    fi
fi
fi # end of the download path, skipped entirely for a local tarball

# ── Or build it ─────────────────────────────────────────────────────────────
if [ -z "$BINARY" ]; then
    step "Building from source"
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
        . "$HOME/.cargo/env"
    fi

    say "fetching the sources for $TAG"
    # Into a directory of its own. The failed prebuilt tarball is still unpacked
    # in $WORK as `sdrtop-<version>-x86_64-linux`, and a search for `sdrtop-*`
    # across $WORK finds that first and tries to build a directory that has no
    # Cargo.toml in it.
    mkdir -p "$WORK/src"
    fetch "https://github.com/$REPO/archive/refs/tags/$TAG.tar.gz" "$WORK/src.tar.gz" \
        || die "could not download the sources for $TAG"
    tar -xzf "$WORK/src.tar.gz" -C "$WORK/src"
    # Identified by what a build actually needs rather than by its name, so the
    # archive's top-level directory can be called anything.
    SRC_DIR=$(find "$WORK/src" -maxdepth 2 -name Cargo.toml -type f | head -1)
    SRC_DIR=${SRC_DIR%/Cargo.toml}
    [ -n "$SRC_DIR" ] && [ -d "$SRC_DIR" ] || die "no Cargo.toml in the source archive for $TAG"

    say "compiling (this takes a few minutes)"
    ( cd "$SRC_DIR" && cargo build --release ) || die "the build failed; see the output above"
    BINARY="$SRC_DIR/target/release/sdrtop"
    [ -f "$SRC_DIR/target/man/sdrtop.1" ] && cp "$SRC_DIR/target/man/sdrtop.1" "$SRC_DIR/sdrtop.1"
fi

# ── Install ─────────────────────────────────────────────────────────────────
step "Installing into $PREFIX"
as_root install -Dm755 "$BINARY" "$BIN_DIR/sdrtop"
say "  $BIN_DIR/sdrtop"
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

# ── The radio has to be openable ────────────────────────────────────────────
# The library packages normally ship the udev rules, so this only writes any
# when the system has none. The IDs are the HackRF One and the two generic
# RTL2832U dongles, copied from the rules those packages install; the full
# osmocom list covers another forty variants.
step "Device access"
if [ -n "$SUDO" ] || [ "$(id -u)" -eq 0 ]; then
    if grep -rqs plugdev /usr/lib/udev/rules.d /lib/udev/rules.d /etc/udev/rules.d; then
        say "udev rules for SDR hardware are already installed"
    else
        say "no SDR udev rules found, writing a minimal set"
        as_root sh -c 'cat > /etc/udev/rules.d/60-sdrtop.rules' <<'RULES'
# Minimal fallback rules, written by the sdrtop installer only because no
# libhackrf or librtlsdr package had supplied any. Replace with your
# distribution's own package rules if you install them later.
SUBSYSTEM=="usb", ATTR{idVendor}=="1d50", ATTR{idProduct}=="6089", MODE="0660", GROUP="plugdev"
SUBSYSTEM=="usb", ATTR{idVendor}=="1d50", ATTR{idProduct}=="604b", MODE="0660", GROUP="plugdev"
SUBSYSTEMS=="usb", ATTRS{idVendor}=="0bda", ATTRS{idProduct}=="2832", MODE="0660", GROUP="plugdev"
SUBSYSTEMS=="usb", ATTRS{idVendor}=="0bda", ATTRS{idProduct}=="2838", MODE="0660", GROUP="plugdev"
RULES
        as_root udevadm control --reload-rules 2>/dev/null || true
        as_root udevadm trigger 2>/dev/null || true
    fi
fi

NEEDS_GROUP=0
if ! id -nG 2>/dev/null | tr ' ' '\n' | grep -qx plugdev; then
    NEEDS_GROUP=1
    if [ -n "$SUDO" ] || [ "$(id -u)" -eq 0 ]; then
        getent group plugdev >/dev/null 2>&1 || as_root groupadd plugdev
        as_root usermod -aG plugdev "$(id -un)" && say "added $(id -un) to plugdev"
    else
        warn "you are not in the plugdev group and this run cannot add you"
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

if [ "$NEEDS_GROUP" -eq 1 ]; then
    say ""
    say "Log out and back in before plugging the radio in: group membership"
    say "only applies to new sessions."
fi
say ""
say "Run 'sdrtop', press Space to start receiving and ? for the keys."
