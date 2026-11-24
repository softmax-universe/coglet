// swift-tools-version: 6.0
import Foundation
import PackageDescription

let environment = ProcessInfo.processInfo.environment
let source = environment["COGLET_RELEASE_SOURCE"]!
let tag = environment["RELEASE_TAG"]!
let releaseVersion = Version(String(tag.dropFirst()))!
let package = Package(
    name: "PinProbe",
    platforms: [.macOS(.v14)],
    dependencies: [.package(url: source, exact: releaseVersion)],
    targets: [
        .executableTarget(name: "PinProbe", dependencies: [.product(name: "CogletSurface", package: "coglet")])
    ]
)
