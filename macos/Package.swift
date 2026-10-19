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
    dependencies: [
        // Terminal emulation and PTY for the workspace terminal panel (MIT).
        // Pinned exactly; bump deliberately and re-check packaging.
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", exact: "1.20.0"),
    ],
    targets: [
        // UI-free core: JSON-RPC framing, the ACP client, the engine child
        // process and the session model. Everything testable lives here.
        .target(name: "RustCodeKit"),
        // The workspace terminal's sessions: PTY shells per project, their
        // lifecycle and limits. UI-free like RustCodeKit, and kept apart from
        // it so the engine client does not depend on SwiftTerm. It never
        // talks to the engine.
        .target(name: "RustCodeTerminal", dependencies: [
            "RustCodeKit",
            .product(name: "SwiftTerm", package: "SwiftTerm"),
        ]),
        .executableTarget(name: "RustCode", dependencies: [
            "RustCodeKit",
            "RustCodeTerminal",
            .product(name: "SwiftTerm", package: "SwiftTerm"),
        ]),
        .testTarget(name: "RustCodeKitTests", dependencies: ["RustCodeKit"], path: "Tests/RustCodeKitTests"),
        .testTarget(name: "RustCodeTerminalTests", dependencies: [
            "RustCodeTerminal",
            .product(name: "SwiftTerm", package: "SwiftTerm"),
        ], path: "Tests/RustCodeTerminalTests"),
    ]
)
