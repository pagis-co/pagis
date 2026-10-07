import XCTest
@testable import App

/// The keys of the Push Subscription of the app (RFC 8291). The private
/// key never leaves the app; the subscription gives the public key and
/// the auth secret.
final class PushKeysTests: XCTestCase {
    func testTheKeysGiveAnUncompressedPointAndSixteenBytesOfAuth() throws {
        let keys = try PushKeyStore(items: MemoryItems()).keys()

        XCTAssertEqual(keys.p256dh.count, 65)
        XCTAssertEqual(keys.p256dh.first, 0x04)
        XCTAssertEqual(keys.auth.count, 16)
    }

    func testASecondReadGivesTheSameStoredKeys() throws {
        let items = MemoryItems()
        let first = try PushKeyStore(items: items).keys()

        let second = try PushKeyStore(items: items).keys()

        XCTAssertEqual(second.p256dh, first.p256dh)
        XCTAssertEqual(second.auth, first.auth)
        XCTAssertEqual(second.privateKey.rawRepresentation, first.privateKey.rawRepresentation)
    }

    func testDeletedKeysAreMadeAgain() throws {
        let store = PushKeyStore(items: MemoryItems())
        let first = try store.keys()

        try store.delete()

        let second = try store.keys()
        XCTAssertNotEqual(second.p256dh, first.p256dh)
        XCTAssertNotEqual(second.auth, first.auth)
    }

    func testBase64URLHasNoPadding() {
        XCTAssertEqual(Data([0xfb, 0xff]).base64URLEncodedString(), "-_8")
        XCTAssertEqual(Data(repeating: 0, count: 16).base64URLEncodedString(), "AAAAAAAAAAAAAAAAAAAAAA")
    }
}
