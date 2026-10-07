import CryptoKit
import Foundation
import Security

/// The keys of the Push Subscription of the app (RFC 8291): a P-256 key
/// agreement key and a 16-byte auth secret. The private key never leaves
/// the app. The Notification Service Extension reads it to decrypt a push.
struct PushKeys {
    let privateKey: P256.KeyAgreement.PrivateKey
    let auth: Data

    /// The public key as the 65-byte uncompressed point, `0x04 || X || Y`.
    var p256dh: Data { privateKey.publicKey.x963Representation }

    /// New keys: a CryptoKit key and 16 bytes from `SecRandomCopyBytes`.
    static func make() throws -> PushKeys {
        var auth = Data(count: 16)
        let status = auth.withUnsafeMutableBytes { bytes in
            SecRandomCopyBytes(kSecRandomDefault, 16, bytes.baseAddress!)
        }
        guard status == errSecSuccess else { throw SecretItemsError(operation: "make the auth secret", status: status) }
        return PushKeys(privateKey: P256.KeyAgreement.PrivateKey(), auth: auth)
    }
}

/// Keeps the keys of the Push Subscription in the secret items of the app.
final class PushKeyStore {
    /// The name of the item that holds the keys.
    static let item = "push.keys"

    private struct Stored: Codable {
        /// The 32-byte private scalar.
        let privateKey: Data
        let auth: Data
    }

    private let items: SecretItems

    init(items: SecretItems) {
        self.items = items
    }

    /// The stored keys, or new keys that the store keeps from now on.
    func keys() throws -> PushKeys {
        if let keys = try stored() { return keys }
        let keys = try PushKeys.make()
        let stored = Stored(privateKey: keys.privateKey.rawRepresentation, auth: keys.auth)
        try items.write(try JSONEncoder().encode(stored), as: PushKeyStore.item)
        return keys
    }

    /// The stored keys, or nil when the app holds none. The Notification
    /// Service Extension reads the keys with this, and never makes them.
    func stored() throws -> PushKeys? {
        guard let data = try items.read(PushKeyStore.item) else { return nil }
        let stored = try JSONDecoder().decode(Stored.self, from: data)
        return PushKeys(
            privateKey: try P256.KeyAgreement.PrivateKey(rawRepresentation: stored.privateKey),
            auth: stored.auth
        )
    }

    func delete() throws {
        try items.delete(PushKeyStore.item)
    }
}

/// Named secret items of the app. The Keychain holds them in the app, and
/// a test gives items in memory.
protocol SecretItems: AnyObject {
    /// The item `name`, or nil when there is none.
    func read(_ name: String) throws -> Data?
    /// Keep `data` as the item `name`, in place of an earlier item.
    func write(_ data: Data, as name: String) throws
    /// Remove the item `name`. No item is no error.
    func delete(_ name: String) throws
}

/// A Keychain operation that failed.
struct SecretItemsError: LocalizedError {
    let operation: String
    let status: OSStatus

    var errorDescription: String? {
        "Pagis cannot \(operation) in the Keychain (status \(status))."
    }
}

/// Items in the Keychain, which the device can read after the first
/// unlock, so the Notification Service Extension reads them while the
/// phone is locked. The items are in the Keychain access group that the
/// `PagisKeychainAccessGroup` key of `Info.plist` names, which the
/// extension shares.
final class KeychainItems: SecretItems {
    private let service: String
    private let accessGroup: String?

    init(
        service: String = "co.pagis.mobile.push",
        accessGroup: String? = Bundle.main.object(forInfoDictionaryKey: "PagisKeychainAccessGroup") as? String
    ) {
        self.service = service
        self.accessGroup = accessGroup
    }

    func read(_ name: String) throws -> Data? {
        var query = baseQuery(name)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else {
            throw SecretItemsError(operation: "read \(name)", status: status)
        }
        return data
    }

    func write(_ data: Data, as name: String) throws {
        try delete(name)
        var item = baseQuery(name)
        item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else { throw SecretItemsError(operation: "keep \(name)", status: status) }
    }

    func delete(_ name: String) throws {
        let status = SecItemDelete(baseQuery(name) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw SecretItemsError(operation: "delete \(name)", status: status)
        }
    }

    private func baseQuery(_ name: String) -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: name,
        ]
        if let accessGroup, !accessGroup.isEmpty {
            query[kSecAttrAccessGroup as String] = accessGroup
        }
        return query
    }
}

extension Data {
    /// Base64url with no padding (RFC 4648 section 5), as Web Push gives
    /// the keys of a subscription.
    func base64URLEncodedString() -> String {
        base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    /// The bytes of base64url text, with or without padding, as Web Push
    /// gives a body and the keys of a subscription. Nil for text that is
    /// not base64url.
    init?(base64URLEncoded text: String) {
        guard !text.contains("+"), !text.contains("/") else { return nil }
        var base64 = text
            .replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        base64 += String(repeating: "=", count: (4 - base64.count % 4) % 4)
        self.init(base64Encoded: base64)
    }
}
