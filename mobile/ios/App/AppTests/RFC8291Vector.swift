import CryptoKit
import Foundation
@testable import App

/// The example of RFC 8291, section 5 and Appendix A: the keys, the salt,
/// the auth secret, the body and the plaintext.
enum RFC8291Vector {
    static let plaintext = data("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24")

    /// The application server public key, `as_public`.
    static let senderPublicKey = data("BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8")
    /// The user agent private key, `ua_private`.
    static let receiverPrivateKey = try! P256.KeyAgreement.PrivateKey(
        rawRepresentation: data("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94")
    )
    /// The user agent public key, `ua_public`.
    static let receiverPublicKey = data("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4")
    static let salt = data("DGv6ra1nlYgDCS1FRnbzlw")
    static let auth = data("BTBZMqHH6r4Tts7J_aSIgg")

    /// The content encryption key and the nonce that Appendix A derives.
    static let cek = SymmetricKey(data: data("oIhVW04MRdy2XN9CiKLxTg"))
    static let nonce = try! AES.GCM.Nonce(data: data("4h_95klXJ5E_qnoN"))

    /// The 86-octet header: the salt, the record size 4096 and `as_public`.
    static let header = data("DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8")
    static let ciphertext = data("8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ")

    /// The body of section 5: the header and the ciphertext.
    static let body = header + ciphertext

    /// The keys of the user agent, as the Keychain gives them.
    static let keys = PushKeys(privateKey: receiverPrivateKey, auth: auth)

    /// A body with the header of the example that holds `record`, sealed
    /// with the key and the nonce of the example. The keys of the user
    /// agent open it.
    static func body(sealing record: Data, header: Data = header) -> Data {
        let sealed = try! AES.GCM.seal(record, using: cek, nonce: nonce)
        return header + sealed.ciphertext + sealed.tag
    }

    /// A body that holds `plaintext` and the delimiter of the last record.
    static func body(holding plaintext: Data) -> Data {
        body(sealing: plaintext + [0x02])
    }

    private static func data(_ base64URL: String) -> Data {
        Data(base64URLEncoded: base64URL)!
    }
}
