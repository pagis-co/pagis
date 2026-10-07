package app.pagis.mobile;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.util.Log;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.util.Arrays;
import java.util.Base64;
import java.util.HashSet;
import java.util.Set;

/**
 * Shows the Notification of a push of the Push Relay (ADR-0032). The push
 * holds the body {@code p}, which the app decrypts with the keys of its
 * Push Subscription and parses as the payload of ADR-0030.
 */
final class PushNotifier {

    /** The channel of the kinds that the daemon sends with {@code Urgency: high}. */
    static final String CHANNEL_NEEDS_YOU = "needs_you";
    /** The channel of each other kind. */
    static final String CHANNEL_ACTIVITY = "activity";
    /** The extra of the content intent that holds the place of the Notification. */
    static final String EXTRA_NAVIGATE = "navigate";

    private static final String TAG = "Pagis";
    /** The kinds that the daemon sends with {@code Urgency: high} (ADR-0030). */
    private static final Set<String> NEEDS_YOU_KINDS = new HashSet<>(Arrays.asList("approval", "waiting", "keypad"));
    /** The id of each Notification of a payload. Its tag is the item. */
    private static final int ITEM_ID = 1;
    /** The id of the fallback Notification, which has no tag. A new one replaces the old one. */
    private static final int FALLBACK_ID = 2;

    private final Context context;
    private final NotificationManager manager;

    PushNotifier(Context context) {
        this.context = context;
        this.manager = context.getSystemService(NotificationManager.class);
    }

    /**
     * Show the payload in the body {@code body} of a push. When the app holds
     * no {@code keys}, or the body does not decrypt or does not parse, show
     * "Pagis" and "Something needs you" with the server {@code origin} as
     * its place. {@code high} tells that FCM got the push with the priority
     * {@code HIGH}, which the relay gives to {@code Urgency: high}.
     */
    void show(String body, boolean high, PushKeys keys, String origin) {
        PushPayload payload;
        try {
            payload = payload(body, keys);
        } catch (GeneralSecurityException | PushPayloadException | IllegalArgumentException ex) {
            Log.w(TAG, "The push shows the fallback Notification: " + ex.getMessage());
            showFallback(high, origin);
            return;
        }
        show(payload);
    }

    /**
     * Show {@code payload}. The tag is the item, so a new push for the same
     * item replaces the old Notification.
     */
    void show(PushPayload payload) {
        String channel = NEEDS_YOU_KINDS.contains(payload.kind) ? CHANNEL_NEEDS_YOU : CHANNEL_ACTIVITY;
        Notification.Builder builder = builder(channel, payload.title, payload.body, payload.navigate, payload.item.hashCode())
            .setGroup(payload.kind);
        if (payload.badge != null) builder.setNumber(payload.badge);
        notify(payload.item, ITEM_ID, builder.build());
    }

    private void showFallback(boolean high, String origin) {
        Notification.Builder builder = builder(
            high ? CHANNEL_NEEDS_YOU : CHANNEL_ACTIVITY,
            context.getString(R.string.app_name),
            context.getString(R.string.notification_fallback),
            origin,
            FALLBACK_ID
        );
        notify(null, FALLBACK_ID, builder.build());
    }

    private static PushPayload payload(String body, PushKeys keys)
        throws GeneralSecurityException, PushPayloadException {
        if (body == null) throw new GeneralSecurityException("The push has no body p.");
        if (keys == null) throw new GeneralSecurityException("The app holds no keys of a Push Subscription.");
        byte[] plaintext = WebPushDecrypt.decrypt(Base64.getUrlDecoder().decode(body), keys);
        return PushPayload.parse(new String(plaintext, StandardCharsets.UTF_8));
    }

    /**
     * A Notification of {@code channel} whose tap opens the app with
     * {@code navigate}. Each {@code requestCode} gives its own content
     * intent, so the extra of one Notification does not change another.
     */
    private Notification.Builder builder(String channel, String title, String text, String navigate, int requestCode) {
        Intent open = new Intent(context, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        if (navigate != null) open.putExtra(EXTRA_NAVIGATE, navigate);
        PendingIntent contentIntent = PendingIntent.getActivity(
            context,
            requestCode,
            open,
            PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT
        );
        return new Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(text)
            .setStyle(new Notification.BigTextStyle().bigText(text))
            .setContentIntent(contentIntent)
            .setAutoCancel(true);
    }

    private void notify(String tag, int id, Notification notification) {
        manager.createNotificationChannels(Arrays.asList(
            new NotificationChannel(CHANNEL_NEEDS_YOU, context.getString(R.string.channel_needs_you), NotificationManager.IMPORTANCE_HIGH),
            new NotificationChannel(CHANNEL_ACTIVITY, context.getString(R.string.channel_activity), NotificationManager.IMPORTANCE_DEFAULT)
        ));
        manager.notify(tag, id, notification);
    }
}
