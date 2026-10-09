package co.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.service.notification.StatusBarNotification;
import java.util.Arrays;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowPendingIntent;

/**
 * {@link PushNotifier} shows the Notification of a push: the payload of
 * ADR-0030, or "Something needs you" when the push does not decrypt or
 * does not parse.
 */
@RunWith(RobolectricTestRunner.class)
public class PushNotifierTest {

    private static final String ORIGIN = "https://pagis.example.com";

    private Context context;
    private NotificationManager manager;
    private PushNotifier notifier;

    @Before
    public void makeNotifier() {
        context = RuntimeEnvironment.getApplication();
        manager = context.getSystemService(NotificationManager.class);
        notifier = new PushNotifier(context);
    }

    @Test
    public void anApprovalShowsItsTextTagGroupNumberAndPlace() {
        notifier.show(approval("request:r-1", 3));

        StatusBarNotification shown = onlyNotification();
        Notification notification = shown.getNotification();
        assertEquals("Robin", notification.extras.getCharSequence(Notification.EXTRA_TITLE).toString());
        assertEquals("Robin needs your approval\nhost_shell", notification.extras.getCharSequence(Notification.EXTRA_TEXT).toString());
        assertEquals("request:r-1", shown.getTag());
        assertEquals("approval", notification.getGroup());
        assertEquals(3, notification.number);
        assertEquals(PushNotifier.CHANNEL_NEEDS_YOU, notification.getChannelId());
        assertEquals("https://pagis.example.com/c/ch-1", navigate(notification));
        assertEquals(MainActivity.class.getName(), contentIntent(notification).getComponent().getClassName());
    }

    @Test
    public void eachKindGoesToItsChannel() {
        for (String kind : Arrays.asList("approval", "waiting", "keypad", "call", "failed", "test")) {
            notifier.show(new PushPayload("T", "B", ORIGIN + "/", null, kind + ":1", kind, null));
        }

        for (StatusBarNotification shown : manager.getActiveNotifications()) {
            String kind = shown.getNotification().getGroup();
            String expected = Arrays.asList("approval", "waiting", "keypad").contains(kind)
                ? PushNotifier.CHANNEL_NEEDS_YOU
                : PushNotifier.CHANNEL_ACTIVITY;
            assertEquals(kind, expected, shown.getNotification().getChannelId());
        }
        assertEquals(6, manager.getActiveNotifications().length);
    }

    @Test
    public void theChannelsAreNeedsYouWithHighImportanceAndActivity() {
        notifier.show(approval("request:r-1", 3));

        NotificationChannel needsYou = manager.getNotificationChannel(PushNotifier.CHANNEL_NEEDS_YOU);
        NotificationChannel activity = manager.getNotificationChannel(PushNotifier.CHANNEL_ACTIVITY);
        assertEquals("Needs you", needsYou.getName().toString());
        assertEquals(NotificationManager.IMPORTANCE_HIGH, needsYou.getImportance());
        assertEquals("Activity", activity.getName().toString());
        assertEquals(NotificationManager.IMPORTANCE_DEFAULT, activity.getImportance());
    }

    @Test
    public void aNewPushForTheSameItemReplacesTheOldOne() {
        notifier.show(approval("request:r-1", 3));
        notifier.show(new PushPayload("Robin", "Robin still waits", ORIGIN + "/c/ch-1", 4, "request:r-1", "approval", null));

        Notification notification = onlyNotification().getNotification();
        assertEquals("Robin still waits", notification.extras.getCharSequence(Notification.EXTRA_TEXT).toString());
        assertEquals(4, notification.number);
    }

    @Test
    public void eachItemHasItsOwnPlace() {
        notifier.show(new PushPayload("A", "A", ORIGIN + "/a", null, "run:a", "failed", null));
        notifier.show(new PushPayload("B", "B", ORIGIN + "/b", null, "run:b", "failed", null));

        for (StatusBarNotification shown : manager.getActiveNotifications()) {
            String expected = shown.getTag().equals("run:a") ? ORIGIN + "/a" : ORIGIN + "/b";
            assertEquals(expected, navigate(shown.getNotification()));
        }
    }

    @Test
    public void aPayloadWithNoBadgeSetsNoNumber() {
        notifier.show(new PushPayload("Pagis", "Notifications work here.", ORIGIN + "/settings/notifications", null, "test", "test", null));

        assertEquals(0, onlyNotification().getNotification().number);
    }

    @Test
    public void anApprovalShowsNoActionWhenThePhoneStoresNoSetting() {
        notifier.show(approval("request:r-1", 3));

        assertNull(onlyNotification().getNotification().actions);
    }

    @Test
    public void anApprovalShowsNoActionWithLockScreenAnswersOff() {
        lockScreenAnswers(false);

        notifier.show(approval("request:r-1", 3));

        assertNull(onlyNotification().getNotification().actions);
    }

    @Test
    public void anApprovalShowsApproveOnceAndDenyThatEachSendABroadcast() {
        lockScreenAnswers(true);

        notifier.show(approval("request:r-1", 3));

        Notification.Action[] actions = onlyNotification().getNotification().actions;
        assertEquals(2, actions.length);
        assertEquals("Approve once", actions[0].title.toString());
        assertEquals("Deny", actions[1].title.toString());
        String[] names = { "approve_once", "deny" };
        for (int index = 0; index < names.length; index++) {
            ShadowPendingIntent pending = shadowOf(actions[index].actionIntent);
            assertTrue(names[index], pending.isBroadcastIntent());
            assertTrue(names[index], (pending.getFlags() & PendingIntent.FLAG_IMMUTABLE) != 0);
            Intent intent = pending.getSavedIntent();
            assertEquals(ApprovalReceiver.class.getName(), intent.getComponent().getClassName());
            assertEquals(names[index], intent.getAction());
            assertEquals("r-1", intent.getStringExtra(ApprovalReceiver.EXTRA_REQUEST));
            assertEquals("request:r-1", intent.getStringExtra(ApprovalReceiver.EXTRA_ITEM));
            assertEquals("approval", intent.getStringExtra(ApprovalReceiver.EXTRA_KIND));
            assertEquals("Robin", intent.getStringExtra(ApprovalReceiver.EXTRA_TITLE));
            assertEquals("https://pagis.example.com/c/ch-1", intent.getStringExtra(PushNotifier.EXTRA_NAVIGATE));
        }
    }

    /** With Answer on the lock screen on, each action still needs an unlocked phone. */
    @Test
    @Config(sdk = { 31, 36 })
    public void bothActionsRequireAnUnlockedPhone() {
        lockScreenAnswers(true);

        notifier.show(approval("request:r-1", 3));

        for (Notification.Action action : onlyNotification().getNotification().actions) {
            assertTrue(action.title.toString(), action.isAuthenticationRequired());
        }
    }

    @Test
    public void eachApprovalAnswersItsOwnRequest() {
        lockScreenAnswers(true);

        notifier.show(approval("request:r-1", 3));
        notifier.show(new PushPayload(
            "Sam", "Sam needs your approval", ORIGIN + "/c/ch-2", 4, "request:r-2", "approval",
            new PushPayload.Request("r-2", Arrays.asList("approve_once", "deny"))
        ));

        for (StatusBarNotification shown : manager.getActiveNotifications()) {
            String expected = shown.getTag().equals("request:r-1") ? "r-1" : "r-2";
            for (Notification.Action action : shown.getNotification().actions) {
                assertEquals(expected, shadowOf(action.actionIntent).getSavedIntent().getStringExtra(ApprovalReceiver.EXTRA_REQUEST));
            }
        }
    }

    @Test
    public void aKindOtherThanAnApprovalGetsNoAction() {
        lockScreenAnswers(true);

        notifier.show(new PushPayload("Robin", "Robin waits for you", ORIGIN + "/c/ch-1", 3, "run:r-1", "waiting", null));

        assertNull(onlyNotification().getNotification().actions);
    }

    @Test
    public void aRequestWithOtherActionsGetsNoAction() {
        lockScreenAnswers(true);

        notifier.show(new PushPayload(
            "Robin", "Robin asks a question", ORIGIN + "/c/ch-1", 3, "request:r-1", "approval",
            new PushPayload.Request("r-1", Arrays.asList("approve_once"))
        ));

        assertNull(onlyNotification().getNotification().actions);
    }

    @Test
    public void aPushThatDecryptsShowsItsPayload() {
        notifier.show(RFC8291Vector.pushHolding(PushPayloadTest.approval(message -> {})), true, RFC8291Vector.keys(), ORIGIN);

        StatusBarNotification shown = onlyNotification();
        assertEquals("request:r-1", shown.getTag());
        assertEquals("Robin", shown.getNotification().extras.getCharSequence(Notification.EXTRA_TITLE).toString());
    }

    @Test
    public void aBodyThatDoesNotDecryptShowsTheFallback() throws Exception {
        WebPushFixture fixture = WebPushFixture.load();
        String body = RFC8291Vector.pushHolding(PushPayloadTest.approval(message -> {}));

        notifier.show(body, true, fixture.keys, ORIGIN);

        assertFallback(onlyNotification().getNotification());
    }

    @Test
    public void aPayloadWhoseVersionIsNotOneShowsTheFallback() {
        String body = RFC8291Vector.pushHolding(PushPayloadTest.approval(message -> PushPayloadTest.data(message).put("v", 2)));

        notifier.show(body, true, RFC8291Vector.keys(), ORIGIN);

        assertFallback(onlyNotification().getNotification());
    }

    @Test
    public void aPushWithNoBodyOrNoKeysShowsTheFallback() {
        notifier.show(null, true, RFC8291Vector.keys(), ORIGIN);
        assertFallback(onlyNotification().getNotification());

        manager.cancelAll();
        notifier.show("not base64url!", true, RFC8291Vector.keys(), ORIGIN);
        assertFallback(onlyNotification().getNotification());

        manager.cancelAll();
        notifier.show(RFC8291Vector.pushHolding(PushPayloadTest.approval(message -> {})), true, null, ORIGIN);
        assertFallback(onlyNotification().getNotification());
    }

    @Test
    public void theFallbackTakesTheChannelOfThePriorityOfThePush() {
        notifier.show(null, true, null, ORIGIN);
        assertEquals(PushNotifier.CHANNEL_NEEDS_YOU, onlyNotification().getNotification().getChannelId());

        manager.cancelAll();
        notifier.show(null, false, null, ORIGIN);
        assertEquals(PushNotifier.CHANNEL_ACTIVITY, onlyNotification().getNotification().getChannelId());
    }

    @Test
    public void theFallbackWithNoServerOpensTheApp() {
        notifier.show(null, true, null, null);

        Intent intent = contentIntent(onlyNotification().getNotification());
        assertEquals(MainActivity.class.getName(), intent.getComponent().getClassName());
        assertFalse(intent.hasExtra(PushNotifier.EXTRA_NAVIGATE));
    }

    private void assertFallback(Notification notification) {
        assertEquals("Pagis", notification.extras.getCharSequence(Notification.EXTRA_TITLE).toString());
        assertEquals("Something needs you", notification.extras.getCharSequence(Notification.EXTRA_TEXT).toString());
        assertEquals(ORIGIN, navigate(notification));
        assertNull(notification.getGroup());
    }

    /** Set Answer on the lock screen of this phone. */
    private void lockScreenAnswers(boolean on) {
        new ServerStore(context).setLockScreenAnswers(on);
    }

    private StatusBarNotification onlyNotification() {
        StatusBarNotification[] shown = manager.getActiveNotifications();
        assertEquals(1, shown.length);
        return shown[0];
    }

    private static PushPayload approval(String item, Integer badge) {
        return new PushPayload(
            "Robin",
            "Robin needs your approval\nhost_shell",
            "https://pagis.example.com/c/ch-1",
            badge,
            item,
            "approval",
            new PushPayload.Request("r-1", Arrays.asList("approve_once", "deny"))
        );
    }

    private static String navigate(Notification notification) {
        return contentIntent(notification).getStringExtra(PushNotifier.EXTRA_NAVIGATE);
    }

    private static Intent contentIntent(Notification notification) {
        return shadowOf(notification.contentIntent).getSavedIntent();
    }
}
