package app.pagis.mobile;

import android.content.Context;
import com.google.crypto.tink.Aead;
import com.google.crypto.tink.KeyTemplates;
import com.google.crypto.tink.KeysetHandle;
import com.google.crypto.tink.RegistryConfiguration;
import com.google.crypto.tink.aead.AeadConfig;
import com.google.crypto.tink.integration.android.AndroidKeysetManager;
import java.io.IOException;
import java.security.GeneralSecurityException;

/**
 * The Tink AEAD that encrypts the secret files of the app: the copy of
 * the Session and the items of the push (ADR-0032). The keyset comes from
 * {@link AndroidKeysetManager}, with a master key in the Android Keystore.
 */
final class AppKeyset {

    private static final String KEYSET = "pagis_session_keyset";
    private static final String KEYSET_PREFERENCES = "pagis_session_keys";
    private static final String MASTER_KEY = "android-keystore://pagis_session_master_key";

    /** One AEAD for the process, so two first uses do not make two keysets. */
    private static Aead aead;

    private AppKeyset() {}

    static synchronized Aead aead(Context context) {
        if (aead != null) return aead;
        try {
            AeadConfig.register();
            KeysetHandle keyset = new AndroidKeysetManager.Builder()
                .withSharedPref(context.getApplicationContext(), KEYSET, KEYSET_PREFERENCES)
                .withKeyTemplate(KeyTemplates.get("AES256_GCM"))
                .withMasterKeyUri(MASTER_KEY)
                .build()
                .getKeysetHandle();
            aead = keyset.getPrimitive(RegistryConfiguration.get(), Aead.class);
            return aead;
        } catch (GeneralSecurityException | IOException ex) {
            throw new IllegalStateException("The app cannot open the key of its secret files.", ex);
        }
    }
}
