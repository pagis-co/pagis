package app.pagis.mobile;

import android.content.Context;
import com.google.crypto.tink.Aead;
import com.google.crypto.tink.KeyTemplates;
import com.google.crypto.tink.KeysetHandle;
import com.google.crypto.tink.RegistryConfiguration;
import com.google.crypto.tink.aead.AeadConfig;
import com.google.crypto.tink.integration.android.AndroidKeysetManager;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.security.GeneralSecurityException;

/**
 * The native copy of the Session for native requests (ADR-0032). The
 * messaging service and a notification action run outside the web view,
 * so they read the copy and not the cookie store.
 *
 * The copy is a file that a Tink AEAD encrypts. The origin of the server
 * is the associated data, so a copy of one origin is no copy of another.
 */
final class SessionCopy {

    private static final String FILE = "pagis_session_copy";
    private static final String KEYSET = "pagis_session_keyset";
    private static final String KEYSET_PREFERENCES = "pagis_session_keys";
    private static final String MASTER_KEY = "android-keystore://pagis_session_master_key";

    private final File file;
    private final Aead aead;

    SessionCopy(File file, Aead aead) {
        this.file = file;
        this.aead = aead;
    }

    /**
     * The copy of this app. Its keyset comes from {@link AndroidKeysetManager},
     * with a master key in the Android Keystore.
     */
    static SessionCopy open(Context context) {
        try {
            AeadConfig.register();
            KeysetHandle keyset = new AndroidKeysetManager.Builder()
                .withSharedPref(context, KEYSET, KEYSET_PREFERENCES)
                .withKeyTemplate(KeyTemplates.get("AES256_GCM"))
                .withMasterKeyUri(MASTER_KEY)
                .build()
                .getKeysetHandle();
            Aead aead = keyset.getPrimitive(RegistryConfiguration.get(), Aead.class);
            return new SessionCopy(new File(context.getFilesDir(), FILE), aead);
        } catch (GeneralSecurityException | IOException ex) {
            throw new IllegalStateException("The app cannot open the key of the copy of the Session.", ex);
        }
    }

    /**
     * The copy for this origin, or null when there is no copy, the copy is
     * of another origin, or it does not decrypt.
     */
    Session read(ServerOrigin origin) {
        if (!file.exists()) return null;
        try {
            byte[] plain = aead.decrypt(Files.readAllBytes(file.toPath()), associatedData(origin));
            DataInputStream in = new DataInputStream(new ByteArrayInputStream(plain));
            return new Session(in.readUTF(), in.readLong());
        } catch (GeneralSecurityException | IOException ex) {
            return null;
        }
    }

    /** Keep {@code session} as the copy of {@code origin}, in place of the earlier copy. */
    void write(ServerOrigin origin, Session session) {
        try {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            DataOutputStream out = new DataOutputStream(bytes);
            out.writeUTF(session.value);
            out.writeLong(session.expiresAtMillis);
            out.flush();
            byte[] sealed = aead.encrypt(bytes.toByteArray(), associatedData(origin));
            File next = new File(file.getPath() + ".next");
            Files.write(next.toPath(), sealed);
            Files.move(next.toPath(), file.toPath(), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
        } catch (GeneralSecurityException | IOException ex) {
            throw new IllegalStateException("The app cannot keep the copy of the Session.", ex);
        }
    }

    /** Remove the copy. */
    void delete() {
        try {
            Files.deleteIfExists(file.toPath());
        } catch (IOException ex) {
            throw new IllegalStateException("The app cannot delete the copy of the Session.", ex);
        }
    }

    private static byte[] associatedData(ServerOrigin origin) {
        return origin.serverUrl().getBytes(StandardCharsets.UTF_8);
    }
}
