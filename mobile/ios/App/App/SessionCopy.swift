import Foundation
import Security

/// The Session cookie of the server, `pagis_session`: its value and its
/// expiry. The value does not change during the life of the Session.
struct Session: Equatable, Codable {
    /// The name of the Session cookie of the daemon.
    static let cookieName = "pagis_session"

    let value: String
    let expires: Date

    /// Whether the Session is still live at `now`.
    func isLive(at now: Date) -> Bool {
        expires > now
    }
}

/// The native copy of the Session for native requests (ADR-0032). The
/// Notification Service Extension and a notification action run outside
/// the web view, so they read the copy and not the cookie store.
protocol SessionCopy: AnyObject {
    /// The copy for this origin, or nil when the copy is of no Session or
    /// of another origin.
    func read(for origin: WebOrigin) -> Session?
    /// Keep `session` as the copy of `origin`, in place of each earlier
    /// copy.
    func write(_ session: Session, for origin: WebOrigin)
    /// Remove the copy.
    func delete()
}

/// The copy in a Keychain item, which the device can read after the first
/// unlock, so the Notification Service Extension reads it while the phone
/// is locked. The item is in the Keychain access group that the
/// `PagisKeychainAccessGroup` key of `Info.plist` names, which the
/// extension shares.
final class KeychainSessionCopy: SessionCopy {
    private let service: String
    private let accessGroup: String?

    init(
        service: String = "co.pagis.mobile.session",
        accessGroup: String? = Bundle.main.object(forInfoDictionaryKey: "PagisKeychainAccessGroup") as? String
    ) {
        self.service = service
        self.accessGroup = accessGroup
    }

    func read(for origin: WebOrigin) -> Session? {
        var query = baseQuery()
        query[kSecAttrAccount as String] = origin.serverURL
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data
        else { return nil }
        return try? JSONDecoder().decode(Session.self, from: data)
    }

    func write(_ session: Session, for origin: WebOrigin) {
        guard let data = try? JSONEncoder().encode(session) else { return }
        // One copy at a time: the copy of each other origin goes.
        delete()
        var item = baseQuery()
        item[kSecAttrAccount as String] = origin.serverURL
        item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        let status = SecItemAdd(item as CFDictionary, nil)
        if status != errSecSuccess {
            NSLog("Pagis did not keep the copy of the Session: Keychain status %d", status)
        }
    }

    func delete() {
        SecItemDelete(baseQuery() as CFDictionary)
    }

    private func baseQuery() -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
        ]
        if let accessGroup, !accessGroup.isEmpty {
            query[kSecAttrAccessGroup as String] = accessGroup
        }
        return query
    }
}
