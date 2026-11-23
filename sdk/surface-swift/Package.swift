// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "CogletSurface",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "CogletSurface", targets: ["CogletSurface"]),
        .library(name: "CogletTypes", targets: ["CogletTypes"])
    ],
    targets: [
        .target(name: "CogletTypes"),
        .target(name: "CogletSurface", dependencies: ["CogletTypes"]),
        .testTarget(name: "CogletTypesTests", dependencies: ["CogletTypes"])
    ]
)
