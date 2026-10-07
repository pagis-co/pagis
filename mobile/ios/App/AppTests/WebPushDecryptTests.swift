import CryptoKit
import XCTest
@testable import App

/// `decrypt` opens the one `aes128gcm` record of a Web Push (RFC 8291,
/// RFC 8188) with the keys of the Push Subscription.
final class WebPushDecryptTests: XCTestCase {
    func testTheVectorOfRFC8291AppendixAGivesItsPlaintext() throws {
        let plaintext = try decrypt(
            body: RFC8291Vector.body,
            privateKey: RFC8291Vector.receiverPrivateKey,
            auth: RFC8291Vector.auth
        )

        XCTAssertEqual(String(decoding: plaintext, as: UTF8.self), "When I grow up, I want to be a watermelon")
        XCTAssertEqual(plaintext, RFC8291Vector.plaintext)
    }

    func testTheFixtureOfPagisPushGivesItsPlaintext() throws {
        let fixture = try WebPushFixture.load()

        let plaintext = try decrypt(body: fixture.body, privateKey: fixture.privateKey, auth: fixture.auth)

        XCTAssertEqual(String(decoding: plaintext, as: UTF8.self), fixture.plaintext)
    }

    func testThePaddingAfterTheDelimiterIsRemoved() throws {
        let body = RFC8291Vector.body(sealing: Data("hello".utf8) + [0x02, 0x00, 0x00, 0x00])

        let plaintext = try decrypt(body: body, privateKey: RFC8291Vector.receiverPrivateKey, auth: RFC8291Vector.auth)

        XCTAssertEqual(plaintext, Data("hello".utf8))
    }

    func testAWrongAuthSecretIsAnError() {
        assertDecrypt(RFC8291Vector.body, auth: Data(repeating: 7, count: 16), throws: .notAuthentic)
    }

    func testAnotherPrivateKeyIsAnError() {
        let body = RFC8291Vector.body
        XCTAssertThrowsError(
            try decrypt(body: body, privateKey: P256.KeyAgreement.PrivateKey(), auth: RFC8291Vector.auth)
        ) { error in
            XCTAssertEqual(error as? WebPushDecryptError, .notAuthentic)
        }
    }

    func testACutBodyIsAnError() {
        let body = RFC8291Vector.body
        assertDecrypt(body.dropLast(1), throws: .notAuthentic)
        assertDecrypt(body.prefix(RFC8291Vector.header.count + 16), throws: .shortBody)
        assertDecrypt(RFC8291Vector.header, throws: .shortBody)
        assertDecrypt(body.prefix(40), throws: .shortBody)
        assertDecrypt(Data(), throws: .shortBody)
    }

    func testABadHeaderIsAnError() {
        var shortKeyID = RFC8291Vector.body
        shortKeyID[20] = 64
        assertDecrypt(shortKeyID, throws: .badKeyID)

        var notAPoint = RFC8291Vector.body
        notAPoint[21] = 0x05
        assertDecrypt(notAPoint, throws: .badKeyID)

        var tinyRecords = RFC8291Vector.body
        tinyRecords.replaceSubrange(16..<20, with: [0, 0, 0, 17])
        assertDecrypt(tinyRecords, throws: .badRecordSize)
    }

    func testASecondRecordIsAnError() {
        // The record size is 30, so the 58 bytes of ciphertext are two
        // records.
        var body = RFC8291Vector.body
        body.replaceSubrange(16..<20, with: [0, 0, 0, 30])

        assertDecrypt(body, throws: .secondRecord)
    }

    func testAWrongPaddingDelimiterIsAnError() {
        // 0x01 ends a record that is not the last one (RFC 8188).
        assertDecrypt(RFC8291Vector.body(sealing: RFC8291Vector.plaintext + [0x01]), throws: .badPadding)
        assertDecrypt(RFC8291Vector.body(sealing: RFC8291Vector.plaintext + [0x02, 0x07]), throws: .badPadding)
        assertDecrypt(RFC8291Vector.body(sealing: Data([0x00, 0x00])), throws: .badPadding)
    }

    private func assertDecrypt(
        _ body: Data,
        auth: Data = RFC8291Vector.auth,
        throws expected: WebPushDecryptError,
        line: UInt = #line
    ) {
        XCTAssertThrowsError(
            try decrypt(body: body, privateKey: RFC8291Vector.receiverPrivateKey, auth: auth),
            line: line
        ) { error in
            XCTAssertEqual(error as? WebPushDecryptError, expected, line: line)
        }
    }
}

/// `fixtures/web-push.json` of the repository: the keys of a Push
/// Subscription, one body that `pagis-push` encrypted for it, and its
/// plaintext. The test bundle holds a copy of the file.
struct WebPushFixture {
    let privateKey: P256.KeyAgreement.PrivateKey
    let p256dh: Data
    let auth: Data
    let body: Data
    let plaintext: String

    private struct File: Decodable {
        struct Subscription: Decodable {
            let private_key: String
            let p256dh: String
            let auth: String
        }

        let subscription: Subscription
        let body: String
        let plaintext: String
    }

    static func load() throws -> WebPushFixture {
        let url = try XCTUnwrap(
            Bundle(for: WebPushDecryptTests.self).url(forResource: "web-push", withExtension: "json"),
            "The test bundle holds no web-push.json."
        )
        let file = try JSONDecoder().decode(File.self, from: Data(contentsOf: url))
        return WebPushFixture(
            privateKey: try P256.KeyAgreement.PrivateKey(
                rawRepresentation: try XCTUnwrap(Data(base64URLEncoded: file.subscription.private_key))
            ),
            p256dh: try XCTUnwrap(Data(base64URLEncoded: file.subscription.p256dh)),
            auth: try XCTUnwrap(Data(base64URLEncoded: file.subscription.auth)),
            body: try XCTUnwrap(Data(base64URLEncoded: file.body)),
            plaintext: file.plaintext
        )
    }
}
