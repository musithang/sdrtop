#!/bin/sh
# Build the release tarball: the binary, the man page and the docs.
#
#   packaging/build-tarball.sh
#
# Built in a Debian 12 container rather than on the host, so the binary carries
# the oldest glibc floor worth supporting instead of whatever the build machine
# happens to have. See packaging/Containerfile.
#
# x86_64 only, on purpose. Every other architecture, and every distribution
# whose librtlsdr soname does not match, is served by packaging/install.sh
# building from source, which links what the target machine actually has.
set -eu

REPO="$(cd "$(dirname "$0")/.." && pwd)"
. "$REPO/packaging/_container.sh"

IMAGE=sdrtop-build
if ! "$CONTAINER" image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "building the $IMAGE image" >&2
    "$CONTAINER" build --platform linux/amd64 \
        -t "$IMAGE" -f "$REPO/packaging/Containerfile" "$REPO"
fi

VERSION=$("$CONTAINER" run --rm $CONTAINER_USER -v "$REPO:/src" -w /src "$IMAGE" \
    sh -c 'cargo metadata --no-deps --format-version 1 | sed -n "s/.*\"version\":\"\([^\"]*\)\".*/\1/p" | head -1')
NAME="sdrtop-${VERSION}-x86_64-linux"

exec "$CONTAINER" run --rm --platform linux/amd64 $CONTAINER_USER \
    -v "$REPO:/src" -w /src -e NAME="$NAME" \
    "$IMAGE" sh -eu -c '
    # Out of the way of the host tree, so a container build never leaves the
    # working copy in a state a later host build inherits.
    export CARGO_TARGET_DIR=/src/target/container
    cargo build --release --target x86_64-unknown-linux-gnu

    # Reading the ELF headers proves what the binary asks for; this proves it
    # starts. It has to happen in here, because Debian 12 is the only place it
    # is expected to run: on the CI runner, which is Ubuntu, it would fail for
    # the librtlsdr soname reason that install.sh exists to handle.
    # `--version` returns before any device is opened, so it needs no radio.
    target/container/x86_64-unknown-linux-gnu/release/sdrtop --version

    OUT=/src/target/tarball/$NAME
    rm -rf "$OUT"; mkdir -p "$OUT"
    cp target/container/x86_64-unknown-linux-gnu/release/sdrtop "$OUT/"
    cp README.md LICENSE "$OUT/"
    cp target/man/sdrtop.1 "$OUT/"
    # install.sh travels with the tarball as well as being the one-liner on the
    # release page, so an unpacked tarball can install itself offline.
    cp packaging/install.sh "$OUT/"
    # Only the markdown, not user_docs/pics: that carries screenshots and a
    # 16 MB screen recording, which turned a 1.5 MB tarball into an 18 MB one.
    mkdir -p "$OUT/user_docs"
    cp user_docs/*.md "$OUT/user_docs/"

    mkdir -p /src/target/dist
    tar -C /src/target/tarball -czf "/src/target/dist/$NAME.tar.gz" "$NAME"
    echo "/src/target/dist/$NAME.tar.gz"
'
