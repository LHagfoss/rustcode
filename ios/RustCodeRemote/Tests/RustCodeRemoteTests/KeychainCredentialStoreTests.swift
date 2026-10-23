import Foundation
import Testing
@testable import RustCodeRemote

/// Runs against the real keychain of the machine running the tests. On macOS that is the
/// user's login keychain, which a command-line test process can use without entitlements.
/// Each run uses its own service name and removes what it stored.
@Suite struct KeychainCredentialStoreTests {
    @Test func savesUpdatesLoadsAndDeletes() throws {
        let store = KeychainCredentialStore(service: "com.lhagfoss.rustcode.tests.\(UUID().uuidString)")
        defer { try? store.delete() }

        #expect(try store.load() == nil)

        let first = makeCredential(token: "first-token")
        try store.save(first)
        #expect(try store.load() == first)

        // Saving again replaces the item instead of failing as a duplicate.
        var second = makeCredential(token: "second-token")
        second.host.name = "Renamed"
        try store.save(second)
        #expect(try store.load() == second)

        try store.delete()
        #expect(try store.load() == nil)
        // Deleting what is not there is not an error.
        try store.delete()
    }

    @Test func theInstallIdentityIsMadeOnceAndOutlivesThePairing() throws {
        let service = "com.lhagfoss.rustcode.tests.\(UUID().uuidString)"
        let identity = KeychainInstallIdentity(service: service)
        let credentials = KeychainCredentialStore(service: service)
        defer {
            identity.reset()
            try? credentials.delete()
        }

        let first = try #require(identity.value())
        #expect(UUID(uuidString: first) != nil)
        // A fresh instance reads the same item: what a relaunch of the app sees.
        #expect(KeychainInstallIdentity(service: service).value() == first)

        // Pairing and unpairing replace and delete the credential, never the identity.
        try credentials.save(makeCredential(token: "token"))
        try credentials.delete()
        #expect(identity.value() == first)

        identity.reset()
        let second = try #require(identity.value())
        #expect(second != first)
    }

    @Test func storesAreSeparatedByService() throws {
        let a = KeychainCredentialStore(service: "com.lhagfoss.rustcode.tests.\(UUID().uuidString)")
        let b = KeychainCredentialStore(service: "com.lhagfoss.rustcode.tests.\(UUID().uuidString)")
        defer {
            try? a.delete()
            try? b.delete()
        }
        try a.save(makeCredential(token: "a"))
        #expect(try b.load() == nil)
    }
}
