import Foundation
import Security

/// A random identifier for this app install, made once and kept in the keychain apart from
/// the pairing, so it survives pairing again and unpairing. The host uses it to replace the
/// device record this install paired before instead of adding another. It is a fresh UUID,
/// derived from nothing about the user or the phone, readable only on this device and never
/// synced to iCloud Keychain.
public struct KeychainInstallIdentity: Sendable {
    private let service: String
    private let account: String

    public init(service: String = "com.lhagfoss.rustcode.remote", account: String = "install-id") {
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

    /// The stored identifier, made and stored first when there is none. `nil` when the
    /// keychain cannot be read or written: pairing then goes ahead without one.
    public func value() -> String? {
        if let stored = load() { return stored }
        let made = UUID().uuidString
        var item = baseQuery
        item[kSecValueData] = Data(made.utf8)
        item[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        switch SecItemAdd(item as CFDictionary, nil) {
        case errSecSuccess: return made
        // Another caller stored one first; use that.
        case errSecDuplicateItem: return load()
        default: return nil
        }
    }

    /// Forgets the identifier; the next `value()` makes a new one.
    public func reset() {
        SecItemDelete(baseQuery as CFDictionary)
    }

    private func load() -> String? {
        var query = baseQuery
        query[kSecReturnData] = true
        query[kSecMatchLimit] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data,
              let value = String(data: data, encoding: .utf8),
              !value.isEmpty
        else { return nil }
        return value
    }
}
