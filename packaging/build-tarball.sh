#!/bin/sh

# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# Build the release tarball: the binary, the man page and the docs.
#
#   packaging/build-tarball.sh
#
# On Linux, built in a Debian 12 container rather than on the host, so the
# binary carries the oldest glibc floor worth supporting instead of whatever
# the build machine happens to have. See packaging/Containerfile.
#
# On macOS, built on the machine itself, for its own architecture: Apple's
# toolchain sets the oldest macOS the binary runs on, so there is no floor to
# keep with a container. release.yaml runs it on an Apple Silicon and an
# Intel runner, and each binary is run where it was built.
#
# What goes into the tarball and how it is packed is packaging/pack-tarball.sh,
# shared by both.
#
# packaging/install.sh builds from source for other architectures or libcs
set -eu

REPO="$(cd "$(dirname "$0")/.." && pwd)"

VERSION=$("$REPO/packaging/version.sh")

# The commit is resolved here, on the host, and handed to build.rs through the
# environment. The container has no git, and bind-mounting .git into it would
# trip git's safe.directory check when the uid does not match. See build.rs.
COMMIT=$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo "")
if [ -n "$COMMIT" ] && ! git -C "$REPO" diff --quiet HEAD 2>/dev/null; then
    COMMIT="$COMMIT-dirty"
    echo "warning: building a release tarball from a dirty tree" >&2
fi

# The commit's own date, which is the one timestamp about this build that is a
# property of the source rather than of when someone happened to run this. Every
# mtime in the archive is forced to it below, so two builds of one commit produce
# one checksum. 0 when there is no git: still deterministic, and a build with no
# commit was never going to be reproducible anyway.
SOURCE_DATE_EPOCH=$(git -C "$REPO" log -1 --format=%ct 2>/dev/null || echo 0)

if [ "$(uname -s)" = Darwin ]; then
    case "$(uname -m)" in
        arm64) TRIPLE=aarch64-apple-darwin ;;
        x86_64) TRIPLE=x86_64-apple-darwin ;;
        *) echo "no macOS release for $(uname -m)" >&2; exit 2 ;;
    esac
    NAME="sdrtop-${VERSION}-${TRIPLE}"
    # A target directory of its own, for the same reason the container has
    # one: a host build never inherits it.
    CARGO_TARGET_DIR="$REPO/target/release-$TRIPLE" SDRTOP_COMMIT="$COMMIT" \
        cargo build --release --locked --target "$TRIPLE" \
        --manifest-path "$REPO/Cargo.toml"
    BIN="$REPO/target/release-$TRIPLE/$TRIPLE/release/sdrtop"
    # Signed ad hoc, on both architectures. Apple Silicon runs nothing without
    # a signature and its linker adds one, but the Intel linker does not, and
    # one rule for both is easier to check than two. An ad-hoc signature is a
    # hash of the file with no certificate and no timestamp, so it costs no
    # reproducibility; it is not a developer signature, and says nothing about
    # who built the file. The attestation on the release says that.
    codesign --sign - --force "$BIN"
    export VERSION SOURCE_DATE_EPOCH
    exec sh "$REPO/packaging/pack-tarball.sh" "$NAME" "$BIN"
fi

# Podman first, then docker. This used to live in a _container.sh that two build
# scripts shared; there is one left, so it lives here. Set CONTAINER to override
# on a machine where the wrong one is first on PATH.
#
# The GitHub runner has both and therefore picks podman, which is worth knowing:
# a workflow step that reaches for `docker` directly cannot see an image this
# script built. That mistake cost a release build once.
if [ -z "${CONTAINER:-}" ]; then
    if command -v podman >/dev/null 2>&1; then
        CONTAINER=podman
    elif command -v docker >/dev/null 2>&1; then
        CONTAINER=docker
    else
        echo "no container runtime: install podman or docker" >&2
        exit 2
    fi
fi

# Rootless podman maps the container's root to the invoking user, so files land
# owned by whoever ran the build. Docker does not: without this, every artefact
# comes back owned by root and the next local build cannot overwrite it.
CONTAINER_USER=""
if [ "$CONTAINER" = docker ]; then
    CONTAINER_USER="--user $(id -u):$(id -g)"
fi

IMAGE=sdrtop-build

# Unconditionally, and that is the fix rather than the cost. This used to be
# guarded by `image inspect`, which meant an edited Containerfile was ignored on
# any machine that already had an image: you got a tarball built by the old
# recipe with nothing on screen to say so. CI never noticed, because a fresh
# runner has no image to skip. The layer cache makes a no-change rebuild nearly
# instant, which is what the layer cache is for.
echo "building the $IMAGE image" >&2
"$CONTAINER" build --platform linux/amd64 \
    -t "$IMAGE" -f "$REPO/packaging/Containerfile" "$REPO"

# The full Rust target triple, not `x86_64-linux`. The short form does not say
# which libc, and that is precisely the axis this project is known to have
# trouble on. It also means a musl or aarch64 build can be added later as a new
# name rather than by breaking this one and every install.sh already in the
# wild.
NAME="sdrtop-${VERSION}-x86_64-unknown-linux-gnu"

exec "$CONTAINER" run --rm --platform linux/amd64 $CONTAINER_USER \
    -v "$REPO:/src" -w /src -e NAME="$NAME" -e SDRTOP_COMMIT="$COMMIT" \
    -e SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" -e VERSION="$VERSION" \
    "$IMAGE" sh -eu -c '
    # Out of the way of the host tree, so a container build never leaves the
    # working copy in a state a later host build inherits.
    export CARGO_TARGET_DIR=/src/target/container
    cargo build --release --target x86_64-unknown-linux-gnu
    # Tested at the supported glibc floor, without native SDR libraries.
    sh packaging/pack-tarball.sh "$NAME" \
        target/container/x86_64-unknown-linux-gnu/release/sdrtop
'
