// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "SoftmaxRuntime", platforms: [.macOS(.v14)],
    products: [.library(name: "SoftmaxRuntime", targets: ["SoftmaxRuntime"])],
    dependencies: [.package(path: "../../../sdk/surface-swift"), .package(path: ".."), .package(path: "../Surface")],
    targets: [
        .binaryTarget(name: "CogletRuntime", path: "Artifacts/CogletRuntime.xcframework"),
        .target(name: "SoftmaxRuntime", dependencies: ["CogletRuntime", .product(name: "CogletSurface", package: "surface-swift"), .product(name: "SoftmaxAppSurface", package: "Surface")],
            linkerSettings: [.linkedFramework("Security"), .linkedFramework("CoreFoundation"), .linkedLibrary("iconv")]),
        .testTarget(name: "RuntimeHostTests", dependencies: ["SoftmaxRuntime", .product(name: "SoftmaxCore", package: "softmax-mac")])
    ])
