package app.pagis.mobile;

import java.io.IOException;

/** Named secret items of the app. The app keeps them in {@link SealedFiles}, and a test in memory. */
interface SecretStore {

    /** The item {@code name}, or null when there is none. */
    byte[] read(String name) throws IOException;

    /** Keep {@code data} as the item {@code name}, in place of an earlier item. */
    void write(String name, byte[] data) throws IOException;

    /** Remove the item {@code name}. No item is no error. */
    void delete(String name) throws IOException;
}
