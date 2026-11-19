#!/bin/sh

# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# The one rule for what version sdrtop is.
#
#   packaging/version.sh     ->   0.4.1
#
# There used to be two rules. release.yaml read Cargo.toml with sed, while
# build-tarball.sh started a container to run `cargo metadata` and then applied
# a regex to the resulting JSON, keeping the first "version" field it saw. Two
# answers to one question drift apart, and the second one also depended on
# cargo's field ordering and cost a container start to read a file.
#
# Anything that needs the version calls this. Nothing parses Cargo.toml twice.
set -eu

MANIFEST="$(cd "$(dirname "$0")/.." && pwd)/Cargo.toml"

# Anchored to the start of the line, so a dependency's own `version = "..."`
# cannot match: those are indented or inline, the internal crates' exact pins
# included. `head -1` then takes [workspace.package]'s, the one version every
# crate of the workspace inherits, and the first table that has one.
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$MANIFEST" | head -1)

[ -n "$version" ] || {
    echo "version.sh: no package version found in $MANIFEST" >&2
    exit 1
}

printf '%s\n' "$version"
