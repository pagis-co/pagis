package co.pagis.mobile;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.graphics.drawable.Icon;
import android.os.Build;
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
    /**
     * The channel of the Notification of the foreground service that sends
     * an answer before Android 12. It has low importance, so it makes no
     * sound.
     */
    static final String CHANNEL_ANSWERS = "answers";
    /** The extra of the content intent that holds the place of the Notification. */
    static final String EXTRA_NAVIGATE = "navigate";

    private static final String TAG = "Pagis";
    /** The kinds that the daemon sends with {@code Urgency: high} (ADR-0030). */
    private static final Set<String> NEEDS_YOU_KINDS = new HashSet<>(Arrays.asList("approval", "waiting", "keypad"));
    /** The id of each Notification of a payload. Its tag is the item. */
    private static final int ITEM_ID = 1;
    /** The id of the fallback Notification, which has no tag. A new one replaces the old one. */
    private static final int FALLBACK_ID = 2;
    /** The id of the Notification of the foreground service that sends an answer. */
    static final int SENDING_ID = 3;

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
     * item replaces the old Notification. A payload whose Request has the
     * actions {@code approve_once} and {@code deny} shows the actions
     * Approve once and Deny.
     */
    void show(PushPayload payload) {
        Notification.Builder builder = itemBuilder(payload.item, payload.kind, payload.title, payload.body, payload.navigate);
        if (payload.badge != null) builder.setNumber(payload.badge);
        if (payload.request != null && payload.request.actions.equals(ApprovalDecision.actions())) {
            for (ApprovalDecision decision : ApprovalDecision.values()) {
                builder.addAction(answerAction(decision, payload));
            }
        }
        notify(payload.item, ITEM_ID, builder.build());
    }

    /** Remove the Notification of {@code item}. */
    void cancel(String item) {
        manager.cancel(item, ITEM_ID);
    }

    /**
     * Replace the Notification of {@code item}, whose answer did not go
     * through, with one that has the same tag, title, channel, group and
     * place, no actions, and the text "Pagis did not take this answer.
     * Open Pagis to see the request."
     */
    void showAnswerFailed(String item, String kind, String title, String navigate) {
        String text = context.getString(R.string.answer_failed);
        notify(item, ITEM_ID, itemBuilder(item, kind, title, text, navigate).build());
    }

    /**
     * The Notification of the foreground service that sends an answer.
     * WorkManager shows it before Android 12 only, where it runs expedited
     * work in a foreground service.
     */
    Notification sending() {
        manager.createNotificationChannel(
            new NotificationChannel(CHANNEL_ANSWERS, context.getString(R.string.channel_answers), NotificationManager.IMPORTANCE_LOW)
        );
        return new Notification.Builder(context, CHANNEL_ANSWERS)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(context.getString(R.string.answer_sending))
            .build();
    }

    private Notification.Builder itemBuilder(String item, String kind, String title, String text, String navigate) {
        String channel = NEEDS_YOU_KINDS.contains(kind) ? CHANNEL_NEEDS_YOU : CHANNEL_ACTIVITY;
        return builder(channel, title, text, navigate, item.hashCode()).setGroup(kind);
    }

    /**
     * The action of {@code decision}: an immutable broadcast to
     * {@link ApprovalReceiver}. The intent action is the name of the action,
     * so the two actions of one Notification have two pending intents. Each
     * action asks the Person to unlock the phone, on Android 12 and later.
     */
    private Notification.Action answerAction(ApprovalDecision decision, PushPayload payload) {
        Intent answer = new Intent(context, ApprovalReceiver.class)
            .setAction(decision.action)
            .putExtra(ApprovalReceiver.EXTRA_REQUEST, payload.request.id)
            .putExtra(ApprovalReceiver.EXTRA_ITEM, payload.item)
            .putExtra(ApprovalReceiver.EXTRA_KIND, payload.kind)
            .putExtra(ApprovalReceiver.EXTRA_TITLE, payload.title)
            .putExtra(EXTRA_NAVIGATE, payload.navigate);
        PendingIntent broadcast = PendingIntent.getBroadcast(
            context,
            payload.item.hashCode(),
            answer,
            PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT
        );
        Notification.Action.Builder action = new Notification.Action.Builder(
            Icon.createWithResource(context, R.drawable.ic_notification),
            context.getString(decision.title),
            broadcast
        );
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            action.setAuthenticationRequired(!new ServerStore(context).lockScreenAnswers());
        }
        return action.build();
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
