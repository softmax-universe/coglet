// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "FaceMac",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "FaceMac", targets: ["FaceMac"]),
        .library(name: "FaceAudioKit", targets: ["FaceAudioKit"])
    ],
    targets: [
        .target(name: "FaceMac"),
        .target(name: "CAudioRing", publicHeadersPath: "include"),
        .target(name: "FaceAudioKit", dependencies: ["CAudioRing"]),
        .testTarget(name: "FaceAudioKitTests", dependencies: ["FaceAudioKit", "CAudioRing"])
    ]
)
