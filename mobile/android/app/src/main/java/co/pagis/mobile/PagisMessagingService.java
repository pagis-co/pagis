package co.pagis.mobile;

import android.util.Log;
import androidx.annotation.NonNull;
import com.google.firebase.messaging.FirebaseMessagingService;
import com.google.firebase.messaging.RemoteMessage;
import java.io.IOException;
import java.security.SecureRandom;

/**
 * The Firebase messaging service of the app. The app owns it, and no
 * Capacitor plugin does, because the decryption and the inline answer of a
 * Notification need it (ADR-0032).
 */
public class PagisMessagingService extends FirebaseMessagingService {

    private static final String TAG = "Pagis";

    /**
     * A push of the Push Relay: a data message with the encrypted body
     * {@code p}. Android shows nothing for a data message, so the app
     * decrypts it and shows the Notification.
     */
    @Override
    public void onMessageReceived(@NonNull RemoteMessage message) {
        ServerOrigin server = new ServerStore(this).server();
        new PushNotifier(this).show(
            message.getData().get("p"),
            message.getOriginalPriority() == RemoteMessage.PRIORITY_HIGH,
            storedKeys(),
            server == null ? null : server.serverUrl()
        );
    }

    /** FCM gave a new token. It goes to the Push Relay, and the endpoint stays the same. */
    @Override
    public void onNewToken(@NonNull String token) {
        try {
            PushSubscriber.of(this).tokenChanged(token);
        } catch (PushException ex) {
            Log.w(TAG, "The Push Relay did not take the new token: " + ex.getMessage());
        }
    }

    /** The keys of the Push Subscription, or null when the app has none or cannot read them. */
    private PushKeys storedKeys() {
        try {
            return new PushKeyStore(SealedFiles.push(this), new SecureRandom()).stored();
        } catch (IOException | IllegalStateException ex) {
            // The Notification still shows, as the fallback.
            Log.w(TAG, "The app cannot read the keys of its notifications: " + ex.getMessage());
            return null;
        }
    }
}
