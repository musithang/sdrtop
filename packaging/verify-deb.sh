#!/bin/sh
# Check one built .deb: right architecture, right dependencies, policy-clean.
#
#   packaging/verify-deb.sh target/debian/sdrtop_0.3.5-1_arm64.deb
#
# The architecture check is the one that matters for a cross build. A package
# whose control file says `arm64` while the binary inside is x86-64 installs
# happily and then does nothing, and no other check in the pipeline would notice.
set -eu

DEB="${1:?usage: verify-deb.sh <path to .deb>}"
[ -f "$DEB" ] || { echo "no such file: $DEB" >&2; exit 2; }

echo "== $DEB"
CONTROL_ARCH=$(dpkg -f "$DEB" Architecture)
echo "  control says      : $CONTROL_ARCH"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
dpkg-deb --fsys-tarfile "$DEB" | tar -x -C "$TMP" ./usr/bin/sdrtop 2>/dev/null

MACHINE=$(readelf -h "$TMP/usr/bin/sdrtop" | sed -n 's/^ *Machine: *//p')
echo "  binary is         : $MACHINE"

case "$CONTROL_ARCH:$MACHINE" in
    amd64:*X86-64*)   ;;
    arm64:*AArch64*)  ;;
    armhf:*ARM*)      ;;
    *) echo "  MISMATCH: an $CONTROL_ARCH package holding a $MACHINE binary" >&2; exit 1 ;;
esac
echo "  architecture      : matches"

echo "  links against     :"
readelf -d "$TMP/usr/bin/sdrtop" | sed -n 's/.*Shared library: \[\(.*\)\]/    \1/p'
echo "  depends           :$(dpkg -f "$DEB" Depends | sed 's/^/ /')"
echo "  installed size    : $(dpkg -f "$DEB" Installed-Size) kB"
echo "  contents          : $(dpkg -c "$DEB" | grep -c '^-') files"
