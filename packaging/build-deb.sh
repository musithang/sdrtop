#!/bin/sh
# Build one architecture's .deb in the Debian 12 build image.
#
#   packaging/build-deb.sh amd64 | arm64 | armhf
#
# Cross-compiled rather than emulated. QEMU would be simpler to reason about but
# needs `qemu-user-static` and binfmt registered on the *host*; this needs
# nothing outside the container. The usual warning about cross builds - that
# they silently pick up the host's libraries - does not apply here: an amd64
# `libhackrf.so` handed to the aarch64 linker is rejected outright, so a mistake
# is a failed build rather than a broken package. `verify-deb.sh` checks the
# output's machine type regardless.
set -eu

ARCH="${1:?usage: build-deb.sh amd64|arm64|armhf}"
IMAGE="sdrtop-deb-$ARCH"

case "$ARCH" in
    amd64) TRIPLE=x86_64-unknown-linux-gnu;      GNU=x86_64-linux-gnu ;;
    arm64) TRIPLE=aarch64-unknown-linux-gnu;     GNU=aarch64-linux-gnu ;;
    armhf) TRIPLE=armv7-unknown-linux-gnueabihf; GNU=arm-linux-gnueabihf ;;
    *) echo "unknown architecture: $ARCH" >&2; exit 2 ;;
esac

REPO="$(cd "$(dirname "$0")/.." && pwd)"
. "$REPO/packaging/_container.sh"


# One image per architecture; the expensive layers are shared through podman's
# cache, so only the first build pays for rustup and cargo-deb.
if ! "$CONTAINER" image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "building the $IMAGE image" >&2
    "$CONTAINER" build --platform linux/amd64 --build-arg "DEB_ARCH=$ARCH" \
        -t "$IMAGE" -f "$REPO/packaging/Containerfile" "$REPO"
fi

exec "$CONTAINER" run --rm --platform linux/amd64 $CONTAINER_USER \
    -v "$REPO:/src" -w /src \
    -e TRIPLE="$TRIPLE" -e GNU="$GNU" \
    "$IMAGE" sh -eu -c '
    # pkg-config must answer for the *target*, not the build host.
    export PKG_CONFIG_ALLOW_CROSS=1
    export PKG_CONFIG_LIBDIR="/usr/lib/$GNU/pkgconfig:/usr/share/pkgconfig"
    export PKG_CONFIG_PATH="$PKG_CONFIG_LIBDIR"
    linker_var=$(echo "CARGO_TARGET_${TRIPLE}_LINKER" | tr "a-z-" "A-Z_")
    export "$linker_var=${GNU}-gcc"

    # Out of the way of the host tree, so a container build never leaves the
    # working copy in a state a later host build inherits.
    export CARGO_TARGET_DIR=/src/target/container

    cargo deb --target "$TRIPLE" --output /src/target/debian/
'
