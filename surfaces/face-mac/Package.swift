// swift-tools-version: 6.0
import PackageDescription

var faceDependencies: [Target.Dependency] = [
    .product(name: "CogletSurface", package: "surface-swift"),
    .product(name: "CogletTypes", package: "surface-swift")
]
#if os(macOS)
faceDependencies.append("FaceAudioKit")
#endif

let package = Package(
    name: "FaceMac",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "FaceMac", targets: ["FaceMac"]),
        .executable(name: "face-mac", targets: ["FaceMacHost"]),
        .library(name: "FaceAudioKit", targets: ["FaceAudioKit"])
    ],
    dependencies: [.package(path: "../../sdk/surface-swift")],
    targets: [
        .target(name: "FaceMac", dependencies: faceDependencies),
        .target(name: "FaceMacEmbeddingExample", dependencies: ["FaceMac", "FaceAudioKit", .product(name: "CogletSurface", package: "surface-swift")], path: "Examples"),
        .executableTarget(name: "FaceMacHost", dependencies: ["FaceMac", .product(name: "CogletSurface", package: "surface-swift"), .product(name: "CogletTypes", package: "surface-swift")]),
        .testTarget(name: "FaceMacTests", dependencies: ["FaceMac"]),
        .target(name: "CAudioRing", publicHeadersPath: "include"),
        .target(name: "FaceAudioKit", dependencies: ["CAudioRing"]),
        .testTarget(name: "FaceAudioKitTests", dependencies: ["FaceAudioKit", "CAudioRing"])
    ]
)
#if !os(macOS)
// Portable semantic tests do not compile AVFoundation. Native CI still builds
// both audio targets and exercises the real production callbacks.
package.targets.removeAll { ["FaceAudioKit", "FaceAudioKitTests", "CAudioRing", "FaceMacEmbeddingExample"].contains($0.name) }
package.products.removeAll { $0.name == "FaceAudioKit" }
#endif
