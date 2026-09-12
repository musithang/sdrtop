#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
set -eu

die() { printf '%s\n' "$*" >&2; exit 1; }

[ -z "${DOCS_RS+x}" ] || die "DOCS_RS must be unset"
[ -z "${LD_LIBRARY_PATH+x}" ] || die "LD_LIBRARY_PATH must be unset"
[ -z "${LIBRARY_PATH+x}" ] || die "LIBRARY_PATH must be unset"
[ -z "${RUSTFLAGS+x}" ] || die "RUSTFLAGS must be unset"
[ -z "${CARGO_ENCODED_RUSTFLAGS+x}" ] || die "CARGO_ENCODED_RUSTFLAGS must be unset"

if ldconfig -p | grep -Eq 'lib(hackrf|rtlsdr|SoapySDR)\.so'; then
    die "The smoke environment must have no SDR runtime libraries"
fi
if find /usr/lib /usr/local/lib /lib /usr/include /usr/local/include \
    \( -name 'libhackrf.so*' -o -name 'librtlsdr.so*' -o -name 'libSoapySDR.so*' \
       -o -name hackrf.h -o -name rtl-sdr.h \) -print | grep -q .; then
    die "The smoke environment must have no SDR libraries or headers"
fi

cargo build --locked
cargo test --locked
bin="${CARGO_TARGET_DIR:-target}/debug/sdrtop"
needed=$(readelf -d "$bin")
if printf '%s\n' "$needed" | grep -Eq 'NEEDED.*lib(hackrf|rtlsdr)'; then
    die "The binary requires a native SDR library"
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
# The CLI reads HOME/.config. Cargo and rustup must use the original HOME
export HOME="$work"
"$bin" --help >/dev/null
"$bin" --version

expect_failure() {
    if "$bin" "$@" >"$work/output" 2>&1; then
        die "Unexpected startup success: $*"
    else
        status=$?
    fi
    [ "$status" -eq 1 ] || die "Unexpected startup exit $status: $*"
}
expect_text() {
    grep -Fq "$1" "$work/output" || {
        cat "$work/output" >&2
        die "Missing startup diagnostic: $1"
    }
}

# These package and version assertions keep installation advice actionable
expect_failure --device hackrf
expect_text "libhackrf backend unavailable"
expect_text "2023.01.1"
expect_text "libhackrf0"
expect_failure --device rtlsdr
expect_text "librtlsdr backend unavailable"
expect_text "librtlsdr0"
expect_text "librtlsdr2"
expect_failure
expect_text "No device found"
expect_text "libhackrf backend unavailable"
expect_text "librtlsdr backend unavailable"
for backend in tinysa soapy; do
    expect_failure --device "$backend"
    expect_text "No device found"
    if grep -Eq 'libhackrf|librtlsdr' "$work/output"; then
        die "Irrelevant native diagnostic for $backend"
    fi
done
printf '%s\n' "No-native-library startup checks passed"
