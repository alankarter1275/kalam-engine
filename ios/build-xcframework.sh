#!/bin/sh -e
# Build libchapbook_ffi.a for every Apple slice and assemble the
# XCFramework the Swift package binds.
#
# An XCFramework by necessity, not preference: the device and simulator
# libraries are both arm64 and differ only in a Mach-O load command, and
# `lipo` refuses to put them in one fat file. The macOS slice is there so
# `swift test` runs the package's tests natively on the machine that
# builds it, with no simulator in the loop.
#
# The header travels inside each slice with a module map, so consumers —
# the package, the demo's bare swiftc — import CChapbook without a copy
# of chapbook.h anywhere.
cd "$(dirname "$0")"
ROOT=..

# The bundled C (SQLite, ring) compiles at the SDK's own minimum unless
# told otherwise, and every consumer link then prints a wall of
# newer-than-linked warnings. Match the package's platforms.
export IPHONEOS_DEPLOYMENT_TARGET=15.0
export MACOSX_DEPLOYMENT_TARGET=13.0

for target in aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin; do
    cargo build -p chapbook-ffi --release --target $target
done

HEADERS=$(mktemp -d /tmp/chapbook-headers.XXXXXX)
trap 'rm -rf "$HEADERS"' EXIT
cp "$ROOT/crates/chapbook-ffi/include/chapbook.h" "$HEADERS/"
cat > "$HEADERS/module.modulemap" <<'EOF'
module CChapbook {
    header "chapbook.h"
    export *
}
EOF

OUT=Chapbook/Chapbook.xcframework
rm -rf "$OUT"
xcodebuild -create-xcframework \
    -library "$ROOT/target/aarch64-apple-ios/release/libchapbook_ffi.a" \
    -headers "$HEADERS" \
    -library "$ROOT/target/aarch64-apple-ios-sim/release/libchapbook_ffi.a" \
    -headers "$HEADERS" \
    -library "$ROOT/target/aarch64-apple-darwin/release/libchapbook_ffi.a" \
    -headers "$HEADERS" \
    -output "$OUT"
