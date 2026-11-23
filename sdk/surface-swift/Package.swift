// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "CogletSurface",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "CogletSurface", targets: ["CogletSurface"]),
        .library(name: "CogletTypes", targets: ["CogletTypes"]),
        .executable(name: "coglet-swift-reference", targets: ["CogletSwiftReference"])
    ],
    targets: [
        .target(name: "CogletTypes"),
        .target(name: "CogletSurface", dependencies: ["CogletTypes"]),
        .executableTarget(name: "CogletSwiftReference", dependencies: ["CogletSurface", "CogletTypes"]),
        .testTarget(name: "CogletTypesTests", dependencies: ["CogletTypes"]),
        .testTarget(name: "CogletSurfaceTests", dependencies: ["CogletSurface", "CogletTypes"])
    ]
)
