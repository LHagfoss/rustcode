// swift-tools-version:5.9
// RustCode for macOS: a native SwiftUI client that runs the local RustCode
// engine (`rustcode --acp`) as a child process and talks ACP v1 to it over
// stdio. Build, packaging and install notes are in docs/install.md.
import PackageDescription

let package = Package(
    name: "RustCode",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "RustCode", targets: ["RustCode"]),
    ],
    targets: [
        // UI-free core: JSON-RPC framing, the ACP client, the engine child
        // process and the session model. Everything testable lives here.
        .target(name: "RustCodeKit"),
        .executableTarget(name: "RustCode", dependencies: ["RustCodeKit"]),
        .testTarget(name: "RustCodeKitTests", dependencies: ["RustCodeKit"], path: "Tests/RustCodeKitTests"),
        // App-target tests that need AppKit or WebKit, such as the preview's
        // web view lifecycle (#2140) and the agent bridge against a real page.
        .testTarget(name: "RustCodeTests", dependencies: ["RustCode", "RustCodeKit"], path: "Tests/RustCodeTests"),
    ]
)
