package app.pagis.mobile;

import android.content.Context;
import java.io.IOException;
import java.security.GeneralSecurityException;
import java.security.SecureRandom;

/**
 * The Push Subscription of the app through the Push Relay (ADR-0032). The
 * relay gives the FCM token a Web Push endpoint, and the app makes the
 * keys that the daemon encrypts each push to.
 *
 * Each call can wait on the network, so it runs on a worker thread. The
 * calls of one subscriber take turns.
 */
final class PushSubscriber {

    private static PushSubscriber app;

    private final RelayClient relay;
    private final RegistrationStore registrations;
    private final PushKeyStore keys;

    PushSubscriber(RelayClient relay, RegistrationStore registrations, PushKeyStore keys) {
        this.relay = relay;
        this.registrations = registrations;
        this.keys = keys;
    }

    /**
     * The subscriber of the app: the relay of the build constant
     * {@code PUSH_RELAY_ORIGIN}, and the sealed files of the app. The plugin
     * and the messaging service share it.
     */
    static synchronized PushSubscriber of(Context context) {
        if (app == null) {
            SealedFiles items = SealedFiles.push(context);
            app = new PushSubscriber(
                new RelayClient(BuildConfig.PUSH_RELAY_ORIGIN),
                new RegistrationStore(items),
                new PushKeyStore(items, new SecureRandom())
            );
        }
        return app;
    }

    /**
     * Ask for the permission, get the token, register with the relay for
     * {@code vapidKey}, make the keys, and answer the subscription. With a
     * registration for the same key, it registers nothing: it sends a token
     * that changed, and answers the stored values. A registration for
     * another key goes first, with its keys.
     */
    synchronized PushSubscription subscribe(PushPlatform platform, String vapidKey) throws PushException {
        if (!platform.askPermission()) {
            throw new PushException("Pagis cannot show notifications on this phone. Allow them in the Settings app of the phone.");
        }
        String token = platform.token();
        try {
            RelayRegistration registration = registrations.read();
            if (registration != null && !registration.vapidKey.equals(vapidKey)) {
                unsubscribe();
                registration = null;
            }
            if (registration != null && !registration.token.equals(token)) {
                registration = changeToken(registration, token);
            }
            if (registration == null) {
                registration = relay.register(token, vapidKey);
                registrations.write(registration);
            }
            PushKeys made = keys.keys();
            return new PushSubscription(registration.endpoint, made.p256dh(), made.authText());
        } catch (IOException | GeneralSecurityException ex) {
            throw new PushException("Pagis cannot keep the keys of its notifications: " + ex.getMessage(), ex);
        }
    }

    /** FCM gave a new token. It goes to the relay with {@code PUT}, and the endpoint stays the same. */
    synchronized void tokenChanged(String token) throws PushException {
        try {
            RelayRegistration registration = registrations.read();
            if (registration == null || registration.token.equals(token)) return;
            changeToken(registration, token);
        } catch (IOException ex) {
            throw new PushException("Pagis cannot keep the new token: " + ex.getMessage(), ex);
        }
    }

    /** Delete the registration with the relay, then the keys. */
    synchronized void unsubscribe() throws PushException {
        try {
            RelayRegistration registration = registrations.read();
            if (registration != null) relay.delete(registration);
            forget();
        } catch (IOException ex) {
            throw new PushException("Pagis cannot delete the keys of its notifications: " + ex.getMessage(), ex);
        }
    }

    /**
     * The registration with the new token, or null when the relay no longer
     * knows it. The relay removes a registration when FCM says that its
     * token is gone, and the app then forgets it and its keys.
     */
    private RelayRegistration changeToken(RelayRegistration registration, String token)
        throws PushException, IOException {
        if (!relay.changeToken(registration, token)) {
            forget();
            return null;
        }
        RelayRegistration changed = registration.withToken(token);
        registrations.write(changed);
        return changed;
    }

    private void forget() throws IOException {
        registrations.delete();
        keys.delete();
    }
}
