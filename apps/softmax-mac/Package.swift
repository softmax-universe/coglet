// swift-tools-version: 6.0
import PackageDescription

// Exercise the actual app source against a test-only Objective-C Speech boundary.
// Keep these native framework tests out of portable Linux discovery.
var nativeSpeechTests: [Target] = []
#if os(macOS)
nativeSpeechTests = [
    .target(name: "SpeechCallbackFixture", path: "Tests/SpeechCallbackFixture", publicHeadersPath: "include"),
    .testTarget(name: "LocalSpeechInputTests", dependencies: ["SoftmaxCore", "SpeechCallbackFixture"])
]
#endif

let package = Package(
    name: "SoftmaxCore",
    platforms: [.macOS(.v14)],
    products: [.library(name: "SoftmaxCore", targets: ["SoftmaxCore"])],
    dependencies: [.package(path: "../../surfaces/face-mac")],
    targets: [
        .target(name: "SoftmaxCore", dependencies: [.product(name: "FaceMac", package: "face-mac")]),
        .testTarget(name: "SoftmaxCoreTests", dependencies: ["SoftmaxCore", .product(name: "FaceMac", package: "face-mac")])
    ] + nativeSpeechTests
)
