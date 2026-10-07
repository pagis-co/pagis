import Foundation

/// What the phone gives for push: the permission of the Person and the
/// APNs token.
@MainActor
protocol PushPlatform: AnyObject {
    /// Ask the Person to allow notifications. True when they are allowed.
    func requestPermission() async throws -> Bool
    /// The APNs token of this installation.
    func token() async throws -> String
}

/// The Push Subscription that the Product App posts to the daemon: the
/// shape of `PushSubscription.toJSON()`, with the keys as base64url.
struct PushSubscription: Equatable {
    let endpoint: String
    let p256dh: String
    let auth: String
}

/// Why the app has no Push Subscription. The Product App shows the
/// description to the Person.
enum PushError: LocalizedError, Equatable {
    case notAllowed
    case relay(status: Int)
    case unreachable(String)

    var errorDescription: String? {
        switch self {
        case .notAllowed:
            return "Pagis cannot show notifications on this phone. Allow them in the Settings app of the phone."
        case .relay(let status):
            return "The Push Relay did not take the registration (status \(status))."
        case .unreachable(let reason):
            return "Pagis cannot reach the Push Relay: \(reason)"
        }
    }
}

/// The Push Subscription of the app through the Push Relay (ADR-0032).
/// The relay gives the APNs token a Web Push endpoint, and the app makes
/// the keys that the daemon encrypts each push to.
@MainActor
final class PushSubscriber {
    private let relay: RelayClient
    private let registrations: RelayRegistrationStore
    private let keys: PushKeyStore

    init(relay: RelayClient, registrations: RelayRegistrationStore, keys: PushKeyStore) {
        self.relay = relay
        self.registrations = registrations
        self.keys = keys
    }

    /// The subscriber of the app: the relay of the build constant
    /// `PUSH_RELAY_ORIGIN`, and the Keychain.
    static let app: PushSubscriber = {
        let items = KeychainItems()
        return PushSubscriber(
            relay: RelayClient(origin: AppBuild.pushRelayOrigin, environment: AppBuild.isDebug ? .sandbox : .production),
            registrations: RelayRegistrationStore(items: items),
            keys: PushKeyStore(items: items)
        )
    }()

    /// Ask for the permission, get the token, register with the relay for
    /// `vapidKey`, make the keys, and answer the subscription. With a
    /// registration for the same key, it registers nothing: it sends a
    /// token that changed, and answers the stored values. A registration
    /// for another key goes first, with its keys.
    func subscribe(vapidKey: String, platform: PushPlatform) async throws -> PushSubscription {
        guard try await platform.requestPermission() else { throw PushError.notAllowed }
        let token = try await platform.token()
        var registration = try registrations.read()
        if let stored = registration, stored.vapidKey != vapidKey {
            try await unsubscribe()
            registration = nil
        }
        if let stored = registration, stored.token != token {
            registration = try await changeToken(of: stored, to: token)
        }
        let current: RelayRegistration
        if let registration {
            current = registration
        } else {
            current = try await relay.register(token: token, vapidKey: vapidKey)
            try registrations.write(current)
        }
        let keys = try keys.keys()
        return PushSubscription(
            endpoint: current.endpoint,
            p256dh: keys.p256dh.base64URLEncodedString(),
            auth: keys.auth.base64URLEncodedString()
        )
    }

    /// APNs gave a token. A token that changed goes to the relay with
    /// `PUT`, and the endpoint stays the same.
    func tokenChanged(_ token: String) async throws {
        guard let stored = try registrations.read(), stored.token != token else { return }
        _ = try await changeToken(of: stored, to: token)
    }

    /// Delete the registration with the relay, then the keys.
    func unsubscribe() async throws {
        if let stored = try registrations.read() {
            try await relay.delete(stored)
        }
        try forget()
    }

    /// The registration with the new token, or nil when the relay no
    /// longer knows it. The relay removes a registration when APNs says
    /// that its token is gone, and the app then forgets it and its keys.
    private func changeToken(of stored: RelayRegistration, to token: String) async throws -> RelayRegistration? {
        guard try await relay.changeToken(of: stored, to: token) else {
            try forget()
            return nil
        }
        var changed = stored
        changed.token = token
        try registrations.write(changed)
        return changed
    }

    private func forget() throws {
        try registrations.delete()
        try keys.delete()
    }
}
