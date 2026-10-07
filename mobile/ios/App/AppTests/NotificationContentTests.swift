import UserNotifications
import XCTest
@testable import App

/// `NotificationContent` gives the content that the Notification Service
/// Extension delivers: the decrypted payload, or the placeholder of the
/// Push Relay with the server origin.
final class NotificationContentTests: XCTestCase {
    private let origin = "https://pagis.example.com"

    func testAnApprovalShowsEachFieldOfThePayload() {
        let content = NotificationContent.content(
            for: placeholder(holding: PushPayloadJSON.approval()),
            keys: RFC8291Vector.keys,
            origin: origin
        )

        XCTAssertEqual(content.title, "Robin")
        XCTAssertEqual(content.body, "Robin needs your approval\nhost_shell")
        XCTAssertEqual(content.threadIdentifier, "approval")
        XCTAssertEqual(content.badge, 3)
        XCTAssertEqual(content.categoryIdentifier, "approval")
        XCTAssertEqual(content.sound, .default)
        XCTAssertEqual(content.userInfo as NSDictionary, [
            "navigate": "https://pagis.example.com/c/ch-1",
            "item": "request:r-1",
            "kind": "approval",
            "request": ["id": "r-1", "actions": ["approve_once", "deny"]],
        ] as NSDictionary)
    }

    func testAnotherKindHasItsThreadAndNoCategory() {
        let content = NotificationContent.content(
            for: placeholder(holding: PushPayloadJSON.approval { message in
                message[path: "notification", "data", "kind"] = "waiting"
                message[path: "notification", "data", "item"] = "run:run-1"
                message[path: "notification", "data", "request"] = nil
            }),
            keys: RFC8291Vector.keys,
            origin: origin
        )

        XCTAssertEqual(content.threadIdentifier, "waiting")
        XCTAssertEqual(content.categoryIdentifier, "")
        XCTAssertEqual(content.userInfo as NSDictionary, [
            "navigate": "https://pagis.example.com/c/ch-1",
            "item": "run:run-1",
            "kind": "waiting",
        ] as NSDictionary)
    }

    func testARequestWithOtherActionsHasNoCategory() {
        let content = NotificationContent.content(
            for: placeholder(holding: PushPayloadJSON.approval {
                $0[path: "notification", "data", "request", "actions"] = ["deny"]
            }),
            keys: RFC8291Vector.keys,
            origin: origin
        )

        XCTAssertEqual(content.categoryIdentifier, "")
    }

    func testAPayloadWithNoBadgeLeavesTheBadge() {
        let content = NotificationContent.content(
            for: placeholder(holding: PushPayloadJSON.approval { $0["app_badge"] = nil }),
            keys: RFC8291Vector.keys,
            origin: origin
        )

        XCTAssertNil(content.badge)
    }

    func testABodyThatDoesNotDecryptShowsThePlaceholderWithTheOrigin() {
        let wrongKeys = PushKeys(privateKey: RFC8291Vector.receiverPrivateKey, auth: Data(repeating: 1, count: 16))

        let content = NotificationContent.content(
            for: placeholder(holding: PushPayloadJSON.approval()),
            keys: wrongKeys,
            origin: origin
        )

        assertPlaceholder(content, navigate: origin)
    }

    func testABodyThatDoesNotParseShowsThePlaceholderWithTheOrigin() {
        let bodies = [
            Data("not json".utf8),
            PushPayloadJSON.approval { $0[path: "notification", "title"] = 42 },
            PushPayloadJSON.approval { $0[path: "notification", "data", "v"] = 2 },
        ]
        for body in bodies {
            let content = NotificationContent.content(
                for: placeholder(holding: body),
                keys: RFC8291Vector.keys,
                origin: origin
            )

            assertPlaceholder(content, navigate: origin)
        }
    }

    func testAPushWithNoReadableBodyShowsThePlaceholder() {
        let noBody = placeholder(userInfo: ["aps": ["mutable-content": 1]])
        let notBase64 = placeholder(userInfo: ["p": "%%%"])

        for content in [noBody, notBase64] {
            assertPlaceholder(
                NotificationContent.content(for: content, keys: RFC8291Vector.keys, origin: origin),
                navigate: origin
            )
        }
    }

    func testNoKeysShowsThePlaceholder() {
        let content = NotificationContent.content(
            for: placeholder(holding: PushPayloadJSON.approval()),
            keys: nil,
            origin: origin
        )

        assertPlaceholder(content, navigate: origin)
    }

    func testNoStoredOriginShowsThePlaceholderWithNoPlace() {
        let content = NotificationContent.content(
            for: placeholder(holding: Data("not json".utf8)),
            keys: RFC8291Vector.keys,
            origin: nil
        )

        XCTAssertEqual(content.title, "Pagis")
        XCTAssertEqual(content.userInfo as NSDictionary, [:] as NSDictionary)
    }

    /// The content that APNs gives the extension: the placeholder of the
    /// Push Relay and the body `p` that holds `plaintext`.
    private func placeholder(holding plaintext: Data) -> UNNotificationContent {
        placeholder(userInfo: [
            "aps": ["alert": ["title": "Pagis", "body": "Something needs you"], "mutable-content": 1, "sound": "default"],
            "p": RFC8291Vector.body(holding: plaintext).base64URLEncodedString(),
        ])
    }

    private func placeholder(userInfo: [AnyHashable: Any]) -> UNNotificationContent {
        let content = UNMutableNotificationContent()
        content.title = "Pagis"
        content.body = "Something needs you"
        content.sound = .default
        content.userInfo = userInfo
        return content
    }

    private func assertPlaceholder(_ content: UNNotificationContent, navigate: String, line: UInt = #line) {
        XCTAssertEqual(content.title, "Pagis", line: line)
        XCTAssertEqual(content.body, "Something needs you", line: line)
        XCTAssertEqual(content.sound, .default, line: line)
        XCTAssertEqual(content.categoryIdentifier, "", line: line)
        XCTAssertNil(content.badge, line: line)
        XCTAssertEqual(content.userInfo as NSDictionary, ["navigate": navigate] as NSDictionary, line: line)
    }
}
