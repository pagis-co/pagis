package co.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;

import com.google.crypto.tink.Aead;
import com.google.crypto.tink.KeysetHandle;
import com.google.crypto.tink.RegistryConfiguration;
import com.google.crypto.tink.aead.AeadConfig;
import com.google.crypto.tink.aead.PredefinedAeadParameters;
import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

/**
 * The native copy of the Session: a file that a Tink AEAD encrypts. The
 * app gets its AEAD from a keyset that a key of the Android Keystore
 * protects. A test has no Android Keystore, so it gives a keyset of its
 * own.
 */
public class SessionCopyTest {

    @Rule
    public final TemporaryFolder folder = new TemporaryFolder();

    private final ServerOrigin server = ServerOrigin.parse("https://a.example", false);
    private final Session session = new Session("s1", 1_900_000_000_000L);
    private File file;
    private SessionCopy copy;

    static Aead testAead() throws Exception {
        AeadConfig.register();
        return KeysetHandle.generateNew(PredefinedAeadParameters.AES256_GCM)
            .getPrimitive(RegistryConfiguration.get(), Aead.class);
    }

    @Before
    public void makeCopy() throws Exception {
        file = new File(folder.getRoot(), "session");
        copy = new SessionCopy(file, testAead());
    }

    @Test
    public void theCopyReadsBackTheValueThatWasWritten() {
        copy.write(server, session);

        assertEquals(session, copy.read(server));
    }

    @Test
    public void theFileHoldsNoClearTextOfTheSession() throws Exception {
        copy.write(server, new Session("the-session-value", 1_900_000_000_000L));

        String bytes = new String(Files.readAllBytes(file.toPath()), StandardCharsets.ISO_8859_1);
        assertFalse(bytes.contains("the-session-value"));
    }

    @Test
    public void theCopyOfOneOriginIsNoCopyOfAnother() {
        copy.write(server, session);

        assertNull(copy.read(ServerOrigin.parse("https://b.example", false)));
        assertNull(copy.read(ServerOrigin.parse("https://a.example:444", false)));
    }

    @Test
    public void aNewSessionReplacesTheCopy() {
        copy.write(server, session);
        copy.write(server, new Session("s2", 1_900_000_000_000L));

        assertEquals("s2", copy.read(server).value);
    }

    @Test
    public void aDeletedCopyReadsAsNone() {
        copy.write(server, session);
        copy.delete();

        assertNull(copy.read(server));
        assertFalse(file.exists());
        copy.delete();
    }

    /** A file that another key encrypted, or that is broken, is no copy. */
    @Test
    public void aCopyThatDoesNotDecryptReadsAsNone() throws Exception {
        copy.write(server, session);
        assertNull(new SessionCopy(file, testAead()).read(server));

        Files.write(file.toPath(), new byte[] { 1, 2, 3 });
        assertNull(copy.read(server));
    }

    @Test
    public void noFileIsNoCopy() {
        assertNull(copy.read(server));
    }
}
