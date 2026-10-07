package app.pagis.mobile;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;

import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.Arrays;
import org.junit.Test;

/**
 * {@link WebPushDecrypt} opens the one {@code aes128gcm} record of a Web
 * Push (RFC 8291, RFC 8188) with the keys of the Push Subscription.
 */
public class WebPushDecryptTest {

    @Test
    public void theVectorOfRfc8291AppendixAGivesItsPlaintext() throws Exception {
        byte[] plaintext = WebPushDecrypt.decrypt(RFC8291Vector.body(), RFC8291Vector.keys());

        assertEquals("When I grow up, I want to be a watermelon", new String(plaintext, StandardCharsets.UTF_8));
        assertArrayEquals(RFC8291Vector.PLAINTEXT, plaintext);
    }

    @Test
    public void theFixtureOfPagisPushGivesItsPlaintext() throws Exception {
        WebPushFixture fixture = WebPushFixture.load();

        byte[] plaintext = WebPushDecrypt.decrypt(fixture.body, fixture.keys);

        assertEquals(fixture.plaintext, new String(plaintext, StandardCharsets.UTF_8));
    }

    @Test
    public void thePaddingAfterTheDelimiterIsRemoved() throws Exception {
        byte[] body = RFC8291Vector.bodySealing(new byte[] { 'h', 'e', 'l', 'l', 'o', 0x02, 0x00, 0x00, 0x00 });

        byte[] plaintext = WebPushDecrypt.decrypt(body, RFC8291Vector.keys());

        assertEquals("hello", new String(plaintext, StandardCharsets.UTF_8));
    }

    @Test
    public void aWrongAuthSecretIsAnError() {
        PushKeys keys = RFC8291Vector.keys();
        byte[] auth = new byte[16];
        Arrays.fill(auth, (byte) 7);

        assertRefused(RFC8291Vector.body(), new PushKeys(keys.privateKey, keys.publicKey, auth));
    }

    @Test
    public void anotherPrivateKeyIsAnError() throws Exception {
        PushKeys other = new PushKeyStore(new MemorySecrets(new ArrayList<>()), new SecureRandom()).keys();

        assertRefused(RFC8291Vector.body(), new PushKeys(other.privateKey, other.publicKey, RFC8291Vector.AUTH));
    }

    @Test
    public void aCutBodyIsAnError() {
        byte[] body = RFC8291Vector.body();

        assertRefused(Arrays.copyOf(body, body.length - 1));
        assertRefused(Arrays.copyOf(body, RFC8291Vector.HEADER.length + 16));
        assertRefused(RFC8291Vector.HEADER);
        assertRefused(Arrays.copyOf(body, 40));
        assertRefused(new byte[0]);
    }

    @Test
    public void aBadHeaderIsAnError() {
        byte[] shortKeyId = RFC8291Vector.body();
        shortKeyId[20] = 64;
        assertRefused(shortKeyId);

        byte[] notAPoint = RFC8291Vector.body();
        notAPoint[21] = 0x05;
        assertRefused(notAPoint);

        assertRefused(withRecordSize(RFC8291Vector.body(), 17));
    }

    @Test
    public void aSecondRecordIsAnError() {
        // The record size is 30, so the 58 bytes of ciphertext are two
        // records.
        assertRefused(withRecordSize(RFC8291Vector.body(), 30));
    }

    @Test
    public void aWrongPaddingDelimiterIsAnError() {
        // 0x01 ends a record that is not the last one (RFC 8188).
        assertRefused(RFC8291Vector.bodySealing(RFC8291Vector.concat(RFC8291Vector.PLAINTEXT, new byte[] { 0x01 })));
        assertRefused(RFC8291Vector.bodySealing(RFC8291Vector.concat(RFC8291Vector.PLAINTEXT, new byte[] { 0x02, 0x07 })));
        assertRefused(RFC8291Vector.bodySealing(new byte[] { 0x00, 0x00 }));
    }

    private static byte[] withRecordSize(byte[] body, int recordSize) {
        ByteBuffer.wrap(body).putInt(16, recordSize);
        return body;
    }

    private static void assertRefused(byte[] body) {
        assertRefused(body, RFC8291Vector.keys());
    }

    private static void assertRefused(byte[] body, PushKeys keys) {
        assertThrows(GeneralSecurityException.class, () -> WebPushDecrypt.decrypt(body, keys));
    }
}
