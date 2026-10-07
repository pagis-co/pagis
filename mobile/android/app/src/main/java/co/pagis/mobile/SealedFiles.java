package co.pagis.mobile;

import android.content.Context;
import com.google.crypto.tink.Aead;
import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.security.GeneralSecurityException;

/**
 * Secret items in files of one directory, which the Tink AEAD of the app
 * encrypts. The name of an item is the associated data, so the file of one
 * item is no file of another.
 */
final class SealedFiles implements SecretStore {

    private final File directory;
    private final Aead aead;

    SealedFiles(File directory, Aead aead) {
        this.directory = directory;
        this.aead = aead;
    }

    /** The items of the push of this app. */
    static SealedFiles push(Context context) {
        return new SealedFiles(new File(context.getFilesDir(), "push"), AppKeyset.aead(context));
    }

    /** The item, or null when there is no file or the file does not decrypt. */
    @Override
    public byte[] read(String name) throws IOException {
        File file = new File(directory, name);
        if (!file.exists()) return null;
        try {
            return aead.decrypt(Files.readAllBytes(file.toPath()), associatedData(name));
        } catch (GeneralSecurityException ex) {
            return null;
        }
    }

    @Override
    public void write(String name, byte[] data) throws IOException {
        byte[] sealed;
        try {
            sealed = aead.encrypt(data, associatedData(name));
        } catch (GeneralSecurityException ex) {
            throw new IOException("The app cannot encrypt " + name + ".", ex);
        }
        Files.createDirectories(directory.toPath());
        File file = new File(directory, name);
        File next = new File(directory, name + ".next");
        Files.write(next.toPath(), sealed);
        Files.move(next.toPath(), file.toPath(), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
    }

    @Override
    public void delete(String name) throws IOException {
        Files.deleteIfExists(new File(directory, name).toPath());
    }

    private static byte[] associatedData(String name) {
        return name.getBytes(StandardCharsets.UTF_8);
    }
}
