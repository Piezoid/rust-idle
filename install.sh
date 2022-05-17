#!/bin/sh
set -e

# Destination directory (used by package builder to set their output directory)
DESTDIR=${DESTDIR:-/}
# Prefix path of installation
export PREFIX=${PREFIX:-/usr/local}
# Path for the config file read by the systemd unit
export CONFD=${CONFD:-/etc/conf.d}

# build.sh puts the target triple in "target/triple". It is read for finding the
# generated binary. When cargo build is used directly, this is empty and will
# pull the binary from target/release/. However if you give an explicit --target
# to cargo, you'll have to manually set this in the environment.
TARGET=${TARGET:-"$(cat target/triple 2>/dev/null || true)"}

_pkgname="rust-idle"
PKGNAME=${PKGNAME:-"$_pkgname"}

install -Dm755 "target/$TARGET/release/rust-idle" "$DESTDIR/$PREFIX/bin/rust-idle"

install -Dm644 README.md "$DESTDIR/$PREFIX/share/doc/$PKGNAME/README.md"
install -Dm644 LICENSE "$DESTDIR/$PREFIX/share/licenses/$PKGNAME/LICENSE"
install -Dm644 rust-idle.conf "$DESTDIR/$CONFD/rust-idle"

envsubst '$PREFIX $CONFD' < rust-idle.service.in | \
   install -Dm644 /dev/stdin "$DESTDIR/$PREFIX/lib/systemd/system/rust-idle.service"
