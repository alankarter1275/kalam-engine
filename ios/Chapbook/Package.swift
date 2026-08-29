// swift-tools-version: 6.0
import PackageDescription

// The binary target is `Chapbook.xcframework`, produced by
// `../build-xcframework.sh` and not checked in — run that first or
// nothing here resolves. It carries `chapbook.h` and a module map in
// every slice, so `import CChapbook` needs no header copy.
let package = Package(
    name: "Chapbook",
    platforms: [.iOS(.v15), .macOS(.v13)],
    products: [
        .library(name: "Chapbook", targets: ["Chapbook"])
    ],
    targets: [
        .binaryTarget(name: "CChapbook", path: "Chapbook.xcframework"),
        .target(name: "Chapbook", dependencies: ["CChapbook"]),
        .testTarget(name: "ChapbookTests", dependencies: ["Chapbook"]),
    ]
)
