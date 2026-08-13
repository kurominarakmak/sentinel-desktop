// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "SentinelMac",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "SentinelMac", targets: ["SentinelMac"])],
    targets: [
        .executableTarget(name: "SentinelMac"),
        .testTarget(name: "SentinelMacTests", dependencies: ["SentinelMac"]),
    ]
)
