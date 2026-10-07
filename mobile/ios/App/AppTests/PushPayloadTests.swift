import XCTest
@testable import App

/// `PushPayload` reads the Declarative Web Push JSON of ADR-0030.
final class PushPayloadTests: XCTestCase {
    func testAnApprovalGivesEachField() throws {
        let payload = try PushPayload(json: PushPayloadJSON.approval())

        XCTAssertEqual(payload, PushPayload(
            title: "Robin",
            body: "Robin needs your approval\nhost_shell",
            navigate: "https://pagis.example.com/c/ch-1",
            badge: 3,
            item: "request:r-1",
            kind: "approval",
            request: PushPayload.Request(id: "r-1", actions: ["approve_once", "deny"])
        ))
    }

    func testThePayloadOfTheTestNotificationHasNoBadgeAndNoRequest() throws {
        let payload = try PushPayload(json: PushPayloadJSON.json([
            "web_push": 8030,
            "notification": [
                "title": "Pagis",
                "body": "Notifications work here.",
                "navigate": "https://pagis.example.com/settings/notifications",
                "data": ["v": 1, "item": "test", "kind": "test"],
            ],
            "mutable": true,
        ]))

        XCTAssertNil(payload.badge)
        XCTAssertNil(payload.request)
        XCTAssertEqual(payload.kind, "test")
        XCTAssertEqual(payload.item, "test")
    }

    func testAnUnknownFieldIsIgnored() throws {
        let payload = try PushPayload(json: PushPayloadJSON.approval { message in
            message["lang"] = "en"
            var notification = message["notification"] as! [String: Any]
            notification["silent"] = false
            var data = notification["data"] as! [String: Any]
            data["later"] = ["a": 1]
            notification["data"] = data
            message["notification"] = notification
        })

        XCTAssertEqual(payload.title, "Robin")
    }

    func testAFieldOfTheWrongTypeIsAnError() {
        let changes: [(String, (inout [String: Any]) -> Void)] = [
            ("title", { $0[path: "notification", "title"] = 42 }),
            ("body", { $0[path: "notification", "body"] = ["text"] }),
            ("navigate", { $0[path: "notification", "navigate"] = false }),
            ("app_badge", { $0["app_badge"] = "3" }),
            ("item", { $0[path: "notification", "data", "item"] = 1 }),
            ("kind", { $0[path: "notification", "data", "kind"] = NSNull() }),
            ("request", { $0[path: "notification", "data", "request"] = "r-1" }),
            ("actions", { $0[path: "notification", "data", "request", "actions"] = "approve_once" }),
            ("v", { $0[path: "notification", "data", "v"] = "1" }),
        ]
        for (field, change) in changes {
            XCTAssertThrowsError(try PushPayload(json: PushPayloadJSON.approval(change)), field)
        }
    }

    func testAMissingFieldIsAnError() {
        let changes: [(String, (inout [String: Any]) -> Void)] = [
            ("title", { $0[path: "notification", "title"] = nil }),
            ("navigate", { $0[path: "notification", "navigate"] = nil }),
            ("data", { $0[path: "notification", "data"] = nil }),
            ("notification", { $0["notification"] = nil }),
        ]
        for (field, change) in changes {
            XCTAssertThrowsError(try PushPayload(json: PushPayloadJSON.approval(change)), field)
        }
    }

    func testAnotherVersionIsAnError() {
        XCTAssertThrowsError(
            try PushPayload(json: PushPayloadJSON.approval { $0[path: "notification", "data", "v"] = 2 })
        ) { error in
            XCTAssertEqual(error as? PushPayloadError, .unknownVersion(2))
        }
    }

    func testAMessageThatIsNotADeclarativeWebPushIsAnError() {
        XCTAssertThrowsError(try PushPayload(json: PushPayloadJSON.approval { $0["web_push"] = 8291 })) { error in
            XCTAssertEqual(error as? PushPayloadError, .notDeclarativeWebPush)
        }
        XCTAssertThrowsError(try PushPayload(json: Data("not json".utf8)))
    }
}

/// The JSON of ADR-0030, as the daemon makes it.
enum PushPayloadJSON {
    /// The payload of a tool action Approval, after `change`.
    static func approval(_ change: (inout [String: Any]) -> Void = { _ in }) -> Data {
        var message: [String: Any] = [
            "web_push": 8030,
            "notification": [
                "title": "Robin",
                "body": "Robin needs your approval\nhost_shell",
                "navigate": "https://pagis.example.com/c/ch-1",
                "data": [
                    "v": 1,
                    "item": "request:r-1",
                    "kind": "approval",
                    "request": ["id": "r-1", "actions": ["approve_once", "deny"]],
                ],
            ],
            "app_badge": 3,
            "mutable": true,
        ]
        change(&message)
        return json(message)
    }

    static func json(_ message: [String: Any]) -> Data {
        try! JSONSerialization.data(withJSONObject: message)
    }
}

extension Dictionary where Key == String, Value == Any {
    /// The value at the path of keys through nested objects. Setting nil
    /// removes the last key.
    subscript(path keys: String...) -> Any? {
        get {
            keys.dropLast().reduce(self as [String: Any]?) { object, key in object?[key] as? [String: Any] }?[keys.last!]
        }
        set {
            self = Dictionary.setting(newValue, at: ArraySlice(keys), in: self)
        }
    }

    private static func setting(_ value: Any?, at keys: ArraySlice<String>, in object: [String: Any]) -> [String: Any] {
        var object = object
        let key = keys.first!
        if keys.count == 1 {
            object[key] = value
        } else {
            object[key] = setting(value, at: keys.dropFirst(), in: object[key] as? [String: Any] ?? [:])
        }
        return object
    }
}
