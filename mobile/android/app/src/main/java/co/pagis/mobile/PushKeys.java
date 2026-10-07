package co.pagis.mobile;

import java.util.Base64;

/**
 * The keys of the Push Subscription of the app (RFC 8291): a P-256 key
 * pair and a 16-byte auth secret. The private key never leaves the app.
 * The messaging service reads it to decrypt a push.
 */
final class PushKeys {

    /** The private key in PKCS#8. */
    final byte[] privateKey;
    /** The public key as the 65-byte uncompressed point, {@code 0x04 || X || Y}. */
    final byte[] publicKey;
    final byte[] auth;

    PushKeys(byte[] privateKey, byte[] publicKey, byte[] auth) {
        this.privateKey = privateKey;
        this.publicKey = publicKey;
        this.auth = auth;
    }

    /** The public key as base64url with no padding, as Web Push gives {@code p256dh}. */
    String p256dh() {
        return base64Url(publicKey);
    }

    /** The auth secret as base64url with no padding, as Web Push gives {@code auth}. */
    String authText() {
        return base64Url(auth);
    }

    private static String base64Url(byte[] bytes) {
        return Base64.getUrlEncoder().withoutPadding().encodeToString(bytes);
    }
}
