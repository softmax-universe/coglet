// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SoftmaxAppSurface",
    platforms: [.macOS(.v14)],
    products: [.library(name: "SoftmaxAppSurface", targets: ["SoftmaxAppSurface"])],
    dependencies: [.package(path: "../../../sdk/surface-swift")],
    targets: [
        .target(name: "SoftmaxAppSurface", dependencies: [
            .product(name: "CogletSurface", package: "surface-swift"),
            .product(name: "CogletTypes", package: "surface-swift")
        ]),
        .testTarget(name: "SoftmaxAppSurfaceTests", dependencies: [
            "SoftmaxAppSurface", .product(name: "CogletSurface", package: "surface-swift"),
            .product(name: "CogletTypes", package: "surface-swift")
        ])
    ]
)
