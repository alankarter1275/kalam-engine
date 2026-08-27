#!/bin/sh -e
# Rung 2 of the iOS ladder: it binds. Build the C ABI for both iOS
# targets, compile the Swift spike against the checked-in header, run the
# simulator binary inside a booted simulator, and link-check the device
# slice this machine cannot run. Findings go to docs/FFI.md.
cd "$(dirname "$0")"
ROOT=../..

cargo build -p chapbook-ffi --release --target aarch64-apple-ios-sim
cargo build -p chapbook-ffi --release --target aarch64-apple-ios

mkdir -p build

# `xcrun -sdk` rather than `swiftc -sdk`: the latter leaves clang's linker
# on the macOS sysroot and warns, even though the -sdk flag wins the link.
# -swift-version 6 is load-bearing — it is what makes the Session
# wrapper's Send-not-Sync claim compiler-checked rather than prose.
xcrun -sdk iphonesimulator swiftc \
    -target arm64-apple-ios15.0-simulator -swift-version 6 \
    -I CChapbook main.swift \
    -L "$ROOT/target/aarch64-apple-ios-sim/release" -lchapbook_ffi \
    -o build/spike-sim

# The device slice links or it doesn't; there is nothing here to run it.
xcrun -sdk iphoneos swiftc \
    -target arm64-apple-ios15.0 -swift-version 6 \
    -I CChapbook main.swift \
    -L "$ROOT/target/aarch64-apple-ios/release" -lchapbook_ffi \
    -o build/spike-dev
echo "device slice links"

# A booted simulator, or the first available iPhone booted headless —
# no Simulator.app required.
UDID=$(xcrun simctl list devices booted | grep -oE '[0-9A-F-]{36}' | head -1)
if [ -z "$UDID" ]; then
    UDID=$(xcrun simctl list devices available | grep "iPhone" | grep -oE '[0-9A-F-]{36}' | head -1)
    [ -n "$UDID" ] || { echo "no iPhone simulator available"; exit 1; }
    xcrun simctl boot "$UDID"
    trap 'xcrun simctl shutdown "$UDID"' EXIT
fi

LIB=$(mktemp -d /tmp/chapbook-spike.XXXXXX)
xcrun simctl spawn "$UDID" "$PWD/build/spike-sim" \
    "$PWD/$ROOT/fixtures/epub/minimal.epub" \
    "$PWD/$ROOT/fixtures/fonts" \
    "$LIB"
