package co.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.NotificationManager;
import android.content.Context;
import android.content.Intent;
import androidx.work.Configuration;
import androidx.work.Data;
import androidx.work.NetworkType;
import androidx.work.OneTimeWorkRequest;
import androidx.work.OutOfQuotaPolicy;
import androidx.work.WorkInfo;
import androidx.work.WorkManager;
import androidx.work.testing.SynchronousExecutor;
import androidx.work.testing.WorkManagerTestInitHelper;
import java.util.Arrays;
import java.util.List;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;

/**
 * {@link ApprovalReceiver} gets the broadcast of a notification action and
 * gives the answer to WorkManager (ADR-0032).
 */
@RunWith(RobolectricTestRunner.class)
public class ApprovalReceiverTest {

    private Context context;

    @Before
    public void findContext() {
        context = RuntimeEnvironment.getApplication();
    }

    @Test
    public void theWorkIsExpeditedAndWaitsForTheNetwork() {
        OneTimeWorkRequest request = ApprovalReceiver.request(broadcast(1));

        assertTrue(request.getWorkSpec().expedited);
        assertEquals(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST, request.getWorkSpec().outOfQuotaPolicy);
        assertEquals(NetworkType.CONNECTED, request.getWorkSpec().constraints.getRequiredNetworkType());
        assertEquals(ApprovalWorker.class.getName(), request.getWorkSpec().workerClassName);
    }

    @Test
    public void theInputHoldsTheActionTheRequestAndTheNotification() {
        Data input = ApprovalReceiver.input(broadcast(1));

        assertEquals("deny", input.getString(ApprovalReceiver.EXTRA_ACTION));
        assertEquals("r-1", input.getString(ApprovalReceiver.EXTRA_REQUEST));
        assertEquals("request:r-1", input.getString(ApprovalReceiver.EXTRA_ITEM));
        assertEquals("approval", input.getString(ApprovalReceiver.EXTRA_KIND));
        assertEquals("Robin", input.getString(ApprovalReceiver.EXTRA_TITLE));
        assertEquals("https://pagis.example.com/c/ch-1", input.getString(PushNotifier.EXTRA_NAVIGATE));
    }

    /** A second touch, also on the other action, sends no second answer while the first one waits. */
    @Test
    public void oneRequestHasOneAnswerAtATime() throws Exception {
        WorkManagerTestInitHelper.initializeTestWorkManager(
            context,
            new Configuration.Builder().setExecutor(new SynchronousExecutor()).build()
        );

        new ApprovalReceiver().onReceive(context, broadcast(1));
        new ApprovalReceiver().onReceive(context, broadcast(0));
        new ApprovalReceiver().onReceive(context, broadcast(1));

        List<WorkInfo> work = WorkManager.getInstance(context).getWorkInfosForUniqueWork("approval:r-1").get();
        assertEquals(1, work.size());
        assertEquals(WorkInfo.State.ENQUEUED, work.get(0).getState());
        assertEquals(NetworkType.CONNECTED, work.get(0).getConstraints().getRequiredNetworkType());
    }

    /** The broadcast of the action {@code index} of the Notification of an Approval. */
    private Intent broadcast(int index) {
        new PushNotifier(context).show(new PushPayload(
            "Robin",
            "Robin needs your approval\nhost_shell",
            "https://pagis.example.com/c/ch-1",
            3,
            "request:r-1",
            "approval",
            new PushPayload.Request("r-1", Arrays.asList("approve_once", "deny"))
        ));
        NotificationManager manager = context.getSystemService(NotificationManager.class);
        return shadowOf(manager.getActiveNotifications()[0].getNotification().actions[index].actionIntent).getSavedIntent();
    }
}
