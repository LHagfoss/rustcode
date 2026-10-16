// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "RustCodeRemote",
    platforms: [.iOS(.v18), .macOS(.v15)],
    products: [
        .library(name: "RustCodeRemote", targets: ["RustCodeRemote"])
    ],
    targets: [
        .target(name: "RustCodeRemote"),
        .testTarget(
            name: "RustCodeRemoteTests",
            dependencies: ["RustCodeRemote"],
            // The protocol contract, copied from the RustCode repository by Scripts/sync-protocol.sh.
            resources: [.copy("Resources/remote-protocol")]
        ),
    ],
    swiftLanguageModes: [.v6]
)
