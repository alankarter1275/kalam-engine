#!/bin/sh -e
# Rung 5 is the one rung a script cannot climb alone: it needs a human
# to pick a book in the document picker, once. This builds and installs
# the app; the ladder is then
#   1. launch it, pick a book         (bookmark stored, warm open)
#   2. terminate it, launch it again  (cold resolve — the actual test)
# Console lines are prefixed RUNG5:
#   xcrun simctl launch --console-pty booted com.ophymx.chapbook.rung5
cd "$(dirname "$0")"
ROOT=../../..

cargo build -p chapbook-ffi --release --target aarch64-apple-ios-sim

mkdir -p build/rung5.app/fonts
cp "$ROOT"/fixtures/fonts/CrimsonText-*.ttf build/rung5.app/fonts/
cp Info.plist build/rung5.app/

xcrun -sdk iphonesimulator swiftc \
    -target arm64-apple-ios15.0-simulator -swift-version 6 -parse-as-library \
    -I ../CChapbook main.swift \
    -L "$ROOT/target/aarch64-apple-ios-sim/release" -lchapbook_ffi \
    -o build/rung5.app/rung5

xcrun simctl install booted build/rung5.app
echo "installed; launch it, pick a book, relaunch cold"
