#!/bin/sh -e
# Typecheck the Swift package against the two iOS slices.
#
# `swift test` runs on the macOS slice, because that is the only one that
# can run at all — and that leaves a hole this closes: PageAccessibility
# puts its UIKit half behind `canImport(UIKit)`, so a macOS build never
# compiles it, and the file is half-gated until something targets iOS.
#
# Typecheck rather than build: no link, no device, no simulator boot. That
# is enough to compile the UIKit branch and to hold both slices to Swift 6
# concurrency, which is the part that would otherwise only be discovered
# in a consumer's project.
#
# The deployment target mirrors `.iOS(.v15)` in Package.swift. Change it
# in both or the package and this check stop agreeing about what compiles.
cd "$(dirname "$0")/Chapbook"

for slice in ios-arm64:iphoneos:arm64-apple-ios15.0 \
             ios-arm64-simulator:iphonesimulator:arm64-apple-ios15.0-simulator; do
    S=${slice%%:*}; rest=${slice#*:}; SDK=${rest%%:*}; TGT=${rest#*:}
    HEADERS=Chapbook.xcframework/$S/Headers
    [ -d "$HEADERS" ] || { echo "run ./build-xcframework.sh first" >&2; exit 1; }
    echo "==> $TGT"
    # The module map travels inside the slice, so this resolves
    # `import CChapbook` without a copy of chapbook.h anywhere.
    xcrun -sdk "$SDK" swiftc -target "$TGT" -typecheck -swift-version 6 \
        -Xcc -fmodule-map-file="$HEADERS/module.modulemap" \
        -I "$HEADERS" \
        Sources/Chapbook/*.swift
done
