package co.pagis.mobile;

import android.content.Context;
import com.google.crypto.tink.Aead;
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

    private final File file;
    private final Aead aead;

    SessionCopy(File file, Aead aead) {
        this.file = file;
        this.aead = aead;
    }

    /** The copy of this app, which the AEAD of {@link AppKeyset} encrypts. */
    static SessionCopy open(Context context) {
        return new SessionCopy(new File(context.getFilesDir(), FILE), AppKeyset.aead(context));
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
