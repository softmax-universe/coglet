// swift-tools-version: 6.0
// Release-tag entry point; implementation remains in sdk/surface-swift.
import PackageDescription

let package = Package(
    name: "CogletSurface",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "CogletSurface", targets: ["CogletSurface"]),
        .library(name: "CogletTypes", targets: ["CogletTypes"])
    ],
    targets: [
        .target(name: "CogletTypes", path: "sdk/surface-swift/Sources/CogletTypes"),
        .target(name: "CogletSurface", dependencies: ["CogletTypes"], path: "sdk/surface-swift/Sources/CogletSurface")
    ]
)
