import Foundation

/// The APNs environment that this build of the app gets its token from. A
/// debug build talks to the sandbox.
enum ApnsEnvironment: String {
    case sandbox
    case production
}

/// The registration of this installation with the Push Relay
/// (ADR-0030): what the relay answered, and the VAPID Key and the token
/// that the app registered.
struct RelayRegistration: Codable, Equatable {
    let id: String
    /// Changes the token and removes the registration. Only the app holds
    /// it.
    let secret: String
    /// The Web Push endpoint that the daemon posts to.
    let endpoint: String
    let vapidKey: String
    var token: String
}

/// The routes of the Push Relay that register an installation.
final class RelayClient {
    private let origin: URL
    private let environment: ApnsEnvironment
    private let session: URLSession

    init(origin: URL, environment: ApnsEnvironment, session: URLSession = .shared) {
        self.origin = origin
        self.environment = environment
        self.session = session
    }

    /// `POST /v1/registrations`, which answers `{id, secret, endpoint}`.
    func register(token: String, vapidKey: String) async throws -> RelayRegistration {
        let body = [
            "platform": "ios",
            "environment": environment.rawValue,
            "token": token,
            "vapid_key": vapidKey,
        ]
        let (data, status) = try await send("POST", "/v1/registrations", body: body)
        guard status == 201 else { throw PushError.relay(status: status) }
        struct Registered: Decodable {
            let id: String
            let secret: String
            let endpoint: String
        }
        guard let registered = try? JSONDecoder().decode(Registered.self, from: data) else {
            throw PushError.relay(status: status)
        }
        return RelayRegistration(
            id: registered.id,
            secret: registered.secret,
            endpoint: registered.endpoint,
            vapidKey: vapidKey,
            token: token
        )
    }

    /// `PUT /v1/registrations/<id>`. False when the relay does not know
    /// the registration.
    func changeToken(of registration: RelayRegistration, to token: String) async throws -> Bool {
        let (_, status) = try await send(
            "PUT",
            "/v1/registrations/\(registration.id)",
            body: ["token": token],
            secret: registration.secret
        )
        switch status {
        case 204: return true
        case 404: return false
        default: throw PushError.relay(status: status)
        }
    }

    /// `DELETE /v1/registrations/<id>`. A registration that the relay does
    /// not know is already gone.
    func delete(_ registration: RelayRegistration) async throws {
        let (_, status) = try await send("DELETE", "/v1/registrations/\(registration.id)", secret: registration.secret)
        guard status == 204 || status == 404 else { throw PushError.relay(status: status) }
    }

    private func send(
        _ method: String,
        _ path: String,
        body: [String: String]? = nil,
        secret: String? = nil
    ) async throws -> (Data, Int) {
        var request = URLRequest(url: origin.appendingPathComponent(path), timeoutInterval: 10)
        request.httpMethod = method
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        if let secret {
            request.setValue("Bearer \(secret)", forHTTPHeaderField: "Authorization")
        }
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            throw PushError.unreachable(error.localizedDescription)
        }
        guard let http = response as? HTTPURLResponse else { throw PushError.relay(status: 0) }
        return (data, http.statusCode)
    }
}

/// Keeps the registration with the Push Relay in the secret items of the
/// app, because it holds the secret.
final class RelayRegistrationStore {
    /// The name of the item that holds the registration.
    static let item = "push.registration"

    private let items: SecretItems

    init(items: SecretItems) {
        self.items = items
    }

    func read() throws -> RelayRegistration? {
        try items.read(RelayRegistrationStore.item).map { try JSONDecoder().decode(RelayRegistration.self, from: $0) }
    }

    func write(_ registration: RelayRegistration) throws {
        try items.write(try JSONEncoder().encode(registration), as: RelayRegistrationStore.item)
    }

    func delete() throws {
        try items.delete(RelayRegistrationStore.item)
    }
}
