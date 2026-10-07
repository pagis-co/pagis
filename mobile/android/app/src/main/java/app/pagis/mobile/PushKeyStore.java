package app.pagis.mobile;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.math.BigInteger;
import java.security.GeneralSecurityException;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.SecureRandom;
import java.security.interfaces.ECPublicKey;
import java.security.spec.ECGenParameterSpec;

/** Keeps the keys of the Push Subscription in the secret items of the app. */
final class PushKeyStore {

    /** The name of the item that holds the keys. */
    static final String ITEM = "push.keys";

    private static final int AUTH_BYTES = 16;
    private static final int COORDINATE_BYTES = 32;

    private final SecretStore items;
    private final SecureRandom random;

    PushKeyStore(SecretStore items, SecureRandom random) {
        this.items = items;
        this.random = random;
    }

    /** The stored keys, or new keys that the store keeps from now on. */
    PushKeys keys() throws IOException, GeneralSecurityException {
        byte[] stored = items.read(ITEM);
        if (stored != null) return decode(stored);
        PushKeys keys = make();
        items.write(ITEM, encode(keys));
        return keys;
    }

    /**
     * The stored keys, or null when the app has no Push Subscription. The
     * messaging service reads the keys and never makes them.
     */
    PushKeys stored() throws IOException {
        byte[] stored = items.read(ITEM);
        return stored == null ? null : decode(stored);
    }

    void delete() throws IOException {
        items.delete(ITEM);
    }

    /** A P-256 key pair from {@code secp256r1} and 16 random bytes. */
    private PushKeys make() throws GeneralSecurityException {
        KeyPairGenerator generator = KeyPairGenerator.getInstance("EC");
        generator.initialize(new ECGenParameterSpec("secp256r1"), random);
        KeyPair pair = generator.generateKeyPair();
        ECPublicKey publicKey = (ECPublicKey) pair.getPublic();
        byte[] point = new byte[1 + 2 * COORDINATE_BYTES];
        point[0] = 0x04;
        copyUnsigned(publicKey.getW().getAffineX(), point, 1);
        copyUnsigned(publicKey.getW().getAffineY(), point, 1 + COORDINATE_BYTES);
        byte[] auth = new byte[AUTH_BYTES];
        random.nextBytes(auth);
        return new PushKeys(pair.getPrivate().getEncoded(), point, auth);
    }

    /** Write {@code value} as 32 big-endian bytes at {@code offset}. */
    private static void copyUnsigned(BigInteger value, byte[] target, int offset) {
        byte[] bytes = value.toByteArray();
        // `toByteArray` can add a sign byte, or give fewer than 32 bytes.
        int length = Math.min(bytes.length, COORDINATE_BYTES);
        System.arraycopy(bytes, bytes.length - length, target, offset + COORDINATE_BYTES - length, length);
    }

    private static byte[] encode(PushKeys keys) throws IOException {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        DataOutputStream out = new DataOutputStream(bytes);
        for (byte[] part : new byte[][] { keys.privateKey, keys.publicKey, keys.auth }) {
            out.writeInt(part.length);
            out.write(part);
        }
        out.flush();
        return bytes.toByteArray();
    }

    private static PushKeys decode(byte[] stored) throws IOException {
        DataInputStream in = new DataInputStream(new ByteArrayInputStream(stored));
        return new PushKeys(readPart(in), readPart(in), readPart(in));
    }

    private static byte[] readPart(DataInputStream in) throws IOException {
        byte[] part = new byte[in.readInt()];
        in.readFully(part);
        return part;
    }
}
