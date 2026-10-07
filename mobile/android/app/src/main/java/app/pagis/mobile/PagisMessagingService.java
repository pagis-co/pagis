package app.pagis.mobile;

import android.util.Log;
import androidx.annotation.NonNull;
import com.google.firebase.messaging.FirebaseMessagingService;

/**
 * The Firebase messaging service of the app. The app owns it, and no
 * Capacitor plugin does, because the decryption and the inline answer of a
 * Notification need it (ADR-0032).
 */
public class PagisMessagingService extends FirebaseMessagingService {

    private static final String TAG = "Pagis";

    /** FCM gave a new token. It goes to the Push Relay, and the endpoint stays the same. */
    @Override
    public void onNewToken(@NonNull String token) {
        try {
            PushSubscriber.of(this).tokenChanged(token);
        } catch (PushException ex) {
            Log.w(TAG, "The Push Relay did not take the new token: " + ex.getMessage());
        }
    }
}
