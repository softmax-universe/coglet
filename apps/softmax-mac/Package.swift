// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SoftmaxCore",
    platforms: [.macOS(.v14)],
    products: [.library(name: "SoftmaxCore", targets: ["SoftmaxCore"])],
    dependencies: [.package(path: "../../surfaces/face-mac")],
    targets: [
        .target(name: "SoftmaxCore", dependencies: [.product(name: "FaceMac", package: "face-mac")]),
        .testTarget(name: "SoftmaxCoreTests", dependencies: ["SoftmaxCore", .product(name: "FaceMac", package: "face-mac")])
    ]
)
