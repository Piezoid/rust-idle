#!/bin/sh
set -e

# Destination directory (used by package builder to set their output directory)
DESTDIR=${DESTDIR:-/}
# Prefix path of installation
export PREFIX=${PREFIX:-/usr/local}
# Path for the config file read by the systemd unit
export CONFD=${CONFD:-/etc/conf.d}

ARTIFACT_DIR=${ARTIFACT_DIR:-.}

_pkgname="rust-idle"
PKGNAME=${PKGNAME:-"$_pkgname"}

install -Dm755 "$ARTIFACT_DIR/rust-idle" "$DESTDIR/$PREFIX/bin/rust-idle"
strip -s "$DESTDIR/$PREFIX/bin/rust-idle"

install -Dm644 README.md "$DESTDIR/$PREFIX/share/doc/$PKGNAME/README.md"
install -Dm644 LICENSE "$DESTDIR/$PREFIX/share/licenses/$PKGNAME/LICENSE"
install -Dm644 rust-idle.conf "$DESTDIR/$CONFD/rust-idle"

envsubst '$PREFIX $CONFD' < rust-idle.service.in | \
   install -Dm644 /dev/stdin "$DESTDIR/$PREFIX/lib/systemd/system/rust-idle.service"
