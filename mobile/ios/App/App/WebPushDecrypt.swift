import CryptoKit
import Foundation

/// Why a Web Push body does not decrypt.
enum WebPushDecryptError: Error, Equatable {
    /// The body is shorter than its header, or than one record.
    case shortBody
    /// The record size of the header is under the 18 bytes of RFC 8188.
    case badRecordSize
    /// The key id of the header is not the 65-byte P-256 point of the
    /// sender.
    case badKeyID
    /// The body holds more than one record. A Web Push holds one.
    case secondRecord
    /// AES-GCM does not open the record: another key, another auth secret
    /// or a changed body.
    case notAuthentic
    /// The record does not end with the delimiter `0x02` of the last
    /// record and zero bytes of padding.
    case badPadding
}

/// The length of the salt, the record size and the key id length of an
/// `aes128gcm` header (RFC 8188, section 2.1).
private let fixedHeader = 16 + 4 + 1
/// The length of a P-256 point in the uncompressed form.
private let pointLength = 65
/// The length of the AES-GCM tag.
private let tagLength = 16

/// The plaintext of a Web Push `body`, a single `aes128gcm` record
/// (RFC 8188) that the sender encrypted for the key pair of `privateKey`
/// and the auth secret `auth` (RFC 8291).
func decrypt(body: Data, privateKey: P256.KeyAgreement.PrivateKey, auth: Data) throws -> Data {
    let bytes = [UInt8](body)
    guard bytes.count >= fixedHeader else { throw WebPushDecryptError.shortBody }
    let salt = Data(bytes[0..<16])
    let recordSize = bytes[16..<20].reduce(0) { $0 << 8 | Int($1) }
    guard recordSize >= tagLength + 2 else { throw WebPushDecryptError.badRecordSize }
    let keyIDLength = Int(bytes[20])
    guard bytes.count >= fixedHeader + keyIDLength else { throw WebPushDecryptError.shortBody }
    guard keyIDLength == pointLength,
          let senderKey = try? P256.KeyAgreement.PublicKey(
              x963Representation: bytes[fixedHeader..<fixedHeader + pointLength]
          )
    else { throw WebPushDecryptError.badKeyID }
    let record = Data(bytes[(fixedHeader + keyIDLength)...])
    // The tag and the delimiter at least.
    guard record.count > tagLength else { throw WebPushDecryptError.shortBody }
    guard record.count <= recordSize else { throw WebPushDecryptError.secondRecord }

    // RFC 8291, section 3.4: the IKM from the ECDH secret and the auth
    // secret, then the key and the nonce of the content from the salt.
    let ecdhSecret = try privateKey.sharedSecretFromKeyAgreement(with: senderKey)
    let keyInfo = Data("WebPush: info".utf8) + [0x00]
        + privateKey.publicKey.x963Representation + senderKey.x963Representation
    let ikm = ecdhSecret.hkdfDerivedSymmetricKey(
        using: SHA256.self, salt: auth, sharedInfo: keyInfo, outputByteCount: 32
    )
    let cek = HKDF<SHA256>.deriveKey(
        inputKeyMaterial: ikm, salt: salt,
        info: Data("Content-Encoding: aes128gcm".utf8) + [0x00], outputByteCount: 16
    )
    let nonce = HKDF<SHA256>.deriveKey(
        inputKeyMaterial: ikm, salt: salt,
        info: Data("Content-Encoding: nonce".utf8) + [0x00], outputByteCount: 12
    )

    let padded: Data
    do {
        let box = try AES.GCM.SealedBox(
            nonce: AES.GCM.Nonce(data: nonce.withUnsafeBytes { Data($0) }),
            ciphertext: record.dropLast(tagLength),
            tag: record.suffix(tagLength)
        )
        padded = try AES.GCM.open(box, using: cek)
    } catch {
        throw WebPushDecryptError.notAuthentic
    }

    // RFC 8188, section 2: the data, the delimiter, then zero bytes.
    guard let delimiter = padded.lastIndex(where: { $0 != 0 }), padded[delimiter] == 0x02 else {
        throw WebPushDecryptError.badPadding
    }
    return Data(padded[padded.startIndex..<delimiter])
}
