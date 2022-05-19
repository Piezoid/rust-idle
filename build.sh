#!/bin/sh

ACTION="${1:-build}"
TARGET=${TARGET:-$(rustc -Vv | grep '^host: ' | cut -c7-)}

if [ $# -gt 0 ]; then
    shift
fi

if rustup toolchain list | grep -q "nightly-$TARGET"; then
    CARGO_FLAGS="+nightly $ACTION -Z build-std=std,panic_abort -Z build-std-features=panic_immediate_abort"
else
    CARGO_FLAGS="$ACTION"
fi

cargo $CARGO_FLAGS --target "$TARGET" --release $@

echo "$TARGET" > target/triple