// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "CogletSurface",
    platforms: [.macOS(.v14)],
    products: [.library(name: "CogletSurface", targets: ["CogletSurface"])],
    targets: [.target(name: "CogletSurface")]
)
