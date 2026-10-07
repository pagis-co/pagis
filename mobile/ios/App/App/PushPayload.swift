import Foundation

/// Why a decrypted push is not a payload that the app reads.
enum PushPayloadError: Error, Equatable {
    /// The message has no `"web_push": 8030`.
    case notDeclarativeWebPush
    /// `notification.data.v` is a version of the payload format that this
    /// build does not read.
    case unknownVersion(Int)
}

/// The plaintext of a Notification: the Declarative Web Push JSON with
/// the Pagis fields in `notification.data` (ADR-0030). A field of the
/// wrong type is an error, and an unknown field is ignored.
struct PushPayload: Equatable {
    /// The Request that the Notification can answer.
    struct Request: Equatable, Decodable {
        let id: String
        let actions: [String]
    }

    /// The version of the payload format that this build reads.
    static let version = 1

    let title: String
    let body: String
    /// The absolute URL of the place of the item in the Product App.
    let navigate: String
    /// The count of the Needs-You Queue, or nil to leave the badge.
    let badge: Int?
    /// The id of the queue item.
    let item: String
    /// A queue kind, or `test`.
    let kind: String
    let request: Request?

    init(json: Data) throws {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let versioned = try decoder.decode(Versioned.self, from: json)
        guard versioned.webPush == 8030 else { throw PushPayloadError.notDeclarativeWebPush }
        let version = versioned.notification.data.v
        guard version == PushPayload.version else { throw PushPayloadError.unknownVersion(version) }
        let message = try decoder.decode(Message.self, from: json)
        self.init(
            title: message.notification.title,
            body: message.notification.body,
            navigate: message.notification.navigate,
            badge: message.appBadge,
            item: message.notification.data.item,
            kind: message.notification.data.kind,
            request: message.notification.data.request
        )
    }

    init(title: String, body: String, navigate: String, badge: Int?, item: String, kind: String, request: Request?) {
        self.title = title
        self.body = body
        self.navigate = navigate
        self.badge = badge
        self.item = item
        self.kind = kind
        self.request = request
    }
}

/// The members that name the format and its version. A later version can
/// change every other member.
private struct Versioned: Decodable {
    struct Notification: Decodable {
        struct Fields: Decodable { let v: Int }
        let data: Fields
    }

    let webPush: Int
    let notification: Notification
}

/// The members of version 1.
private struct Message: Decodable {
    struct Notification: Decodable {
        struct Fields: Decodable {
            let item: String
            let kind: String
            let request: PushPayload.Request?
        }

        let title: String
        let body: String
        let navigate: String
        let data: Fields
    }

    let notification: Notification
    let appBadge: Int?
}
