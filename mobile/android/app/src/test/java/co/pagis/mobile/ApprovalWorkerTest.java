package co.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.robolectric.Shadows.shadowOf;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.Context;
import android.content.Intent;
import android.service.notification.StatusBarNotification;
import androidx.annotation.NonNull;
import androidx.work.ForegroundInfo;
import androidx.work.ListenableWorker;
import androidx.work.WorkerFactory;
import androidx.work.WorkerParameters;
import androidx.work.testing.TestWorkerBuilder;
import java.io.File;
import java.util.Arrays;
import java.util.concurrent.Executors;
import okhttp3.mockwebserver.MockResponse;
import okhttp3.mockwebserver.MockWebServer;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;

/**
 * {@link ApprovalWorker} posts the answer of a notification action and
 * then removes the Notification of the Approval, or shows the failure in
 * its place (ADR-0032). {@link MockWebServer} answers for the daemon, and
 * the copy of the Session uses a keyset of the test.
 */
@RunWith(RobolectricTestRunner.class)
public class ApprovalWorkerTest {

    private static final String FAILURE = "Pagis did not take this answer. Open Pagis to see the request.";
    private static final String PLACE = "https://pagis.example.com/c/ch-1";
    private static final int APPROVE_ONCE = 0;
    private static final int DENY = 1;

    @Rule
    public final MockWebServer daemon = new MockWebServer();

    @Rule
    public final TemporaryFolder folder = new TemporaryFolder();

    private Context context;
    private NotificationManager manager;
    private ServerOrigin server;
    private SessionCopy copy;

    @Before
    public void showApproval() throws Exception {
        context = RuntimeEnvironment.getApplication();
        manager = context.getSystemService(NotificationManager.class);
        String url = daemon.url("/").toString();
        server = ServerOrigin.parse(url.substring(0, url.length() - 1), true);
        new ServerStore(context).keep(server);
        copy = new SessionCopy(new File(folder.getRoot(), "session"), SessionCopyTest.testAead());
        copy.write(server, new Session("s1", 1_900_000_000_000L));
        showApprovalNotification();
    }

    @Test
    public void aTakenAnswerRemovesTheNotification() throws Exception {
        daemon.enqueue(new MockResponse().setResponseCode(200));

        ListenableWorker.Result result = answer(DENY, 0);

        assertEquals(ListenableWorker.Result.success(), result);
        assertEquals(0, manager.getActiveNotifications().length);
        assertEquals("/api/v1/requests/r-1/decision", daemon.takeRequest().getPath());
    }

    @Test
    public void a401ShowsTheFailureAndDeletesTheCopyOfTheSession() {
        daemon.enqueue(new MockResponse().setResponseCode(401));

        ListenableWorker.Result result = answer(APPROVE_ONCE, 0);

        assertEquals(ListenableWorker.Result.failure(), result);
        assertFailure(onlyNotification());
        assertNull(copy.read(server));
    }

    @Test
    public void eachOtherRefusalShowsTheFailureAndKeepsTheCopy() {
        for (int status : new int[] { 404, 409, 500 }) {
            showApprovalNotification();
            daemon.enqueue(new MockResponse().setResponseCode(status));

            ListenableWorker.Result result = answer(APPROVE_ONCE, 0);

            assertEquals(String.valueOf(status), ListenableWorker.Result.failure(), result);
            assertFailure(onlyNotification());
            assertNotNull(copy.read(server));
        }
    }

    @Test
    public void aNetworkErrorRetriesAndTheLastFailedAttemptShowsTheFailure() throws Exception {
        ServerOrigin nobody = ApprovalAnswerTest.closedPort();
        new ServerStore(context).keep(nobody);
        copy.write(nobody, new Session("s1", 1_900_000_000_000L));

        assertEquals(ListenableWorker.Result.retry(), answer(DENY, 0));
        assertNotNull(onlyNotification().getNotification().actions);
        assertEquals(ListenableWorker.Result.retry(), answer(DENY, 1));
        assertNotNull(onlyNotification().getNotification().actions);

        assertEquals(ListenableWorker.Result.failure(), answer(DENY, 2));
        assertFailure(onlyNotification());
    }

    @Test
    public void noCopyOfTheSessionShowsTheFailureAndPostsNothing() {
        copy.delete();

        assertEquals(ListenableWorker.Result.failure(), answer(DENY, 0));
        assertFailure(onlyNotification());
        assertEquals(0, daemon.getRequestCount());
    }

    /**
     * Before Android 12, WorkManager runs expedited work in a foreground
     * service, which shows a Notification. It goes to a channel of low
     * importance, which makes no sound.
     */
    @Test
    public void theNotificationOfTheForegroundServiceIsQuiet() {
        ForegroundInfo info = worker(DENY, 0).getForegroundInfo();

        Notification notification = info.getNotification();
        assertEquals("Pagis sends your answer.", notification.extras.getCharSequence(Notification.EXTRA_TITLE).toString());
        NotificationChannel channel = manager.getNotificationChannel(notification.getChannelId());
        assertEquals("Answers", channel.getName().toString());
        assertEquals(NotificationManager.IMPORTANCE_LOW, channel.getImportance());
    }

    private void showApprovalNotification() {
        new PushNotifier(context).show(new PushPayload(
            "Robin",
            "Robin needs your approval\nhost_shell",
            PLACE,
            3,
            "request:r-1",
            "approval",
            new PushPayload.Request("r-1", Arrays.asList("approve_once", "deny"))
        ));
    }

    private ListenableWorker.Result answer(int action, int attempt) {
        return worker(action, attempt).doWork();
    }

    /** The worker of one action of the Notification, with the input that its broadcast gives. */
    private ApprovalWorker worker(int action, int attempt) {
        Intent broadcast = shadowOf(onlyNotification().getNotification().actions[action].actionIntent).getSavedIntent();
        return TestWorkerBuilder.from(context, ApprovalWorker.class, Executors.newSingleThreadExecutor())
            .setInputData(ApprovalReceiver.input(broadcast))
            .setRunAttemptCount(attempt)
            .setWorkerFactory(new WorkerFactory() {
                @Override
                public ListenableWorker createWorker(
                    @NonNull Context appContext,
                    @NonNull String workerClassName,
                    @NonNull WorkerParameters parameters
                ) {
                    return new ApprovalWorker(appContext, parameters, new ApprovalAnswer(), () -> copy);
                }
            })
            .build();
    }

    /** The failure has the tag, the title, the channel and the place of the Approval, and no actions. */
    private static void assertFailure(StatusBarNotification shown) {
        Notification notification = shown.getNotification();
        assertEquals("request:r-1", shown.getTag());
        assertEquals("Robin", notification.extras.getCharSequence(Notification.EXTRA_TITLE).toString());
        assertEquals(FAILURE, notification.extras.getCharSequence(Notification.EXTRA_TEXT).toString());
        assertEquals(PushNotifier.CHANNEL_NEEDS_YOU, notification.getChannelId());
        assertEquals("approval", notification.getGroup());
        assertNull(notification.actions);
        Intent open = shadowOf(notification.contentIntent).getSavedIntent();
        assertEquals(MainActivity.class.getName(), open.getComponent().getClassName());
        assertEquals(PLACE, open.getStringExtra(PushNotifier.EXTRA_NAVIGATE));
    }

    private StatusBarNotification onlyNotification() {
        StatusBarNotification[] shown = manager.getActiveNotifications();
        assertEquals(1, shown.length);
        return shown[0];
    }
}
