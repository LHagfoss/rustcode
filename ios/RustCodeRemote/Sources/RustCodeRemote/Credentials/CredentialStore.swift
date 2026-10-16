import Foundation
import Security
import Synchronization

/// Secure storage for the one paired host and its device token.
public protocol CredentialStore: Sendable {
    func load() throws -> StoredCredential?
    func save(_ credential: StoredCredential) throws
    func delete() throws
}

public final class InMemoryCredentialStore: CredentialStore {
    private let storage: Mutex<StoredCredential?>

    public init(_ credential: StoredCredential? = nil) {
        storage = Mutex(credential)
    }

    public func load() -> StoredCredential? {
        storage.withLock { $0 }
    }

    public func save(_ credential: StoredCredential) {
        storage.withLock { $0 = credential }
    }

    public func delete() {
        storage.withLock { $0 = nil }
    }
}

public struct KeychainError: Error, Sendable, Equatable, LocalizedError {
    public var status: OSStatus

    public var errorDescription: String? {
        let message = SecCopyErrorMessageString(status, nil) as String? ?? "unknown error"
        return "Keychain error \(status): \(message)"
    }
}

/// Stores the credential as one generic-password item, readable only on this device and
/// only after first unlock. It is never synced to iCloud Keychain.
public struct KeychainCredentialStore: CredentialStore {
    private let service: String
    private let account: String

    public init(service: String = "com.lhagfoss.rustcode.remote", account: String = "paired-host") {
        self.service = service
        self.account = account
    }

    private var baseQuery: [CFString: Any] {
        [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecAttrSynchronizable: false,
        ]
    }

    public func load() throws -> StoredCredential? {
        var query = baseQuery
        query[kSecReturnData] = true
        query[kSecMatchLimit] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        switch status {
        case errSecSuccess:
            guard let data = result as? Data else { return nil }
            // An item that no longer decodes is from an older build; treat it as unpaired.
            return try? JSONDecoder().decode(StoredCredential.self, from: data)
        case errSecItemNotFound:
            return nil
        default:
            throw KeychainError(status: status)
        }
    }

    public func save(_ credential: StoredCredential) throws {
        let data = try JSONEncoder().encode(credential)
        let update: [CFString: Any] = [
            kSecValueData: data,
            kSecAttrAccessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        var status = SecItemUpdate(baseQuery as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            status = SecItemAdd(baseQuery.merging(update) { $1 } as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw KeychainError(status: status) }
    }

    public func delete() throws {
        let status = SecItemDelete(baseQuery as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainError(status: status)
        }
    }
}
