#!/bin/sh -e
# Build the demo as a hand-rolled .app — no Xcode project — against the
# simulator slice of the XCFramework. The package compiles first as a
# real `Chapbook` module, so `main.swift` imports it exactly the way a
# SwiftPM consumer would; only the build system is hand-rolled.
#
# The custody loop needs a human once:
#   ./build.sh
#   xcrun simctl install booted build/ChapbookDemo.app
#   xcrun simctl launch --console-pty booted com.ophymx.chapbook.demo
#   -> pick a book (bookmark stored, warm open), terminate, launch again
#   -> cold resolve, no picker, same page. Console lines are DEMO-prefixed.
cd "$(dirname "$0")"
SLICE=../Chapbook/Chapbook.xcframework/ios-arm64-simulator
[ -d "$SLICE" ] || { echo "run ../build-xcframework.sh first" >&2; exit 1; }

mkdir -p build/ChapbookDemo.app/fonts
cp ../../fixtures/fonts/CrimsonText-*.ttf build/ChapbookDemo.app/fonts/
cp Info.plist build/ChapbookDemo.app/

xcrun -sdk iphonesimulator swiftc \
    -target arm64-apple-ios15.0-simulator -swift-version 6 -parse-as-library \
    -I "$SLICE/Headers" \
    -module-name Chapbook \
    -emit-module -emit-module-path build/Chapbook.swiftmodule \
    -emit-library -static -o build/libChapbook.a \
    ../Chapbook/Sources/Chapbook/*.swift

xcrun -sdk iphonesimulator swiftc \
    -target arm64-apple-ios15.0-simulator -swift-version 6 -parse-as-library \
    -I build -I "$SLICE/Headers" \
    main.swift \
    -L build -lChapbook -L "$SLICE" -lchapbook_ffi \
    -o build/ChapbookDemo.app/ChapbookDemo

echo "built; install and launch it in a booted simulator"
