package co.pagis.mobile;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;

/**
 * Keeps the registration with the Push Relay in the secret items of the
 * app, because it holds the secret.
 */
final class RegistrationStore {

    /** The name of the item that holds the registration. */
    static final String ITEM = "push.registration";

    private final SecretStore items;

    RegistrationStore(SecretStore items) {
        this.items = items;
    }

    /** The registration, or null when the app holds none. */
    RelayRegistration read() throws IOException {
        byte[] stored = items.read(ITEM);
        if (stored == null) return null;
        DataInputStream in = new DataInputStream(new ByteArrayInputStream(stored));
        return new RelayRegistration(in.readUTF(), in.readUTF(), in.readUTF(), in.readUTF(), in.readUTF());
    }

    void write(RelayRegistration registration) throws IOException {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        DataOutputStream out = new DataOutputStream(bytes);
        out.writeUTF(registration.id);
        out.writeUTF(registration.secret);
        out.writeUTF(registration.endpoint);
        out.writeUTF(registration.vapidKey);
        out.writeUTF(registration.token);
        out.flush();
        items.write(ITEM, bytes.toByteArray());
    }

    void delete() throws IOException {
        items.delete(ITEM);
    }
}
