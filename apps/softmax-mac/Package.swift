// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SoftmaxCore",
    platforms: [.macOS(.v14)],
    products: [.library(name: "SoftmaxCore", targets: ["SoftmaxCore"])],
    targets: [
        .target(name: "SoftmaxCore"),
        .testTarget(name: "SoftmaxCoreTests", dependencies: ["SoftmaxCore"])
    ]
)
