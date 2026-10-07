package app.pagis.mobile;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;

import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.Arrays;
import org.junit.Test;

/**
 * The keys of the Push Subscription of the app (RFC 8291). The private
 * key never leaves the app; the subscription gives the public key and the
 * auth secret.
 */
public class PushKeyStoreTest {

    private final MemorySecrets items = new MemorySecrets(new ArrayList<>());

    @Test
    public void theKeysGiveAnUncompressedPointAndSixteenBytesOfAuth() throws Exception {
        PushKeys keys = new PushKeyStore(items, new SecureRandom()).keys();

        assertEquals(65, keys.publicKey.length);
        assertEquals(0x04, keys.publicKey[0]);
        assertEquals(16, keys.auth.length);
    }

    @Test
    public void aSecondReadGivesTheSameStoredKeys() throws Exception {
        PushKeys first = new PushKeyStore(items, new SecureRandom()).keys();

        PushKeys second = new PushKeyStore(items, new SecureRandom()).keys();

        assertArrayEquals(first.publicKey, second.publicKey);
        assertArrayEquals(first.auth, second.auth);
        assertArrayEquals(first.privateKey, second.privateKey);
    }

    @Test
    public void deletedKeysAreMadeAgain() throws Exception {
        PushKeyStore store = new PushKeyStore(items, new SecureRandom());
        PushKeys first = store.keys();

        store.delete();

        PushKeys second = store.keys();
        assertFalse(Arrays.equals(first.publicKey, second.publicKey));
        assertFalse(Arrays.equals(first.auth, second.auth));
    }

    @Test
    public void theKeysGoOutAsBase64UrlWithNoPadding() throws Exception {
        PushKeys keys = new PushKeyStore(items, new SecureRandom()).keys();

        assertEquals(87, keys.p256dh().length());
        assertEquals(22, keys.authText().length());
        assertFalse(keys.p256dh().matches(".*[+/=].*"));
        assertFalse(keys.authText().matches(".*[+/=].*"));
    }
}
