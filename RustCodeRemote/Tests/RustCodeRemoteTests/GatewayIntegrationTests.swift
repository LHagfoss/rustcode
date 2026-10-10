import Foundation
import Testing
@testable import RustCodeRemote

/// Drives a real RustCode gateway over loopback. Runs only when `RUSTCODE_GATEWAY_BIN` names
/// a `rustcode` binary that has `remote serve`:
///
///     RUSTCODE_GATEWAY_BIN=/path/to/rustcode swift test --filter GatewayIntegrationTests
@Suite(.enabled(if: ProcessInfo.processInfo.environment["RUSTCODE_GATEWAY_BIN"] != nil), .serialized)
struct GatewayIntegrationTests {
    /// One gateway process in its own configuration directory.
    final class Gateway: Sendable {
        let binary: String
        let configuration: URL
        let port: Int
        private let process: Process

        /// - Parameter configFile: contents of `config.toml` for this configuration directory,
        ///   which a terminal session started with it will also use.
        init(configFile: String? = nil, bind: String? = "127.0.0.1", binary: String? = nil) async throws {
            self.binary = try #require(binary ?? ProcessInfo.processInfo.environment["RUSTCODE_GATEWAY_BIN"])
            // The gateway's control socket lives under the configuration directory, and a
            // Unix socket path has to stay short.
            configuration = URL(fileURLWithPath: "/tmp/rcgw-\(UUID().uuidString.prefix(8))")
            try FileManager.default.createDirectory(at: configuration, withIntermediateDirectories: true)
            if let configFile {
                try configFile.write(to: configuration.appendingPathComponent("config.toml"), atomically: true, encoding: .utf8)
            }
            port = Int.random(in: 20000...40000)

            process = Process()
            process.executableURL = URL(fileURLWithPath: self.binary)
            process.arguments = ["remote", "serve", "--port", "\(port)"]
            if let bind { process.arguments! += ["--bind", bind] }
            process.environment = ["RUSTCODE_CONFIG_DIR": configuration.path, "PATH": "/usr/bin:/bin", "HOME": FileManager.default.homeDirectoryForCurrentUser.path]
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            try process.run()

            for _ in 0..<100 {
                if (try? run("remote", "status"))?.contains("running") == true { return }
                try await Task.sleep(for: .milliseconds(50))
            }
            throw RemoteError.transport("the gateway did not start")
        }

        @discardableResult
        func run(_ arguments: String...) throws -> String {
            try runUsing(binary, arguments: arguments)
        }

        func runUsing(_ executable: String, arguments: [String]) throws -> String {
            let command = Process()
            command.executableURL = URL(fileURLWithPath: executable)
            command.arguments = arguments
            command.environment = ["RUSTCODE_CONFIG_DIR": configuration.path, "PATH": "/usr/bin:/bin", "HOME": FileManager.default.homeDirectoryForCurrentUser.path]
            let output = Pipe()
            command.standardOutput = output
            command.standardError = output
            try command.run()
            let data = output.fileHandleForReading.readDataToEndOfFile()
            command.waitUntilExit()
            return String(decoding: data, as: UTF8.self)
        }

        /// A fresh pairing challenge: the manual code and the QR payload.
        func pairingChallenge() throws -> (code: String, payload: String) {
            let output = try run("remote", "pair")
            func value(after label: String) throws -> String {
                let line = try #require(output.split(separator: "\n").first { $0.contains(label) })
                return String(line[line.range(of: label)!.upperBound...]).trimmingCharacters(in: .whitespaces)
            }
            return (try value(after: "code:"), try value(after: "QR payload:"))
        }

        func stop() {
            _ = try? run("remote", "stop")
            process.terminate()
            try? FileManager.default.removeItem(at: configuration)
        }
    }

    private func makeClient() -> LiveRemoteClient {
        LiveRemoteClient(codec: WireCodec(), requestTimeout: .seconds(5))
    }

    @Test(.enabled(if: ProcessInfo.processInfo.environment["RUSTCODE_OLD_GATEWAY_BIN"] != nil), .timeLimit(.minutes(2)))
    func upgradingBackgroundGatewayKeepsPairingAndAddress() async throws {
        let old = try #require(ProcessInfo.processInfo.environment["RUSTCODE_OLD_GATEWAY_BIN"])
        let current = try #require(ProcessInfo.processInfo.environment["RUSTCODE_GATEWAY_BIN"])
        let gateway = try await Gateway(configFile: "version = 1\n[remote]\nbind = \"127.0.0.1\"\n", binary: old)
        defer { gateway.stop() }
        let client = makeClient()
        let credential = try await client.pair(.scanned(try gateway.pairingChallenge().payload), deviceName: "Upgrade test")
        let before = try await client.connect(using: credential)
        await client.disconnect()
        _ = try gateway.runUsing(current, arguments: ["remote", "start"])
        let after = try await client.connect(using: credential)
        #expect(after.gatewayID == before.gatewayID)
        #expect(after.instanceID != before.instanceID)
        #expect(credential.host.address.port == gateway.port)
        var sessions = try await client.subscribeSessions().makeAsyncIterator()
        #expect(try await sessions.next() == [])
        await client.disconnect()
    }

    /// Opt in on a Mac with a reachable LAN; do not make isolated CI require Wi-Fi.
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RUSTCODE_LAN_PAIRING"] == "1"))
    func automaticallyAdvertisedLANQRCodePairsWithTheLiveClient() async throws {
        let gateway = try await Gateway(bind: nil)
        defer { gateway.stop() }
        let challenge = try gateway.pairingChallenge()
        let target = try WireCodec().pairingTarget(for: .scanned(challenge.payload))
        #expect(target.address.host != "127.0.0.1")
        #expect(target.address.host != "localhost")
        #expect(target.address.host != "0.0.0.0")
        #expect(target.address.host != "::")
        #expect(target.address.host != "::1")
        let client = makeClient()
        let credential = try await client.pair(.scanned(challenge.payload), deviceName: "LAN Integration Client")
        #expect(credential.host.address == target.address)
        let identity = try await client.connect(using: credential)
        #expect(identity.gatewayID == target.expectedGatewayID)
        var lists = try await client.subscribeSessions().makeAsyncIterator()
        #expect(try await lists.next() == [])
        await client.disconnect()
    }

    @Test func pairsWithTheQRPayloadAuthenticatesAndIsClosedWhenRevoked() async throws {
        let gateway = try await Gateway()
        defer { gateway.stop() }
        let challenge = try gateway.pairingChallenge()

        // Pair with the credential from the QR payload.
        let client = makeClient()
        let credential = try await client.pair(.scanned(challenge.payload), deviceName: "Integration Phone")
        #expect(credential.host.address == HostAddress(host: "127.0.0.1", port: gateway.port))
        #expect(credential.deviceToken.count == 43)
        #expect(!credential.host.device.id.isEmpty)
        #expect(credential.host.device.name == "Integration Phone")
        let devices = try gateway.run("remote", "devices", "--json")
        #expect(devices.contains(credential.host.device.id))
        #expect(!devices.contains(credential.deviceToken))

        // The challenge is single use, in both representations.
        do {
            _ = try await makeClient().pair(
                .manual(address: credential.host.address, code: challenge.code),
                deviceName: "Second"
            )
            Issue.record("a used challenge paired again")
        } catch {
            guard case RemoteError.pairingFailed = error else {
                Issue.record("unexpected error \(error)")
                return
            }
        }

        // Reconnect with the stored token.
        let identity = try await client.connect(using: credential)
        #expect(identity.gatewayID == credential.host.gatewayID)
        #expect(!identity.instanceID.isEmpty)

        // Nothing is shared yet: connected, with an empty session list.
        let sessions = try await client.subscribeSessions()
        var iterator = sessions.makeAsyncIterator()
        #expect(try await iterator.next() == [])
        #expect(try gateway.run("remote", "status").contains("running"))

        // Revoking the device closes its connection with `revoked`.
        try gateway.run("remote", "revoke", credential.host.device.id)
        await #expect(throws: RemoteError.unauthorized) {
            while try await iterator.next() != nil {}
        }
        // And the token is dead from then on.
        await #expect(throws: RemoteError.unauthorized) {
            try await makeClient().connect(using: credential)
        }
    }

    @Test func pairsWithTheManualCodeAndRejectsAWrongOne() async throws {
        let gateway = try await Gateway()
        defer { gateway.stop() }
        let challenge = try gateway.pairingChallenge()
        let address = HostAddress(host: "127.0.0.1", port: gateway.port)

        do {
            _ = try await makeClient().pair(.manual(address: address, code: "0000-0000"), deviceName: "Phone")
            Issue.record("a wrong code paired")
        } catch {
            guard case RemoteError.pairingFailed = error else {
                Issue.record("unexpected error \(error)")
                return
            }
        }

        // Typed without the separator, as a person might.
        let bare = challenge.code.replacingOccurrences(of: "-", with: "")
        let credential = try await makeClient().pair(.manual(address: address, code: bare), deviceName: "Phone")
        let identity = try await makeClient().connect(using: credential)
        #expect(identity.gatewayID == credential.host.gatewayID)
    }

    @Test func aRestartedGatewayKeepsItsIdentityButChangesItsInstance() async throws {
        let gateway = try await Gateway()
        let credential = try await makeClient().pair(.scanned(try gateway.pairingChallenge().payload), deviceName: "Phone")
        let first = try await makeClient().connect(using: credential)
        _ = try? gateway.run("remote", "stop")
        try await Task.sleep(for: .milliseconds(500))

        // Same configuration directory and port: the same host, started again.
        let restarted = Process()
        restarted.executableURL = URL(fileURLWithPath: gateway.binary)
        restarted.arguments = ["remote", "serve", "--bind", "127.0.0.1", "--port", "\(gateway.port)"]
        restarted.environment = ["RUSTCODE_CONFIG_DIR": gateway.configuration.path, "PATH": "/usr/bin:/bin"]
        restarted.standardOutput = FileHandle.nullDevice
        restarted.standardError = FileHandle.nullDevice
        try restarted.run()
        defer {
            gateway.stop()
            restarted.terminate()
        }
        var second: ConnectionIdentity?
        for _ in 0..<50 where second == nil {
            second = try? await makeClient().connect(using: credential)
            if second == nil { try await Task.sleep(for: .milliseconds(100)) }
        }
        #expect(second?.gatewayID == first.gatewayID)
        #expect(second?.instanceID != first.instanceID)
    }

    @Test func tooManyWrongCodesLockPairingAndSayForHowLong() async throws {
        let gateway = try await Gateway()
        defer { gateway.stop() }
        let address = HostAddress(host: "127.0.0.1", port: gateway.port)

        var lockedFor: Duration?
        for _ in 0..<14 where lockedFor == nil {
            // Five wrong attempts exhaust a challenge, so keep one open.
            _ = try gateway.pairingChallenge()
            do {
                _ = try await makeClient().pair(.manual(address: address, code: "0000-0000"), deviceName: "Phone")
            } catch RemoteError.rateLimited(let retryAfter) {
                lockedFor = retryAfter ?? .zero
            } catch {}
        }
        let retryAfter = try #require(lockedFor, "pairing was never rate limited")
        #expect(retryAfter > .zero)
        #expect(retryAfter <= .seconds(300))
    }
}
