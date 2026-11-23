// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "FaceMac",
    platforms: [.macOS(.v14)],
    products: [.library(name: "FaceMac", targets: ["FaceMac"])],
    targets: [.target(name: "FaceMac")]
)
