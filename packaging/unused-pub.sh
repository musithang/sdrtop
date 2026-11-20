#!/bin/sh

# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# The `pub` items of the engine's crates that nothing uses.
#
#   packaging/unused-pub.sh
#
# In a binary an unused function is a warning, and `clippy -D warnings` kept
# the tree at none. In a library a `pub` item is part of what the crate offers,
# so the compiler stays silent about it even when its last caller is gone. The
# monas crates are libraries, so this looks instead: every `pub` fn, struct,
# enum, trait, type, const, static or union declared under crates/monas*/src
# whose name appears on no line of code in the workspace but the one that
# declares it.
#
# Advisory, and said so: it counts names, not paths, so a name that is also
# used for something else (`new`, `len`, a field) is never reported, and a
# name mentioned only in a string is counted as used. What it lists is worth
# a look; what it does not list is not proven alive. It always exits 0.
#
# Run at each release (RELEASING.md); read the list, and either delete what
# is dead or make it `pub(crate)` again where only its own crate uses it.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

# Every identifier on every line of code in the workspace, comments taken off,
# counted once per line it stands on.
find crates -path '*/src/*' -name '*.rs' -exec cat {} + \
    | sed -e 's://.*$::' \
    | awk '{
        delete seen
        n = split($0, words, /[^A-Za-z0-9_]+/)
        for (i = 1; i <= n; i++)
            if (words[i] ~ /^[A-Za-z_]/ && !(words[i] in seen)) { seen[words[i]] = 1; print words[i] }
      }' \
    | sort | uniq -c | awk '{ print $2, $1 }' > "$work/lines"

# Every `pub` item of the monas crates: name, kind, where.
find crates -path '*/src/*' -name '*.rs' | grep -E '^crates/monas[^/]*/' | sort \
    | while IFS= read -r file; do
        grep -nE '^[[:space:]]*pub (unsafe |async |const |extern "C" )*(fn|struct|enum|trait|type|const|static|union) [A-Za-z_]' "$file" \
            | sed -E 's/^([0-9]+):[[:space:]]*pub (unsafe |async |const |extern "C" )*(fn|struct|enum|trait|type|const|static|union) ([A-Za-z_][A-Za-z0-9_]*).*/\4 \3 \1/' \
            | while read -r name kind line; do printf '%s %s %s:%s\n' "$name" "$kind" "$file" "$line"; done
    done > "$work/decls"

# A name used on no more lines than it is declared on is used nowhere else.
awk 'NR == FNR { lines[$1] = $2; next }
     { declared[$1]++; rows[NR] = $0; names[NR] = $1 }
     END {
        for (r in rows) if (lines[names[r]] <= declared[names[r]]) print rows[r]
     }' "$work/lines" "$work/decls" | sort -k3 > "$work/unused"

total=$(wc -l < "$work/decls" | tr -d ' ')
found=$(wc -l < "$work/unused" | tr -d ' ')
printf '%s of %s pub items in the monas crates are used nowhere but where they are declared:\n\n' "$found" "$total"
awk '{ printf "  %-40s %-7s %s\n", $1, $2, $3 }' "$work/unused"
