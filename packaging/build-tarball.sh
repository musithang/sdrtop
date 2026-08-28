#!/bin/sh
# Build the plain x86_64 tarball: the binary, the man page and the docs, for
# people who do not want a package.
#
#   packaging/build-tarball.sh
#
# Built in the same Debian 12 image as the .debs, so it carries the same glibc
# floor. A tarball built on the host would need a newer glibc than the packages
# do, which is a confusing thing to hand someone as the "portable" option.
set -eu

REPO="$(cd "$(dirname "$0")/.." && pwd)"
. "$REPO/packaging/_container.sh"

IMAGE=sdrtop-deb-amd64
if ! "$CONTAINER" image inspect "$IMAGE" >/dev/null 2>&1; then
    "$CONTAINER" build --platform linux/amd64 --build-arg DEB_ARCH=amd64 \
        -t "$IMAGE" -f "$REPO/packaging/Containerfile" "$REPO"
fi

VERSION=$("$CONTAINER" run --rm $CONTAINER_USER -v "$REPO:/src" -w /src "$IMAGE" \
    sh -c 'cargo metadata --no-deps --format-version 1 | sed -n "s/.*\"version\":\"\([^\"]*\)\".*/\1/p" | head -1')
NAME="sdrtop-${VERSION}-x86_64-linux"

exec "$CONTAINER" run --rm --platform linux/amd64 $CONTAINER_USER \
    -v "$REPO:/src" -w /src -e NAME="$NAME" \
    "$IMAGE" sh -eu -c '
    export CARGO_TARGET_DIR=/src/target/container
    cargo build --release --target x86_64-unknown-linux-gnu

    OUT=/src/target/tarball/$NAME
    rm -rf "$OUT"; mkdir -p "$OUT"
    cp target/container/x86_64-unknown-linux-gnu/release/sdrtop "$OUT/"
    cp README.md LICENSE "$OUT/"
    cp target/man/sdrtop.1 "$OUT/"
    # Only the markdown, the same as the .deb ships: user_docs/pics carries
    # screenshots and a 16 MB screen recording, which turned a 1.5 MB tarball
    # into an 18 MB one.
    mkdir -p "$OUT/user_docs"
    cp user_docs/*.md "$OUT/user_docs/"

    mkdir -p /src/target/dist
    tar -C /src/target/tarball -czf "/src/target/dist/$NAME.tar.gz" "$NAME"
    echo "/src/target/dist/$NAME.tar.gz"
'
