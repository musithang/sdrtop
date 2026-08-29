#!/bin/sh
# Print the CHANGELOG.md section for one version, for use as a release body.
#
#   packaging/release-notes.sh 0.4.1
#
# `gh release create --generate-notes` produces a list of commit subjects, which
# is a log rather than release notes. It cannot say "RadioText never arrived and
# now does", because no commit subject says that. A human writes the changelog;
# this reads it back.
#
# Exits non-zero when the version has no section, and that is the point: a
# release whose contents nobody wrote down should not be created at all.
set -eu

[ $# -eq 1 ] || {
    echo "usage: release-notes.sh VERSION" >&2
    exit 2
}
version=$1
changelog="$(cd "$(dirname "$0")/.." && pwd)/CHANGELOG.md"

# The heading for this version opens the section. Two things close it, and the
# second is not optional: the next version heading, or the block of link
# reference definitions at the foot of the file. The oldest version is the last
# section, so nothing follows it but those links, and without that second rule
# its release body ends with a list of bare compare URLs.
#
# `index(...) == 1` anchors the heading test to the start of the line, so a
# `## [` inside prose cannot end a section early.
#
# Blank lines are buffered rather than printed: interior ones are flushed once
# the next real line arrives, leading ones are dropped because nothing has been
# seen yet, and trailing ones are never flushed at all. That trims the section
# without a second pass.
notes=$(awk -v v="$version" '
    index($0, "## [" v "]") == 1 { inside = 1; next }
    inside && index($0, "## [") == 1 { exit }
    inside && /^\[[^]]*\]:[[:space:]]/ { exit }
    inside {
        if ($0 ~ /^[[:space:]]*$/) { blanks++; next }
        if (seen) { for (i = 0; i < blanks; i++) print "" }
        blanks = 0
        seen = 1
        print
    }
' "$changelog")

[ -n "$notes" ] || {
    echo "release-notes.sh: no \"## [$version]\" section in $changelog" >&2
    exit 1
}

printf '%s\n' "$notes"
