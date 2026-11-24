// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "FaceRuntimeProof", platforms: [.macOS(.v14)],
    dependencies: [.package(path: ".."), .package(path: "../../../sdk/surface-swift")],
    targets: [
        .systemLibrary(name: "CRuntime"),
        .executableTarget(name: "FaceRuntimeProof", dependencies: ["CRuntime",
            .product(name: "FaceMac", package: "face-mac"),
            .product(name: "CogletSurface", package: "surface-swift")])
    ])
