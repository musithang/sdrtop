#!/bin/sh

# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# Stage a built binary with the man page and the docs, check it, and pack it
# into a deterministic tarball in target/dist.
#
#   packaging/pack-tarball.sh NAME BINARY
#
# build-tarball.sh calls this once the binary is built: inside the Debian
# container for Linux, on the runner itself for macOS. One copy of what goes
# into a release and how it is packed, so the two systems' tarballs cannot
# drift apart.
#
# Needs VERSION and SOURCE_DATE_EPOCH in the environment, and GNU tar: TAR
# names it where it is not `tar` (macOS ships bsdtar, Homebrew's is `gtar`).
set -eu

[ $# -eq 2 ] || {
    echo "usage: pack-tarball.sh NAME BINARY" >&2
    exit 2
}
NAME=$1
BINARY=$2
REPO="$(cd "$(dirname "$0")/.." && pwd)"
TAR=${TAR:-tar}
: "${VERSION:?VERSION must be set}"
: "${SOURCE_DATE_EPOCH:?SOURCE_DATE_EPOCH must be set}"

# Checked before anything is staged. bsdtar takes most of the same flags and
# quietly ignores the ones that make the archive reproducible.
"$TAR" --version 2>/dev/null | grep -q 'GNU tar' || {
    echo "pack-tarball.sh: $TAR is not GNU tar; set TAR (on macOS: brew install gnu-tar, TAR=gtar)" >&2
    exit 2
}

OUT=$REPO/target/tarball/$NAME
rm -rf "$OUT"; mkdir -p "$OUT"
cp "$BINARY" "$OUT/"

# Reading the binary's headers proves what it asks of a machine; this proves
# it runs. Tested here, in the staging directory, so the file that gets
# exercised is the exact one that goes into the archive rather than the one it
# was copied from.
reported=$("$OUT/sdrtop" --version)
echo "$reported"
"$OUT/sdrtop" --help >/dev/null

# And it has to be *this* version. Nothing else in the pipeline compares the
# binary against the name on the archive, so a stale target directory could
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
cp "$REPO/README.md" "$REPO/LICENSE" "$OUT/"
cp "$REPO/target/man/sdrtop.1" "$OUT/"
# install.sh travels with the tarball as well as being the one-liner on the
# release page, so an unpacked tarball can install itself offline.
cp "$REPO/packaging/install.sh" "$OUT/"
# Only the markdown, not user_docs/pics: that carries screenshots and a
# 16 MB screen recording, which turned a 1.5 MB tarball into an 18 MB one.
mkdir -p "$OUT/user_docs"
cp "$REPO"/user_docs/*.md "$OUT/user_docs/"

# Cleared, not just created. release.yaml collects the release assets with
# `target/dist/*.tar.gz`, so anything left here from an earlier build ships
# alongside this one: rename the asset and a local run leaves both the old and
# the new name sitting in the glob. A fresh CI runner never sees it, which is
# exactly what makes it worth closing here. target/ is build output, so this
# deletes nothing that cannot be rebuilt.
rm -rf "$REPO/target/dist"
mkdir -p "$REPO/target/dist"
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
# Two steps rather than a pipeline: this is a POSIX sh with no pipefail, so a
# failing tar in a pipe would be invisible. `set -e` catches both of these.
"$TAR" --sort=name --format=gnu \
    --mtime="@$SOURCE_DATE_EPOCH" \
    --owner=0 --group=0 --numeric-owner \
    -C "$REPO/target/tarball" -cf "$REPO/target/dist/$NAME.tar" "$NAME"
gzip -9n "$REPO/target/dist/$NAME.tar"
echo "$REPO/target/dist/$NAME.tar.gz"
