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

VERSION=$("$REPO/packaging/version.sh")

# The full Rust target triple, not `x86_64-linux`. The short form does not say
# which libc, and that is precisely the axis this project is known to have
# trouble on. It also means a musl or aarch64 build can be added later as a new
# name rather than by breaking this one and every install.sh already in the
# wild. See dev_docs/release-process-plan.md.
NAME="sdrtop-${VERSION}-x86_64-unknown-linux-gnu"

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

exec "$CONTAINER" run --rm --platform linux/amd64 $CONTAINER_USER \
    -v "$REPO:/src" -w /src -e NAME="$NAME" -e SDRTOP_COMMIT="$COMMIT" \
    -e SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" -e VERSION="$VERSION" \
    "$IMAGE" sh -eu -c '
    # Out of the way of the host tree, so a container build never leaves the
    # working copy in a state a later host build inherits.
    export CARGO_TARGET_DIR=/src/target/container
    cargo build --release --target x86_64-unknown-linux-gnu

    OUT=/src/target/tarball/$NAME
    rm -rf "$OUT"; mkdir -p "$OUT"
    cp target/container/x86_64-unknown-linux-gnu/release/sdrtop "$OUT/"

    # Reading the ELF headers proves what the binary asks of a machine; this
    # proves it runs. Tested here, in the staging directory, so the file that
    # gets exercised is the exact one that goes into the archive rather than the
    # one it was copied from.
    #
    # It has to happen inside the container: Debian 12 is the only place this
    # binary is expected to start, and on the Ubuntu CI runner it fails for the
    # librtlsdr soname reason that install.sh exists to handle. Both flags
    # return before any device is opened, so neither needs a radio.
    reported=$("$OUT/sdrtop" --version)
    echo "$reported"
    "$OUT/sdrtop" --help >/dev/null

    # And it has to be *this* version. Nothing else in the pipeline compares the
    # binary against the name on the archive, so a stale CARGO_TARGET_DIR could
    # ship an older build under the current version and no check would notice.
    # release.yaml separately asserts the tag against Cargo.toml, and $VERSION
    # came from version.sh reading that same file, so the two together chain the
    # tag all the way to the binary.
    case "$reported" in
        "sdrtop $VERSION" | "sdrtop $VERSION "*) ;;
        *)
            echo "the binary reports \"$reported\", not sdrtop $VERSION" >&2
            exit 1
            ;;
    esac
    cp README.md LICENSE "$OUT/"
    cp target/man/sdrtop.1 "$OUT/"
    # install.sh travels with the tarball as well as being the one-liner on the
    # release page, so an unpacked tarball can install itself offline.
    cp packaging/install.sh "$OUT/"
    # Only the markdown, not user_docs/pics: that carries screenshots and a
    # 16 MB screen recording, which turned a 1.5 MB tarball into an 18 MB one.
    mkdir -p "$OUT/user_docs"
    cp user_docs/*.md "$OUT/user_docs/"

    # Cleared, not just created. release.yaml collects the release assets with
    # `cp target/dist/*.tar.gz`, so anything left here from an earlier build
    # ships alongside this one: rename the asset, as R5 just did, and a local
    # run leaves both the old and the new name sitting in the glob. A fresh CI
    # runner never sees it, which is exactly what makes it worth closing here.
    # target/ is build output, so this deletes nothing that cannot be rebuilt.
    rm -rf /src/target/dist
    mkdir -p /src/target/dist
    # Deterministic, so the same commit gives the same checksum and anyone can
    # rebuild this and compare. `tar -czf` is not: it records each file mtime
    # (which `cp` set to now) and walks the directory in filesystem order, and
    # gzip stamps its own header with the current time. Each flag kills one of
    # those sources of variance:
    #
    #   --sort=name     archive order stops depending on the filesystem
    #   --mtime         every entry gets the commit date, not the copy time
    #   --owner/--group whoever ran the build stops being recorded
    #   --format=gnu    explicit, since pax would add extended time headers
    #   gzip -n         no timestamp, and no original filename, in the header
    #
    # Two steps rather than a pipeline: this is dash, which has no pipefail, so
    # a failing tar in a pipe would be invisible. `set -e` catches both of these.
    tar --sort=name --format=gnu \
        --mtime="@$SOURCE_DATE_EPOCH" \
        --owner=0 --group=0 --numeric-owner \
        -C /src/target/tarball -cf "/src/target/dist/$NAME.tar" "$NAME"
    gzip -9n "/src/target/dist/$NAME.tar"
    echo "/src/target/dist/$NAME.tar.gz"
'
