package co.pagis.mobile;

import com.google.crypto.tink.apps.webpush.WebPushHybridDecrypt;
import java.nio.ByteBuffer;
import java.security.GeneralSecurityException;
import java.security.KeyFactory;
import java.security.interfaces.ECPrivateKey;
import java.security.spec.PKCS8EncodedKeySpec;

/**
 * Decrypts the body of a Web Push (RFC 8291) with the keys of the Push
 * Subscription of the app. The body is one {@code aes128gcm} record
 * (RFC 8188): the salt, the record size, the key of the sender, then the
 * record. The {@code apps-webpush} module of Tink does the key agreement,
 * the key derivation, AES-128-GCM and the padding.
 */
final class WebPushDecrypt {

    private static final int SALT_BYTES = 16;
    /** The salt, the record size, the key id length and the 65-byte key of the sender. */
    private static final int HEADER_BYTES = SALT_BYTES + 4 + 1 + 65;
    /** The smallest record of RFC 8188: the tag and the delimiter. */
    private static final int MIN_RECORD_SIZE = 18;
    /**
     * The record size that Tink reads. Tink takes a body only when the
     * record size of its header is this value, and it then holds the body
     * as one record of at most 4096 bytes with its header.
     */
    private static final int TINK_RECORD_SIZE = 4096;

    private WebPushDecrypt() {}

    /**
     * The plaintext of {@code body}, which a sender encrypted for the
     * public key and the auth secret of {@code keys}.
     *
     * {@code pagis-push} writes the length of the one record as the record
     * size, and Tink refuses each record size other than the one that its
     * builder holds. The record size only marks where a record ends, and
     * the encryption does not cover it. So the app checks that the body is
     * one record, and gives Tink a copy with its record size.
     */
    static byte[] decrypt(byte[] body, PushKeys keys) throws GeneralSecurityException {
        if (body.length < HEADER_BYTES) {
            throw new GeneralSecurityException("The body is shorter than its header.");
        }
        long recordSize = Integer.toUnsignedLong(ByteBuffer.wrap(body).getInt(SALT_BYTES));
        if (recordSize < MIN_RECORD_SIZE) {
            throw new GeneralSecurityException("The record size " + recordSize + " is under " + MIN_RECORD_SIZE + ".");
        }
        if (body.length - HEADER_BYTES > recordSize) {
            throw new GeneralSecurityException("The body holds more than one record.");
        }
        byte[] oneRecord = body.clone();
        ByteBuffer.wrap(oneRecord).putInt(SALT_BYTES, TINK_RECORD_SIZE);
        return new WebPushHybridDecrypt.Builder()
            .withRecipientPrivateKey(privateKey(keys.privateKey))
            .withRecipientPublicKey(keys.publicKey)
            .withAuthSecret(keys.auth)
            .withRecordSize(TINK_RECORD_SIZE)
            .build()
            .decrypt(oneRecord, null);
    }

    /** The P-256 private key of its PKCS#8 encoding. */
    private static ECPrivateKey privateKey(byte[] pkcs8) throws GeneralSecurityException {
        return (ECPrivateKey) KeyFactory.getInstance("EC").generatePrivate(new PKCS8EncodedKeySpec(pkcs8));
    }
}
