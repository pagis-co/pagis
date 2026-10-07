package co.pagis.mobile;

import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.AlgorithmParameters;
import java.security.KeyFactory;
import java.security.spec.ECGenParameterSpec;
import java.security.spec.ECParameterSpec;
import java.security.spec.ECPrivateKeySpec;
import java.util.Base64;
import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;

/**
 * The example of RFC 8291, section 5 and Appendix A: the keys, the auth
 * secret, the body and the plaintext.
 */
final class RFC8291Vector {

    static final byte[] PLAINTEXT = data("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24");

    /** The user agent private key, {@code ua_private}, as the 32 bytes of the scalar. */
    static final byte[] RECEIVER_PRIVATE_KEY = data("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94");
    /** The user agent public key, {@code ua_public}. */
    static final byte[] RECEIVER_PUBLIC_KEY = data("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
    static final byte[] AUTH = data("BTBZMqHH6r4Tts7J_aSIgg");

    /** The content encryption key and the nonce that Appendix A derives. */
    static final byte[] CEK = data("oIhVW04MRdy2XN9CiKLxTg");
    static final byte[] NONCE = data("4h_95klXJ5E_qnoN");

    /** The 86-octet header: the salt, the record size 4096 and {@code as_public}. */
    static final byte[] HEADER = data("DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8");
    static final byte[] CIPHERTEXT = data("8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ");

    private RFC8291Vector() {}

    /** The body of section 5: the header and the ciphertext. */
    static byte[] body() {
        return concat(HEADER, CIPHERTEXT);
    }

    /** The keys of the user agent, as the app stores them. */
    static PushKeys keys() {
        return new PushKeys(pkcs8(RECEIVER_PRIVATE_KEY), RECEIVER_PUBLIC_KEY.clone(), AUTH.clone());
    }

    /**
     * A body with the header of the example that holds {@code record},
     * sealed with the key and the nonce of the example. The keys of the
     * user agent open it.
     */
    static byte[] bodySealing(byte[] record) {
        try {
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(CEK, "AES"), new GCMParameterSpec(128, NONCE));
            return concat(HEADER, cipher.doFinal(record));
        } catch (Exception ex) {
            throw new IllegalStateException(ex);
        }
    }

    /** A body that holds {@code plaintext} and the delimiter of the last record. */
    static byte[] bodyHolding(byte[] plaintext) {
        return bodySealing(concat(plaintext, new byte[] { 0x02 }));
    }

    /** A body that holds the UTF-8 of {@code text}, as {@code p} of a push. */
    static String pushHolding(String text) {
        return Base64.getUrlEncoder().withoutPadding().encodeToString(bodyHolding(text.getBytes(StandardCharsets.UTF_8)));
    }

    /** The P-256 private key of the 32 bytes of {@code scalar}, in PKCS#8. */
    static byte[] pkcs8(byte[] scalar) {
        try {
            AlgorithmParameters parameters = AlgorithmParameters.getInstance("EC");
            parameters.init(new ECGenParameterSpec("secp256r1"));
            ECParameterSpec curve = parameters.getParameterSpec(ECParameterSpec.class);
            return KeyFactory.getInstance("EC")
                .generatePrivate(new ECPrivateKeySpec(new BigInteger(1, scalar), curve))
                .getEncoded();
        } catch (Exception ex) {
            throw new IllegalStateException(ex);
        }
    }

    static byte[] concat(byte[] first, byte[] second) {
        return ByteBuffer.allocate(first.length + second.length).put(first).put(second).array();
    }

    static byte[] data(String base64Url) {
        return Base64.getUrlDecoder().decode(base64Url);
    }
}
